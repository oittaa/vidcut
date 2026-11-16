use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use vidcut_core::analysis::{Detections, Found, Look};
use vidcut_core::catalog;
use vidcut_core::model::{FindModel, Subject};

use crate::decode::{Input, Reading, VideoReader};
use crate::error::{Context, Error, Result};
use crate::export::Progress;
use crate::ffmpeg::{format, frame, software::scaling};
use crate::models::{self, Fetch, Need};

pub mod ppocr;
pub mod retinaface;
pub mod yolox;
pub mod yunet;

const STEP: u32 = 32;

fn padded(side: u32) -> u32 {
    side.div_ceil(STEP) * STEP
}

fn bad(path: &Path, what: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{}: {what}", path.display()))
}

fn overlap(a: &Found, b: &Found) -> f32 {
    let side = |i: usize| {
        let (a0, a1) = (a.center[i] - a.size[i] / 2.0, a.center[i] + a.size[i] / 2.0);
        let (b0, b1) = (b.center[i] - b.size[i] / 2.0, b.center[i] + b.size[i] / 2.0);
        (a1.min(b1) - a0.max(b0)).max(0.0)
    };
    let both = side(0) * side(1);
    let all = a.size[0] * a.size[1] + b.size[0] * b.size[1] - both;
    if all > 0.0 { both / all } else { 0.0 }
}

fn apart(mut found: Vec<Found>, same: f32) -> Vec<Found> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Found> = Vec::new();
    for f in found {
        if kept.iter().all(|k| overlap(k, &f) <= same) {
            kept.push(f);
        }
    }
    kept
}

fn slow(subject: Subject, model: FindModel) -> bool {
    subject == Subject::Face && model == FindModel::Slow
}

fn listed(subject: Subject, model: FindModel) -> &'static catalog::Model {
    match subject {
        Subject::Face if slow(subject, model) => &catalog::RETINAFACE,
        Subject::Face => &catalog::YUNET,
        Subject::Text => &catalog::PPOCR_DET,
        Subject::Person | Subject::Vehicle => &catalog::YOLOX,
    }
}

fn file(subject: Subject, model: FindModel) -> catalog::File {
    match subject {
        Subject::Face if slow(subject, model) => retinaface::FILE,
        Subject::Face => yunet::FILE,
        Subject::Text => ppocr::FILE,
        Subject::Person | Subject::Vehicle => yolox::FILE,
    }
}

pub fn side(subject: Subject, model: FindModel) -> u32 {
    match subject {
        Subject::Face if slow(subject, model) => retinaface::SIDE,
        Subject::Face => yunet::SIDE,
        Subject::Text => ppocr::SIDE,
        Subject::Person | Subject::Vehicle => yolox::SIDE,
    }
}

fn rules(subject: Subject, model: FindModel) -> String {
    match subject {
        Subject::Face if slow(subject, model) => retinaface::name(),
        Subject::Face => yunet::name(),
        Subject::Text => ppocr::name(),
        Subject::Person => yolox::name(yolox::PEOPLE),
        Subject::Vehicle => yolox::name(yolox::VEHICLES),
    }
}

fn takes(subject: Subject, model: FindModel) -> Result<()> {
    if model.finds(subject) {
        return Ok(());
    }
    Err(Error::Invalid(format!(
        "the {} model finds only faces, not {}",
        model.name(),
        subject.name()
    )))
}

enum Net {
    Face(yunet::Model),
    Faces(Box<retinaface::Model>),
    Text(ppocr::Model),
    Things(yolox::Model),
}

fn each(
    pictures: &[Vec<f32>],
    find: &(dyn Fn(&[f32]) -> Result<Vec<Found>> + Sync),
) -> Result<Vec<Vec<Found>>> {
    let slots: Vec<Mutex<Option<Result<Vec<Found>>>>> =
        pictures.iter().map(|_| Mutex::new(None)).collect();
    crate::pool::run(pictures.len(), &|i| {
        *crate::pool::lock(&slots[i]) = Some(find(&pictures[i]));
    });
    slots
        .into_iter()
        .map(|slot| {
            let found = slot.into_inner().unwrap_or_else(|e| e.into_inner());
            found.unwrap_or_else(|| Ok(Vec::new()))
        })
        .collect()
}

