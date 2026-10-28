use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use vidcut_core::catalog::{self, Measure};
use vidcut_core::model::{Media, Project, Retime};
use vidcut_core::render::{RenderPlan, VideoPiece};

use crate::compute::Gpu;
use crate::error::{Error, Result};
use crate::ffmpeg::frame;
use crate::models::{self, Fetch, Need};
use crate::route::Place;
use crate::screen::held;
use crate::super_scale::Picture;

mod ema;
mod macos;
mod ops;
mod rife;

use ops::{Kernels, Ops, View, tensor};

const WHAT: &str = "frame interpolation";
const SHARED: u32 = 16 * 1024;
const UHD: (u32, u32) = (3840, 2160);

fn model(r: Retime) -> Option<&'static catalog::Model> {
    match r {
        Retime::AiFast => Some(&catalog::RIFE),
        Retime::AiBest => Some(&catalog::EMA_VFI),
        Retime::Nearest | Retime::FrameBlend | Retime::Macos => None,
    }
}

pub fn name(r: Retime) -> &'static str {
    match r {
        Retime::AiFast => "AI Fast (RIFE 4.25)",
        Retime::AiBest => "AI Best (EMA-VFI)",
        Retime::Macos => "AI (macOS frame rate conversion)",
        Retime::Nearest | Retime::FrameBlend => r.title(),
    }
}

#[cfg(target_os = "macos")]
pub fn off_here(r: Retime) -> bool {
    matches!(r, Retime::AiFast | Retime::AiBest)
}

#[cfg(not(target_os = "macos"))]
pub fn off_here(r: Retime) -> bool {
    r == Retime::Macos
}

pub fn of(id: &str) -> Option<Retime> {
    Retime::ALL
        .into_iter()
        .find(|r| model(*r).is_some_and(|m| m.id == id))
}

fn files(r: Retime) -> &'static [catalog::File] {
    model(r).map_or(&[], |m| m.files)
}

pub fn needed(dir: &Path, r: Retime) -> Vec<Need> {
    files(r)
        .iter()
        .filter(|f| !models::present(dir, f))
        .map(|f| Need {
            title: f.title.into(),
            bytes: Some(f.bytes),
            system: false,
        })
        .collect()
}

pub fn get_ready(dir: &Path, r: Retime, download: bool) -> Result<()> {
    if r == Retime::Macos {
        return macos::ready();
    }
    let missing = needed(dir, r);
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
    for f in files(r).iter().filter(|f| !models::present(dir, f)) {
        models::fetch(&Fetch::of(f), dir)?;
    }
    Ok(())
}

pub fn inspect(dir: &Path, r: Retime, download: bool) -> Result<()> {
    get_ready(dir, r, download)?;
    if r == Retime::Macos {
        return Ok(());
    }
    models::verify(dir, files(r))?;
    with_engine(|e| e.net(r).map(drop))
}

pub fn unfit(m: &Media, r: Retime) -> Option<&'static str> {
    if r.is_nearest() {
        return None;
    }
    let hdr = m.video_as_read()?.hdr.is_some();
    let refused = match model(r) {
        Some(model) => !catalog::check(model, &[(Measure::HdrPictures, f64::from(u8::from(hdr)))])
            .refusals
            .is_empty(),
        None => hdr,
    };
    refused.then_some("HDR pictures")
}

