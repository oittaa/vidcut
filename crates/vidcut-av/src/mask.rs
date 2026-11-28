mod kernels;
mod net;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use vidcut_core::catalog::{self, File};

use crate::error::{Error, Result};
use crate::models::{self, Fetch, Need};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Tiny,
    BasePlus,
    Large,
}

struct Config {
    embed: u32,
    heads: u32,
    stages: [u32; 4],
    global: &'static [u32],
    windows: [u32; 4],
    bkg: u32,
}

impl Size {
    fn config(self) -> Config {
        match self {
            Self::Tiny => Config {
                embed: 96,
                heads: 1,
                stages: [1, 2, 7, 2],
                global: &[5, 7, 9],
                windows: [8, 4, 14, 7],
                bkg: 7,
            },
            Self::BasePlus => Config {
                embed: 112,
                heads: 2,
                stages: [2, 3, 16, 3],
                global: &[12, 16, 20],
                windows: [8, 4, 14, 7],
                bkg: 14,
            },
            Self::Large => Config {
                embed: 144,
                heads: 2,
                stages: [2, 6, 36, 4],
                global: &[23, 33, 43],
                windows: [8, 4, 16, 8],
                bkg: 7,
            },
        }
    }

    fn model(self) -> &'static catalog::Model {
        match self {
            Self::Tiny => &catalog::SAM21_TINY,
            Self::BasePlus => &catalog::SAM21_BASE_PLUS,
            Self::Large => &catalog::SAM21_LARGE,
        }
    }

    fn file(self) -> &'static File {
        &self.model().files[0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    pub at: [f64; 2],
    pub positive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub logits: Vec<f32>,
    pub object: f32,
}

pub fn needed(dir: &Path, size: Size) -> Vec<Need> {
    let f = size.file();
    match models::present(dir, f) {
        true => Vec::new(),
        false => vec![Need {
            title: f.title.into(),
            bytes: Some(f.bytes),
            system: false,
        }],
    }
}

pub fn get_ready(dir: &Path, size: Size, download: bool) -> Result<()> {
    let missing = needed(dir, size);
    if missing.is_empty() {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} isn't on this computer yet: pass download (--download) to download it into {}, \
             checked against its SHA-256",
            models::listed(&missing),
            dir.display()
        )));
    }
    models::network();
    models::fetch(&Fetch::of(size.file()), dir)?;
    Ok(())
}

type Held = (Size, bool, net::Net);

static NET: Mutex<Option<Held>> = Mutex::new(None);

fn picture_checked((w, h, pixels): (u32, u32, &[[f32; 3]])) -> Result<()> {
    match w == 0 || h == 0 || pixels.len() != w as usize * h as usize {
        true => Err(Error::Invalid(format!(
            "a subject mask needs {w}x{h} pixels, not {}",
            pixels.len()
        ))),
        false => Ok(()),
    }
}

fn loaded(dir: &Path, size: Size, on_gpu: bool) -> Result<MutexGuard<'static, Option<Held>>> {
    get_ready(dir, size, false)?;
    let mut held = NET.lock().unwrap_or_else(PoisonError::into_inner);
    if held
        .as_ref()
        .is_none_or(|(s, g, _)| *s != size || *g != on_gpu)
    {
        *held = None;
        let path = dir.join(size.file().name);
        let net = net::Net::load(&path, size.model().title, &size.config(), on_gpu)?;
        *held = Some((size, on_gpu, net));
    }
    Ok(held)
}

fn net_of(held: &Option<Held>) -> Result<&net::Net> {
    held.as_ref()
        .map(|(_, _, n)| n)
        .ok_or_else(|| Error::Invalid("no subject mask model".into()))
}

pub fn find(
    dir: &Path,
    (size, on_gpu): (Size, bool),
    picture: (u32, u32, &[[f32; 3]]),
    subjects: &[Vec<Click>],
) -> Result<Vec<Found>> {
    picture_checked(picture)?;
    let mut held = loaded(dir, size, on_gpu)?;
    let found: Result<Vec<Found>> = (|| {
        let net = net_of(&held)?;
        net.encode((picture.0, picture.1), picture.2)?;
        if let Some(dir) = dumped()? {
            net.dump(&dir, None)?;
        }
        subjects.iter().map(|c| net.decode(c)).collect()
    })();
    if found.is_err() {
        *held = None;
    }
    found
}

pub struct Track {
    held: MutexGuard<'static, Option<Held>>,
    work: net::Work,
    banks: Vec<net::Bank>,
    frames: usize,
    step: usize,
}

impl Track {
    pub fn start(
        dir: &Path,
        (size, on_gpu): (Size, bool),
        picture: (u32, u32, &[[f32; 3]]),
        subjects: &[Vec<Click>],
        frames: usize,
    ) -> Result<(Self, Vec<Found>)> {
        picture_checked(picture)?;
        let held = loaded(dir, size, on_gpu)?;
        let net = net_of(&held)?;
        let work = net.work();
        let banks = subjects.iter().map(|_| net.bank()).collect();
        let mut track = Self {
            held,
            work,
            banks,
            frames,
            step: 0,
        };
        let found = track.run(|net, work, banks, step| {
            net.encode((picture.0, picture.1), picture.2)?;
            if let Some(dir) = dumped()? {
                net.dump(&dir, Some(step))?;
            }
            (banks.iter_mut().zip(subjects))
                .map(|(bank, clicks)| net.first(work, bank, clicks))
                .collect()
        })?;
        Ok((track, found))
    }