impl Net {
    fn load(subject: Subject, model: FindModel, path: &Path, size: (u32, u32)) -> Result<Self> {
        Ok(match subject {
            Subject::Face if slow(subject, model) => {
                Self::Faces(Box::new(retinaface::Model::load(path, size)?))
            }
            Subject::Face => Self::Face(yunet::Model::load(path, size)?),
            Subject::Text => Self::Text(ppocr::Model::load(path, size)?),
            Subject::Person => Self::Things(yolox::Model::load(path, yolox::PEOPLE, size)?),
            Subject::Vehicle => Self::Things(yolox::Model::load(path, yolox::VEHICLES, size)?),
        })
    }

    fn size(&self) -> (usize, usize) {
        match self {
            Self::Face(m) => m.size(),
            Self::Faces(m) => m.size(),
            Self::Text(m) => m.size(),
            Self::Things(m) => m.size(),
        }
    }

    fn probe(&self, dir: &Path, pictures: &[Vec<f32>]) -> Result<()> {
        match self {
            Self::Faces(m) => m.probe(dir, pictures),
            _ => Ok(()),
        }
    }

    fn found(&self, pictures: &[Vec<f32>], content: (usize, usize)) -> Result<Vec<Vec<Found>>> {
        match self {
            Self::Face(m) => each(pictures, &|p| m.find(p)),
            Self::Faces(m) => m.find(pictures),
            Self::Text(m) => each(pictures, &|p| m.find(p, content)),
            Self::Things(m) => each(pictures, &|p| m.find(p, content)),
        }
    }
}

pub fn of(id: &str) -> Option<(Subject, FindModel)> {
    let pairs = Subject::ALL
        .into_iter()
        .flat_map(|s| FindModel::ALL.map(|m| (s, m)));
    pairs
        .filter(|&(s, m)| m.finds(s))
        .find(|&(s, m)| listed(s, m).id == id)
}

pub fn name(subject: Subject, model: FindModel) -> String {
    let weights: String = file(subject, model).sha256.chars().take(8).collect();
    format!("{}-{weights}-{}", subject.name(), rules(subject, model))
}

pub fn needed(dir: &Path, subject: Subject, model: FindModel) -> Vec<Need> {
    let file = file(subject, model);
    if models::present(dir, &file) {
        return Vec::new();
    }
    vec![Need {
        title: file.title.into(),
        bytes: Some(file.bytes),
        system: false,
    }]
}

pub fn get_ready(dir: &Path, subject: Subject, model: FindModel, download: bool) -> Result<()> {
    takes(subject, model)?;
    let file = file(subject, model);
    if models::present(dir, &file) {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} ({}) isn't on this computer yet: pass download (--download) to download it into \
             {}, checked against its SHA-256",
            file.title,
            models::size(file.bytes),
            dir.display()
        )));
    }
    models::network();
    models::fetch(&Fetch::of(&file), dir)?;
    Ok(())
}

pub fn inspect(dir: &Path, subject: Subject, model: FindModel, download: bool) -> Result<()> {
    get_ready(dir, subject, model, download)?;
    let file = file(subject, model);
    models::verify(dir, &[file])?;
    let side = side(subject, model);
    Net::load(subject, model, &dir.join(file.name), (side, side)).map(drop)
}

pub(crate) fn fitted(decoded: &frame::Video, turn: u16, most: u32) -> (u32, u32) {
    let sar = decoded.aspect_ratio();
    let wide = match (sar.numerator(), sar.denominator()) {
        (n, d) if n > 0 && d > 0 => f64::from(n) / f64::from(d),
        _ => 1.0,
    };
    let (w, h) = (
        f64::from(decoded.width()) * wide,
        f64::from(decoded.height()),
    );
    let (w, h) = if turn % 180 == 90 { (h, w) } else { (w, h) };
    let scale = f64::from(most) / w.max(h).max(1.0);
    let side = |v: f64| ((v * scale).round() as u32).clamp(2, most);
    (side(w), side(h))
}