pub fn check<'a>(
    p: &Project,
    plans: impl IntoIterator<Item = &'a RenderPlan>,
    dir: &Path,
    download: bool,
) -> Result<()> {
    let mut used: Vec<(Retime, String)> = Vec::new();
    for plan in plans {
        for piece in &plan.video {
            for (layer, _) in piece.layers() {
                if let VideoPiece::Source { media, retime, .. } = layer
                    && !retime.is_nearest()
                    && !used.iter().any(|(r, m)| r == retime && m == media)
                {
                    used.push((*retime, media.clone()));
                }
            }
        }
    }
    let mut off: Vec<String> = used
        .iter()
        .filter(|(r, _)| off_here(*r))
        .filter_map(|(_, id)| p.media(id).map(|m| m.name.clone()))
        .collect();
    off.sort();
    off.dedup();
    if !off.is_empty() {
        #[cfg(target_os = "macos")]
        let (which, instead) = ("AI Fast and AI Best are off on macOS for now", "AI");
        #[cfg(not(target_os = "macos"))]
        let (which, instead) = ("AI of macOS is on macOS only", "AI Fast or AI Best");
        return Err(Error::Invalid(format!(
            "{which}: set the Retime Process of {} to {instead}",
            off.join(", ")
        )));
    }
    let refused: Vec<String> = used
        .iter()
        .filter_map(|(r, id)| {
            let m = p.media(id)?;
            unfit(m, *r).map(|why| format!("{} ({why}, {})", m.name, r.title()))
        })
        .collect();
    if !refused.is_empty() {
        return Err(Error::Invalid(format!(
            "Retime Process takes SDR pictures for now, so it can't take {}: set their clips' \
             Retime Process to Nearest",
            refused.join(", ")
        )));
    }
    let mut models: Vec<Retime> = used.iter().map(|(r, _)| *r).filter(Retime::ai).collect();
    models.sort_by_key(|r| *r as u8);
    models.dedup();
    for r in models {
        get_ready(dir, r, download)?;
    }
    Ok(())
}

enum Net {
    Rife(Box<rife::Rife>),
    Ema(Box<ema::Ema>),
}

enum State {
    Rife(Box<rife::Pair>),
    Ema(Box<ema::Pair>),
    Blend(View, View),
}

struct Held {
    key: (PathBuf, u64, i64, i64),
    retime: Retime,
    size: (u32, u32),
    state: State,
    out: wgpu::Buffer,
}

struct Engine {
    gpu: Gpu,
    kernels: Kernels,
    nets: Vec<(Retime, Net)>,
    held: Option<Held>,
}

static ENGINE: Mutex<Option<std::result::Result<Engine, String>>> = Mutex::new(None);
static USERS: Mutex<usize> = Mutex::new(0);
static DUMPED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    pub retime: Retime,
    pub size: (u32, u32),
    pub at: f64,
    pub took: Option<Duration>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub runs_on: Option<String>,
    pub made: Option<Made>,
}

static REPORT: Mutex<Report> = Mutex::new(Report {
    runs_on: None,
    made: None,
});

fn report_lock() -> MutexGuard<'static, Report> {
    REPORT.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn report() -> Report {
    report_lock().clone()
}

pub(crate) fn runs_on(device: String) {
    report_lock().runs_on = Some(device);
}

pub(crate) fn kept_at_cut(retime: Retime, size: (u32, u32), at: f64) {
    report_lock().made = Some(Made {
        retime,
        size,
        at,
        took: None,
    });
}

fn engine() -> MutexGuard<'static, Option<std::result::Result<Engine, String>>> {
    ENGINE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn with_engine<T>(run: impl FnOnce(&mut Engine) -> Result<T>) -> Result<T> {
    let mut slot = engine();
    let e = match slot.get_or_insert_with(Engine::start) {
        Ok(e) => e,
        Err(why) => return Err(Error::Invalid(why.clone())),
    };
    let device = e.gpu.device.clone();
    crate::compute::checked(&device, WHAT, || run(e))
}

pub(crate) struct User;

impl User {
    pub(crate) fn of(r: Retime) -> Option<Self> {
        if r.is_nearest() {
            return None;
        }
        *USERS.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Some(Self)
    }
}

impl Drop for User {
    fn drop(&mut self) {
        let last = {
            let mut n = USERS.lock().unwrap_or_else(PoisonError::into_inner);
            *n = n.saturating_sub(1);
            *n == 0
        };
        if last {
            *engine() = None;
            macos::release();
        }
    }
}