    pub fn next(&mut self, picture: (u32, u32, &[[f32; 3]])) -> Result<Vec<Found>> {
        picture_checked(picture)?;
        crate::cancel::stop()?;
        let frames = self.frames;
        self.run(|net, work, banks, step| {
            net.encode((picture.0, picture.1), picture.2)?;
            if let Some(dir) = dumped()? {
                net.dump(&dir, Some(step))?;
            }
            (banks.iter_mut())
                .map(|bank| net.next(work, bank, frames))
                .collect()
        })
    }

    fn run(
        &mut self,
        step: impl FnOnce(&net::Net, &net::Work, &mut [net::Bank], usize) -> Result<Vec<Found>>,
    ) -> Result<Vec<Found>> {
        let at = self.step;
        let found = net_of(&self.held).and_then(|net| step(net, &self.work, &mut self.banks, at));
        match found.is_err() {
            true => *self.held = None,
            false => self.step += 1,
        }
        found
    }
}

const PART_BYTES: usize = 256 << 20;

type Picture = (u32, u32, Vec<[f32; 3]>);

fn picture(p: &Picture) -> (u32, u32, &[[f32; 3]]) {
    (p.0, p.1, &p.2[..])
}

fn ended(frame: i64) -> Error {
    Error::Invalid(format!(
        "the picture of the Magic Mask track ends before frame {frame}"
    ))
}

pub struct Run<'a> {
    pub plan: &'a vidcut_core::render::RenderPlan,
    pub source: &'a Path,
    pub at: i64,
    pub backward: bool,
    pub forward: bool,
    pub decoder: Option<&'a crate::hw::GpuDecoder>,
}

pub fn track(
    dir: &Path,
    model: (Size, bool),
    run: &Run,
    subjects: &[Vec<Click>],
    mut each: impl FnMut(i64, Vec<Found>) -> Result<()>,
) -> Result<()> {
    let frames = run.plan.frames.max(0);
    if run.at < 0 || run.at >= frames {
        return Err(Error::Invalid(format!(
            "the Magic Mask track starts on frame {}, outside its {frames} frames",
            run.at
        )));
    }
    let first = Pictures::read(run.plan, (run.at, run.at + 1), run.decoder)?
        .next_picture()?
        .ok_or_else(|| ended(run.at))?;
    let count = frames as usize;
    let ahead = if run.forward { frames - run.at - 1 } else { 0 };
    let behind = if run.backward { run.at } else { 0 };
    let shown = crate::tracking::Shown {
        path: run.source,
        started: std::time::Instant::now(),
        all: (ahead + behind).max(1) as f64,
    };
    let mut done = 0;
    let mut each = |f: i64, found: Vec<Found>| {
        done += 1;
        shown.report(done);
        each(f, found)
    };
    if run.forward && run.at + 1 < frames {
        let (mut track, _) = Track::start(dir, model, picture(&first), subjects, count)?;
        let mut pictures = Pictures::read(run.plan, (run.at + 1, frames), run.decoder)?;
        for f in run.at + 1..frames {
            let p = pictures.next_picture()?.ok_or_else(|| ended(f))?;
            each(f, track.next(picture(&p))?)?;
        }
    }
    if run.backward && run.at > 0 {
        let (mut track, _) = Track::start(dir, model, picture(&first), subjects, count)?;
        let per = (first.0 as usize * first.1 as usize * 12).max(1);
        let most = (PART_BYTES / per).max(1) as i64;
        let mut end = run.at;
        while end > 0 {
            let start = (end - most).max(0);
            let mut pictures = Pictures::read(run.plan, (start, end), run.decoder)?;
            let mut part = Vec::new();
            for f in start..end {
                part.push(pictures.next_picture()?.ok_or_else(|| ended(f))?);
            }
            for (f, p) in (start..end).rev().zip(part.iter().rev()) {
                each(f, track.next(picture(p))?)?;
            }
            end = start;
        }
    }
    Ok(())
}

pub struct Pictures {
    frames: crate::export::video::VideoFrames,
}

impl Pictures {
    pub fn read(
        plan: &vidcut_core::render::RenderPlan,
        (from, to): (i64, i64),
        decoder: Option<&crate::hw::GpuDecoder>,
    ) -> Result<Self> {
        crate::cancel::stop()?;
        crate::init();
        let size = (plan.width, plan.height);
        let colors = (plan.colors, None, None);
        let pixel = crate::ffmpeg::format::Pixel::GBRPF32LE;
        let frames = crate::export::renderer::stream(plan.fps, size, pixel, colors, decoder)?;
        let mut frames = frames.in_frame(size);
        frames.read_pieces(&crate::tracking::pieces(plan, from, to));
        Ok(Self { frames })
    }

    pub fn next_picture(&mut self) -> Result<Option<Picture>> {
        let frame = self.frames.next_frame()?;
        Ok(frame.map(|f| crate::export::still::floats_of(&f)))
    }
}

fn dumped() -> Result<Option<PathBuf>> {
    let Some(dir) = std::env::var_os("VIDCUT_MASK_DUMP") else {
        return Ok(None);
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    Ok(Some(dir))
}

fn dump_index() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