pub(crate) fn small(
    scaler: &mut Option<scaling::Context>,
    decoded: &frame::Video,
    (w, h): (u32, u32),
) -> Result<frame::Video> {
    let down;
    let decoded = if crate::hw::on_gpu(decoded) {
        down = crate::hw::to_read(decoded)?;
        &down
    } else {
        decoded
    };
    let same = |s: &scaling::Context| {
        let from = s.input();
        (from.format, from.width, from.height)
            == (decoded.format(), decoded.width(), decoded.height())
    };
    let scaler = match scaler {
        Some(s) if same(s) => s,
        _ => scaler.insert(
            scaling::Context::get(
                decoded.format(),
                decoded.width(),
                decoded.height(),
                format::Pixel::BGR24,
                w,
                h,
                scaling::Flags::AREA,
            )
            .ctx(|| "creating the scaler of the pictures to look at".into())?,
        ),
    };
    let mut out = frame::Video::empty();
    scaler
        .run(decoded, &mut out)
        .ctx(|| "scaling a picture to look at".into())?;
    Ok(out)
}

fn planes(
    bgr: &frame::Video,
    turn: u16,
    (cw, ch): (usize, usize),
    model: (usize, usize),
) -> Vec<f32> {
    let (pw, ph) = (bgr.width() as usize, bgr.height() as usize);
    let (mw, mh) = model;
    let (stride, data) = (bgr.stride(0), bgr.data(0));
    let mut out = vec![0.0; 3 * mw * mh];
    for y in 0..ph {
        let row = &data[y * stride..y * stride + pw * 3];
        for (x, pixel) in row.as_chunks::<3>().0.iter().enumerate() {
            let (ux, uy) = match turn {
                90 => (ph - 1 - y, x),
                180 => (pw - 1 - x, ph - 1 - y),
                270 => (y, pw - 1 - x),
                _ => (x, y),
            };
            if ux < cw && uy < ch {
                for (c, v) in pixel.iter().enumerate() {
                    out[c * mw * mh + uy * mw + ux] = f32::from(*v);
                }
            }
        }
    }
    out
}

struct Looking {
    model: Net,
    content: (u32, u32),
    turn: u16,
    scaler: Option<scaling::Context>,
    measure: Option<scaling::Context>,
    before: Option<(Vec<u8>, f64)>,
}

impl Looking {
    fn start(
        (subject, model): (Subject, FindModel),
        path: &Path,
        (first, turn): (&frame::Video, u16),
        wanted: Option<u32>,
    ) -> Result<Self> {
        let source = first.width().max(first.height());
        let most = wanted.map_or(side(subject, model), |s| s.min(source));
        let content = fitted(first, turn, most);
        Ok(Self {
            model: Net::load(subject, model, path, content)?,
            content,
            turn,
            scaler: None,
            measure: None,
            before: None,
        })
    }

    fn picture(&mut self, decoded: &frame::Video) -> Result<(Vec<f32>, f32)> {
        let (cw, ch) = self.content;
        let scaled = if self.turn % 180 == 90 {
            (ch, cw)
        } else {
            (cw, ch)
        };
        let bgr = small(&mut self.scaler, decoded, scaled)?;
        let luma = crate::analysis::small_luma(&mut self.measure, &bgr)?;
        let (scene, moved) = match &self.before {
            Some((last, moved)) => {
                let now = crate::analysis::mafd(last, &luma);
                (crate::analysis::scene_score(now, *moved), now)
            }
            None => (0.0, 0.0),
        };
        self.before = Some((luma, moved));
        let content = (cw as usize, ch as usize);
        let planes = planes(&bgr, self.turn, content, self.model.size());
        Ok((planes, scene))
    }

    fn found(&self, pictures: &[Vec<f32>]) -> Result<Vec<Vec<Found>>> {
        let content = (self.content.0 as usize, self.content.1 as usize);
        let (cw, ch) = (self.content.0 as f32, self.content.1 as f32);
        let share = |f: Found| Found {
            center: [f.center[0] / cw, f.center[1] / ch],
            size: [f.size[0] / cw, f.size[1] / ch],
            ..f
        };
        let found = self.model.found(pictures, content)?;
        Ok(found
            .into_iter()
            .map(|f| f.into_iter().map(share).collect())
            .collect())
    }
}