impl Engine {
    fn start() -> std::result::Result<Self, String> {
        let gpu = Gpu::start(WHAT, SHARED)?;
        runs_on(gpu.name.clone());
        let kernels = crate::compute::checked(&gpu.device, WHAT, || Ok(Kernels::new(&gpu)))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            gpu,
            kernels,
            nets: Vec::new(),
            held: None,
        })
    }

    fn loaded(&self, r: Retime) -> Option<&Net> {
        self.nets.iter().find(|(k, _)| *k == r).map(|(_, n)| n)
    }

    fn net(&mut self, r: Retime) -> Result<&Net> {
        if self.loaded(r).is_none() {
            let dir = vidcut_core::dirs::models();
            get_ready(&dir, r, false)?;
            let m =
                model(r).ok_or_else(|| Error::Invalid(format!("{} has no model", r.title())))?;
            let path = dir.join(m.files[0].name);
            let net = match r {
                Retime::AiFast => Net::Rife(Box::new(rife::load(&self.gpu, &path, m.title)?)),
                _ => Net::Ema(Box::new(ema::load(&self.gpu, &path, m.title)?)),
            };
            self.nets.push((r, net));
        }
        Ok(self.loaded(r).expect("loaded"))
    }

    fn submit(&self, run: impl FnOnce(&mut Ops)) {
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            let mut o = Ops {
                gpu: &self.gpu,
                k: &self.kernels,
                pass: &mut pass,
            };
            run(&mut o);
        }
        self.gpu.flush();
        self.gpu.queue.submit([enc.finish()]);
    }

    fn hold(
        &mut self,
        r: Retime,
        key: &(PathBuf, u64, i64, i64),
        (ua, ub): (&wgpu::Buffer, &wgpu::Buffer),
        size: (u32, u32),
    ) -> Result<()> {
        if self
            .held
            .as_ref()
            .is_some_and(|h| h.key == *key && h.retime == r && h.size == size)
        {
            return Ok(());
        }
        let kept = self.held.take().filter(|h| h.retime == r && h.size == size);
        if r.ai() {
            self.net(r)?;
        }
        let (w, h) = size;
        let whole = u64::from(w) * u64::from(h) * 3;
        if ua.size() < whole || ub.size() < whole {
            return Err(Error::Invalid(format!(
                "{} got pictures smaller than {w}x{h}",
                r.title()
            )));
        }
        let gpu = &self.gpu;
        let (state, out) = match kept {
            Some(h) => (h.state, h.out),
            None => {
                let out = gpu.buffer((w as usize * h as usize * 3).next_multiple_of(4));
                let state = match (r, self.loaded(r)) {
                    (Retime::FrameBlend, _) => {
                        State::Blend(tensor(gpu, 2, h, w, 3), tensor(gpu, 1, h, w, 3))
                    }
                    (_, Some(Net::Rife(net))) => {
                        let scale = if w >= UHD.0 || h >= UHD.1 { 0.5 } else { 1.0 };
                        State::Rife(Box::new(net.pair(gpu, rife::padded(size, scale), scale)))
                    }
                    (_, Some(Net::Ema(_))) => State::Ema(Box::new(ema::Pair::new(gpu, size)?)),
                    _ => return Err(Error::Invalid(format!("{} has no model", r.title()))),
                };
                (state, out)
            }
        };
        self.submit(|o| match (&state, self.loaded(r)) {
            (State::Blend(img, _), _) => {
                o.load(ua, size, (0, 0), false, &img.frame(0));
                o.load(ub, size, (0, 0), false, &img.frame(1));
            }
            (State::Rife(pair), Some(Net::Rife(net))) => {
                o.load(ua, size, (0, 0), false, &pair.img.frame(0));
                o.load(ub, size, (0, 0), false, &pair.img.frame(1));
                net.features(o, pair);
            }
            (State::Ema(pair), Some(Net::Ema(net))) => {
                pair.load(o, (ua, ub), size);
                pair.backbone(o, net);
            }
            _ => {}
        });
        self.held = Some(Held {
            key: key.clone(),
            retime: r,
            size,
            state,
            out,
        });
        Ok(())
    }

    fn frame(&self, r: Retime, t: f32) -> Result<held::Held> {
        let held = self.held.as_ref().expect("held");
        let size = held.size;
        self.submit(|o| match (&held.state, self.loaded(r)) {
            (State::Blend(img, mixed), _) => {
                o.combine(&img.frame(0), &img.frame(1), mixed, 1.0 - t, t);
                o.store(mixed, (0, 0), size, &held.out);
            }
            (State::Rife(pair), Some(Net::Rife(net))) => {
                net.frame(o, pair, t);
                net.finish(o, pair, size, &held.out);
            }
            (State::Ema(pair), Some(Net::Ema(net))) => {
                pair.frame(o, net, t, size, &held.out);
            }
            _ => {}
        });
        let whole = (size.0 as u64 * size.1 as u64 * 3).next_multiple_of(4);
        let out = self.gpu.buffer(whole as usize);
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&held.out, 0, &out, 0, whole);
        self.gpu.queue.submit([enc.finish()]);
        Ok(held::Held::new(out, &self.gpu.device, &self.gpu.queue))
    }
}

