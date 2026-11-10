use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use vidcut_core::analysis::{Detections, Found, Look};
use vidcut_core::catalog;
use vidcut_core::model::Subject;

use crate::decode::{Input, Reading, VideoReader};
use crate::error::{Context, Error, Result};
use crate::export::Progress;
use crate::ffmpeg::{format, frame, software::scaling};
use crate::models::{self, Fetch, Need};

pub mod ppocr;
pub mod yunet;

const STEP: u32 = 32;

fn padded(side: u32) -> u32 {
    side.div_ceil(STEP) * STEP
}

fn model(subject: Subject) -> &'static catalog::Model {
    match subject {
        Subject::Face => &catalog::YUNET,
        Subject::Text => &catalog::PPOCR_DET,
    }
}

fn file(subject: Subject) -> catalog::File {
    match subject {
        Subject::Face => yunet::FILE,
        Subject::Text => ppocr::FILE,
    }
}

fn side(subject: Subject) -> u32 {
    match subject {
        Subject::Face => yunet::SIDE,
        Subject::Text => ppocr::SIDE,
    }
}

fn rules(subject: Subject) -> String {
    match subject {
        Subject::Face => yunet::name(),
        Subject::Text => ppocr::name(),
    }
}

enum Net {
    Face(yunet::Model),
    Text(ppocr::Model),
}

impl Net {
    fn load(subject: Subject, path: &Path, size: (u32, u32)) -> Result<Self> {
        Ok(match subject {
            Subject::Face => Self::Face(yunet::Model::load(path, size)?),
            Subject::Text => Self::Text(ppocr::Model::load(path, size)?),
        })
    }

    fn size(&self) -> (usize, usize) {
        match self {
            Self::Face(m) => m.size(),
            Self::Text(m) => m.size(),
        }
    }

    fn find(&self, picture: &[f32], content: (usize, usize)) -> Result<Vec<Found>> {
        match self {
            Self::Face(m) => m.find(picture),
            Self::Text(m) => m.find(picture, content),
        }
    }
}

pub fn of(id: &str) -> Option<Subject> {
    Subject::ALL.into_iter().find(|s| model(*s).id == id)
}

pub fn name(subject: Subject) -> String {
    format!("{}-{}", subject.name(), rules(subject))
}

pub fn needed(dir: &Path, subject: Subject) -> Vec<Need> {
    let file = file(subject);
    if models::present(dir, &file) {
        return Vec::new();
    }
    vec![Need {
        title: file.title.into(),
        bytes: Some(file.bytes),
        system: false,
    }]
}

pub fn get_ready(dir: &Path, subject: Subject, download: bool) -> Result<()> {
    let file = file(subject);
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

pub fn inspect(dir: &Path, subject: Subject, download: bool) -> Result<()> {
    get_ready(dir, subject, download)?;
    models::verify(dir, &[file(subject)])?;
    let side = (side(subject), side(subject));
    Net::load(subject, &dir.join(file(subject).name), side).map(drop)
}

fn fitted(decoded: &frame::Video, turn: u16, most: u32) -> (u32, u32) {
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

fn small(
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
    fn start(subject: Subject, path: &Path, first: &frame::Video, turn: u16) -> Result<Self> {
        let content = fitted(first, turn, side(subject));
        Ok(Self {
            model: Net::load(subject, path, content)?,
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
        let slots: Vec<Mutex<Option<Result<Vec<Found>>>>> =
            pictures.iter().map(|_| Mutex::new(None)).collect();
        let model = &self.model;
        let content = (self.content.0 as usize, self.content.1 as usize);
        crate::pool::run(pictures.len(), &|i| {
            let found = model.find(&pictures[i], content);
            *crate::pool::lock(&slots[i]) = Some(found);
        });
        let (cw, ch) = (self.content.0 as f32, self.content.1 as f32);
        let share = |f: Found| Found {
            center: [f.center[0] / cw, f.center[1] / ch],
            size: [f.size[0] / cw, f.size[1] / ch],
            ..f
        };
        slots
            .into_iter()
            .map(|slot| {
                let found = slot.into_inner().unwrap_or_else(|e| e.into_inner());
                let found = found.unwrap_or_else(|| Ok(Vec::new()))?;
                Ok(found.into_iter().map(share).collect())
            })
            .collect()
    }
}

pub fn find(
    source: &Input,
    subject: Subject,
    look: Look,
    gpu: Option<&crate::hw::GpuDecoder>,
    dir: &Path,
) -> Result<Detections> {
    crate::cancel::stop()?;
    get_ready(dir, subject, false)?;
    let weights = dir.join(file(subject).name);
    let _turn = crate::turns::cpu_analysis()?;
    let mut reader = VideoReader::open_as(source, gpu, Reading::Stream)?;
    let tick = f64::from(reader.time_base());
    let step = reader
        .frame_rate()
        .map_or(0.04, |r| 1.0 / f64::from(r).max(1.0));
    reader.seek((look.from / tick).floor() as i64)?;
    let turn = reader.rotation();
    let started = Instant::now();
    let batch = crate::pool::threads().max(1);
    let mut out = Detections::default();
    let mut looking: Option<Looking> = None;
    let mut waiting: Vec<Vec<f32>> = Vec::new();
    let (mut ended, mut last, mut wanted) = (false, None, 0u64);
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
                let due = look.from + wanted as f64 * look.every;
                if pts >= due {
                    let seeing = match &mut looking {
                        Some(seeing) => seeing,
                        None => {
                            let seeing = Looking::start(subject, &weights, &decoded, turn)?;
                            looking.insert(seeing)
                        }
                    };
                    let (picture, scene) = seeing.picture(&decoded)?;
                    waiting.push(picture);
                    out.pts.push(pts);
                    out.scene.push(scene);
                    wanted = ((pts - look.from) / look.every.max(1e-9)).floor() as u64 + 1;
                }
            }
            None => ended = true,
        }
        if let Some(seeing) = &looking
            && (waiting.len() >= batch || ended)
            && !waiting.is_empty()
        {
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