fn dumped() -> Result<Option<PathBuf>> {
    let Some(dir) = std::env::var_os("VIDCUT_SUBJECTS_DUMP") else {
        return Ok(None);
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    Ok(Some(dir))
}

fn dump_picture(dir: &Path, at: usize, planes: &[f32], (w, h): (usize, usize)) -> Result<()> {
    let bytes: Vec<u8> = planes.iter().map(|&v| v as u8).collect();
    crate::npy::write_bytes(&dir.join(format!("{at:05}.npy")), &[3, h, w], &bytes)
}

fn dump_boxes(dir: &Path, found: &[Vec<Found>], (w, h): (u32, u32)) -> Result<()> {
    let rows: Vec<f32> = found
        .iter()
        .enumerate()
        .flat_map(|(i, boxes)| {
            boxes.iter().flat_map(move |b| {
                [
                    i as f32,
                    b.center[0],
                    b.center[1],
                    b.size[0],
                    b.size[1],
                    b.score,
                ]
            })
        })
        .collect();
    crate::npy::write(&dir.join("boxes.npy"), &[rows.len() / 6, 6], &rows)?;
    crate::npy::write(&dir.join("content.npy"), &[2], &[w as f32, h as f32])
}

pub fn find(
    source: &Input,
    subject: Subject,
    look: Look,
    gpu: Option<&crate::hw::GpuDecoder>,
    dir: &Path,
) -> Result<Detections> {
    crate::cancel::stop()?;
    get_ready(dir, subject, look.model, false)?;
    let weights = dir.join(file(subject, look.model).name);
    let _turn = crate::turns::cpu_analysis()?;
    let mut reader = VideoReader::open_as(source, gpu, Reading::Stream)?;
    let tick = f64::from(reader.time_base());
    let step = reader
        .frame_rate()
        .map_or(0.04, |r| 1.0 / f64::from(r).max(1.0));
    let every = look.every.max(1e-9);
    let seed = crate::analysis::SEED;
    reader.seek(((look.from - seed as f64 * every) / tick).floor() as i64)?;
    let turn = reader.rotation();
    let dump = dumped()?;
    let started = Instant::now();
    let batch = crate::pool::threads().max(1);
    let mut out = Detections::default();
    let mut looking: Option<Looking> = None;
    let mut waiting: Vec<Vec<f32>> = Vec::new();
    let (mut ended, mut last, mut wanted) = (false, None, -seed);
    let (mut frames, mut in_model) = (0u64, Duration::ZERO);
    while !ended {
        crate::cancel::stop()?;
        match reader.next_frame()? {
            Some(decoded) => {
                frames += 1;
                let pts = decoded.pts().map(|p| p as f64 * tick);
                let pts = pts.unwrap_or_else(|| last.map_or(look.from, |p: f64| p + step));
                last = Some(pts);
                ended = pts >= look.to;
                let due = look.from + wanted as f64 * every;
                if pts >= due {
                    let seeing = match &mut looking {
                        Some(seeing) => seeing,
                        None => {
                            let seeing = Looking::start(
                                (subject, look.model),
                                &weights,
                                (&decoded, turn),
                                look.side,
                            )?;
                            looking.insert(seeing)
                        }
                    };
                    let (picture, scene) = seeing.picture(&decoded)?;
                    wanted = wanted.max(((pts - look.from) / every).floor() as i64) + 1;
                    if wanted > 0 {
                        if let Some(dir) = &dump {
                            dump_picture(dir, out.pts.len(), &picture, seeing.model.size())?;
                        }
                        waiting.push(picture);
                        out.pts.push(pts);
                        out.scene.push(scene);
                    }
                }
            }
            None => ended = true,
        }
        if let Some(seeing) = &looking
            && (waiting.len() >= batch || ended)
            && !waiting.is_empty()
        {
            if let Some(dir) = &dump
                && out.found.is_empty()
            {
                seeing.model.probe(dir, &waiting)?;
            }
            let asked = Instant::now();
            out.found.extend(seeing.found(&waiting)?);
            in_model += asked.elapsed();
            waiting.clear();
            let length = (look.to - look.from).max(step);
            let share = last.map_or(1.0, |p| (p - look.from) / length);
            let seen = Progress::measuring(share, started.elapsed());
            crate::progress::report(source.path(), seen);
        }
    }
    if let (Some(dir), Some(seeing)) = (&dump, &looking) {
        dump_boxes(dir, &out.found, seeing.content)?;
    }
    crate::debug!(
        "subjects",
        "{}: {} pictures of {frames} frames in {:.2} s, {:.2} s of it in the model on {batch} threads",
        source.path().display(),
        out.pts.len(),
        started.elapsed().as_secs_f64(),
        in_model.as_secs_f64()
    );
    Ok(out)
}