fn dump(name: &str, size: (u32, u32), rgb: &[u8]) -> Result<()> {
    let Some(dir) = std::env::var_os("VIDCUT_INTERPOLATE_DUMP") else {
        return Ok(());
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    crate::npy::write_bytes(
        &dir.join(format!("{name}.npy")),
        &[size.1 as usize, size.0 as usize, 3],
        rgb,
    )
}

pub(crate) fn takes_frames(r: Retime, a: &frame::Video, b: &frame::Video) -> bool {
    r == Retime::Macos && Place::of(a) == Place::VideoToolbox && Place::of(b) == Place::VideoToolbox
}

pub(crate) fn between_frames(
    key: &(PathBuf, u64, i64, i64),
    a: &frame::Video,
    b: &frame::Video,
    size: (u32, u32),
    t: f64,
) -> Result<frame::Video> {
    let clock = Instant::now();
    let made = macos::between_frames(key, a, b, size, t)?;
    made_in(Retime::Macos, size, t, clock);
    Ok(made)
}

fn made_in(retime: Retime, size: (u32, u32), at: f64, clock: Instant) {
    report_lock().made = Some(Made {
        retime,
        size,
        at,
        took: Some(clock.elapsed()),
    });
}

pub(crate) fn between(
    r: Retime,
    key: &(PathBuf, u64, i64, i64),
    a: &frame::Video,
    b: &frame::Video,
    size: (u32, u32),
    t: f64,
) -> Result<Picture> {
    let clock = Instant::now();
    let made = if r == Retime::Macos {
        let (a, b) = (held::rgb(a)?, held::rgb(b)?);
        Picture::Rgb(macos::between(
            key,
            macos::Input::Rgb(&a),
            macos::Input::Rgb(&b),
            size,
            t,
        )?)
    } else {
        let pair = (held::held(a)?.buffer(), held::held(b)?.buffer());
        Picture::Held(with_engine(|e| {
            e.hold(r, key, pair, size)?;
            e.frame(r, t as f32)
        })?)
    };
    made_in(r, size, t, clock);
    if std::env::var_os("VIDCUT_INTERPOLATE_DUMP").is_some() {
        let n = DUMPED.fetch_add(1, Ordering::Relaxed);
        let name = format!("{n:04}");
        dump(&format!("{name}-a"), size, &held::rgb(a)?)?;
        dump(&format!("{name}-b"), size, &held::rgb(b)?)?;
        let out = match &made {
            Picture::Held(h) => h.read(size.0 as usize * size.1 as usize * 3)?,
            Picture::Rgb(rgb) => rgb.clone(),
            Picture::Frame(_) => Vec::new(),
        };
        dump(
            &format!("{name}-out-{t:.4}-{}", r.title().replace(' ', "")),
            size,
            &out,
        )?;
    }
    Ok(made)
}
