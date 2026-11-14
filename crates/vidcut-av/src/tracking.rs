use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tract_onnx::prelude::*;
use vidcut_core::analysis::{Follow, Found, Tracked};
use vidcut_core::render::{RenderPlan, VideoPiece};

use crate::error::{Error, Result};
use crate::export::Progress;
use crate::export::video::VideoFrames;
use crate::ffmpeg::{format, frame};
use crate::models::{self, Fetch, Need};

pub use vidcut_core::catalog::VITTRACK_ONNX as FILE;

const TEMPLATE: usize = 128;
const SEARCH: usize = 256;
const GRID: usize = 16;
const NEAR: f64 = 2.0;
const AROUND: f64 = 4.0;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const SPREAD: [f32; 3] = [0.229, 0.224, 0.225];
const SIDES: (f64, f64) = (320.0, 1280.0);
const EDGE: f64 = 10.0;
const PART_BYTES: usize = 64 << 20;

pub fn name() -> String {
    let weights: String = FILE.sha256.chars().take(8).collect();
    format!("vittrack-{weights}")
}

pub fn needed(dir: &Path) -> Vec<Need> {
    if models::present(dir, &FILE) {
        return Vec::new();
    }
    vec![Need {
        title: FILE.title.into(),
        bytes: Some(FILE.bytes),
        system: false,
    }]
}

pub fn get_ready(dir: &Path, download: bool) -> Result<()> {
    if models::present(dir, &FILE) {
        return Ok(());
    }
    if !download {
        return Err(Error::Missing(format!(
            "{} ({}) isn't on this computer yet: pass download (--download) to download it into \
             {}, checked against its SHA-256",
            FILE.title,
            models::size(FILE.bytes),
            dir.display()
        )));
    }
    models::network();
    models::fetch(&Fetch::of(&FILE), dir)?;
    Ok(())
}

pub fn inspect(dir: &Path, download: bool) -> Result<()> {
    get_ready(dir, download)?;
    models::verify(dir, &[FILE])?;
    Model::load(&dir.join(FILE.name)).map(drop)
}

struct Model {
    plan: Arc<TypedRunnableModel>,
    search_first: bool,
    outputs: [usize; 3],
}

impl Model {
    fn load(path: &Path) -> Result<Self> {
        let bad = |what: String| Error::Invalid(format!("{}: {what}", path.display()));
        let onnx = tract_onnx::onnx()
            .with_ignore_output_shapes(true)
            .with_ignore_value_info(true);
        let proto = onnx
            .proto_model_for_path(path)
            .map_err(|e| bad(e.to_string()))?;
        let graph = proto.graph.iter();
        let names: Vec<&str> = graph
            .flat_map(|g| g.output.iter().map(|o| o.name.as_str()))
            .collect();
        let at = |name: &str| {
            let found = names.iter().position(|n| *n == name);
            found.ok_or_else(|| bad(format!("it has no output {name}")))
        };
        let outputs = [at("output1")?, at("output2")?, at("output3")?];
        let mut model = onnx
            .model_for_proto_model(&proto)
            .map_err(|e| bad(e.to_string()))?;
        let inputs: Vec<String> = model
            .input_outlets()
            .map_err(|e| bad(e.to_string()))?
            .iter()
            .map(|o| model.node(o.node).name.clone())
            .collect();
        let place = |name: &str| {
            let found = inputs.iter().position(|n| n == name);
            found.ok_or_else(|| bad(format!("it has no input {name}")))
        };
        let (template, search) = (place("template")?, place("search")?);
        for (input, side) in [(template, TEMPLATE), (search, SEARCH)] {
            model
                .set_input_fact(input, f32::fact([1, 3, side, side]).into())
                .map_err(|e| bad(e.to_string()))?;
        }
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| bad(e.to_string()))?;
        Ok(Self {
            plan,
            search_first: search < template,
            outputs,
        })
    }

    fn look(&self, template: &[f32], search: &[f32]) -> Result<[Vec<f32>; 3]> {
        let failed = |e: TractError| Error::Invalid(format!("tracking: {e}"));
        let template = Tensor::from_shape(&[1, 3, TEMPLATE, TEMPLATE], template).map_err(failed)?;
        let search = Tensor::from_shape(&[1, 3, SEARCH, SEARCH], search).map_err(failed)?;
        let inputs = if self.search_first {
            tvec!(search.into(), template.into())
        } else {
            tvec!(template.into(), search.into())
        };
        let out = self.plan.run(inputs).map_err(failed)?;
        let flat = |i: usize, len: usize| {
            let view = out[self.outputs[i]]
                .to_plain_array_view::<f32>()
                .map_err(failed)?;
            let values = view.as_slice().filter(|s| s.len() == len);
            let values = values.map(<[f32]>::to_vec);
            values.ok_or_else(|| Error::Invalid("tracking: an unexpected output".into()))
        };
        let cells = GRID * GRID;
        Ok([flat(0, cells)?, flat(1, 2 * cells)?, flat(2, 2 * cells)?])
    }
}

struct Picture {
    size: (usize, usize),
    bgr: Vec<u8>,
}

fn picture(frame: &frame::Video) -> Picture {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let (stride, data) = (frame.stride(0), frame.data(0));
    let mut bgr = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = &data[y * stride..y * stride + w * 3];
        bgr.extend(
            row.as_chunks::<3>()
                .0
                .iter()
                .flat_map(|&[r, g, b]| [b, g, r]),
        );
    }
    Picture { size: (w, h), bgr }
}

fn crop(p: &Picture, middle: [f64; 2], side: f64, to: usize) -> Vec<f32> {
    let (w, h) = (p.size.0 as isize, p.size.1 as isize);
    let at = |x: isize, y: isize, c: usize| {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            f32::from(p.bgr[(y * w + x) as usize * 3 + c])
        }
    };
    let step = side / to as f64;
    let spot = |middle: f64, i: usize| {
        let s = middle - side / 2.0 + (i as f64 + 0.5) * step - 0.5;
        (s.floor() as isize, (s - s.floor()) as f32)
    };
    let mut out = vec![0.0f32; 3 * to * to];
    for j in 0..to {
        let (y, fy) = spot(middle[1], j);
        for i in 0..to {
            let (x, fx) = spot(middle[0], i);
            for c in 0..3 {
                let top = at(x, y, c) * (1.0 - fx) + at(x + 1, y, c) * fx;
                let low = at(x, y + 1, c) * (1.0 - fx) + at(x + 1, y + 1, c) * fx;
                let v = top * (1.0 - fy) + low * fy;
                out[c * to * to + j * to + i] = (v / 255.0 - MEAN[c]) / SPREAD[c];
            }
        }
    }
    out
}

fn window() -> Vec<f32> {
    let one = |i: usize| {
        let turn = std::f32::consts::TAU / (GRID + 1) as f32 * (i + 1) as f32;
        0.5 * (1.0 - turn.cos())
    };
    (0..GRID * GRID)
        .map(|i| one(i / GRID) * one(i % GRID))
        .collect()
}

fn clipped([cx, cy, w, h]: [f64; 4], (pw, ph): (usize, usize)) -> [f64; 4] {
    let (pw, ph) = (pw as f64, ph as f64);
    let (mx, my) = (EDGE.min(pw), EDGE.min(ph));
    let x1 = (cx - w / 2.0).max(0.0).min(pw - mx);
    let y1 = (cy - h / 2.0).max(0.0).min(ph - my);
    let x2 = (cx + w / 2.0).max(mx).min(pw);
    let y2 = (cy + h / 2.0).max(my).min(ph);
    let (w, h) = ((x2 - x1).max(mx), (y2 - y1).max(my));
    [x1 + w / 2.0, y1 + h / 2.0, w, h]
}

fn found([cx, cy, bw, bh]: [f64; 4], (w, h): (usize, usize), score: f32) -> Found {
    let (w, h) = (w as f64, h as f64);
    Found {
        center: [(cx / w) as f32, (cy / h) as f32],
        size: [(bw / w) as f32, (bh / h) as f32],
        rotation: 0.0,
        score,
    }
}

struct Tracker<'a> {
    model: &'a Model,
    template: Vec<f32>,
    window: Vec<f32>,
    last: [f64; 4],
}

impl<'a> Tracker<'a> {
    fn start(model: &'a Model, picture: &Picture, area: [f64; 4]) -> Self {
        let [cx, cy, w, h] = area;
        let side = ((w * h).sqrt() * NEAR).ceil().max(1.0);
        Self {
            model,
            template: crop(picture, [cx, cy], side, TEMPLATE),
            window: window(),
            last: area,
        }
    }

    fn step(&mut self, picture: &Picture) -> Result<Found> {
        let [cx, cy, w, h] = self.last;
        let side = ((w * h).sqrt() * AROUND).ceil().max(1.0);
        let search = crop(picture, [cx, cy], side, SEARCH);
        let [sure, size, offset] = self.model.look(&self.template, &search)?;
        let scores = sure.iter().zip(&self.window).map(|(s, w)| s * w);
        let (best, score) = scores
            .enumerate()
            .fold((0, f32::MIN), |m, (i, v)| if v > m.1 { (i, v) } else { m });
        let cells = GRID * GRID;
        let (col, row) = ((best % GRID) as f32, (best / GRID) as f32);
        let x = f64::from((col + offset[best]) / GRID as f32);
        let y = f64::from((row + offset[cells + best]) / GRID as f32);
        let (w, h) = (f64::from(size[best]), f64::from(size[cells + best]));
        let next = [
            cx - side / 2.0 + x * side,
            cy - side / 2.0 + y * side,
            w * side,
            h * side,
        ];
        self.last = clipped(next, picture.size);
        Ok(found(self.last, picture.size, score.clamp(0.0, 1.0)))
    }
}

fn working((w, h): (u32, u32), area: [f32; 4]) -> (u32, u32) {
    let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let (long, short) = (w.max(h), w.min(h));
    let share = f64::from(area[2] * area[3]).max(1e-9);
    let wanted = (4096.0 * long / short / share).sqrt();
    let most = wanted.clamp(SIDES.0, SIDES.1).min(long.max(SIDES.0));
    let scale = most / long;
    let side = |v: f64| ((v * scale).round() as u32).clamp(2, most as u32);
    (side(w), side(h))
}

fn pieces(plan: &RenderPlan, from: i64, to: i64) -> Vec<VideoPiece> {
    let mut out = Vec::new();
    let mut at = 0;
    for piece in &plan.video {
        let n = piece.frames();
        let (a, b) = (from.max(at), to.min(at + n));
        if a < b {
            out.push(piece.part(a - at, b - a));
        }
        at += n;
    }
    out
}

struct Reader<'a> {
    plan: &'a RenderPlan,
    size: (u32, u32),
    gpu: Option<&'a crate::hw::GpuDecoder>,
}

impl Reader<'_> {
    fn run(&self, from: i64, to: i64) -> Result<VideoFrames> {
        let colors = (self.plan.colors, None);
        let pixel = format::Pixel::RGB24;
        let frames =
            crate::export::renderer::stream(self.plan.fps, self.size, pixel, colors, self.gpu)?;
        let mut frames = frames.in_frame((self.plan.width, self.plan.height));
        frames.read_pieces(&pieces(self.plan, from, to));
        Ok(frames)
    }

    fn part(&self, from: i64, to: i64) -> Result<Option<Vec<Picture>>> {
        let Some(mut run) = going(self.run(from, to))? else {
            return Ok(None);
        };
        let mut part = Vec::new();
        for _ in from..to {
            match going(run.next_frame())? {
                None => return Ok(None),
                Some(None) => return Err(ended(from + part.len() as i64)),
                Some(Some(f)) => part.push(picture(&f)),
            }
        }
        Ok(Some(part))
    }
}

fn going<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Err(_) if crate::cancel::cancelled() => Ok(None),
        r => r.map(Some),
    }
}

fn ended(frame: i64) -> Error {
    Error::Invalid(format!(
        "the picture of the track ends before frame {frame}"
    ))
}

struct Shown<'a> {
    path: &'a Path,
    started: Instant,
    all: f64,
}

impl Shown<'_> {
    fn report(&self, done: usize) {
        let seen = Progress::measuring(done as f64 / self.all, self.started.elapsed());
        crate::progress::report(self.path, seen);
    }
}

pub fn track(
    plan: &RenderPlan,
    follow: &Follow,
    (gpu, dir): (Option<&crate::hw::GpuDecoder>, &Path),
    source: &Path,
) -> Result<Tracked> {
    crate::cancel::stop()?;
    get_ready(dir, false)?;
    let model = Model::load(&dir.join(FILE.name))?;
    let _turn = crate::turns::cpu_analysis()?;
    let size = working((plan.width, plan.height), follow.area);
    let reader = Reader { plan, size, gpu };
    let (lo, hi) = (follow.frames.0.max(0), follow.frames.1.min(plan.frames));
    if hi <= lo {
        return Err(Error::Invalid("the track has no frame".into()));
    }
    let at = follow.at.clamp(lo, hi - 1);
    let Some(first) = reader.part(at, at + 1)?.and_then(|mut p| p.pop()) else {
        crate::cancel::stop()?;
        return Err(ended(at));
    };
    let size = first.size;
    let (w, h) = (size.0 as f64, size.1 as f64);
    let [cx, cy, bw, bh] = follow.area.map(f64::from);
    let area = clipped([cx * w, cy * h, bw * w, bh * h], size);
    let ahead = if follow.forward { hi - at - 1 } else { 0 };
    let behind = if follow.backward { at - lo } else { 0 };
    let shown = Shown {
        path: source,
        started: Instant::now(),
        all: (ahead + behind).max(1) as f64,
    };
    let mut whole = true;
    let mut after = Vec::new();
    if ahead > 0 {
        let mut tracker = Tracker::start(&model, &first, area);
        match going(reader.run(at + 1, hi))? {
            None => whole = false,
            Some(mut run) => {
                while (after.len() as i64) < ahead {
                    if crate::cancel::cancelled() {
                        whole = false;
                        break;
                    }
                    let frame = match going(run.next_frame())? {
                        None => {
                            whole = false;
                            break;
                        }
                        Some(None) => return Err(ended(at + 1 + after.len() as i64)),
                        Some(Some(frame)) => frame,
                    };
                    after.push(tracker.step(&picture(&frame))?);
                    shown.report(after.len());
                }
            }
        }
    }
    let mut before = Vec::new();
    if whole && behind > 0 {
        let mut tracker = Tracker::start(&model, &first, area);
        let most = (PART_BYTES / (size.0 * size.1 * 3).max(1)).max(1) as i64;
        let mut end = at;
        'parts: while end > lo {
            let start = (end - most).max(lo);
            let Some(part) = reader.part(start, end)? else {
                whole = false;
                break;
            };
            for p in part.iter().rev() {
                if crate::cancel::cancelled() {
                    whole = false;
                    break 'parts;
                }
                before.push(tracker.step(p)?);
                shown.report(after.len() + before.len());
            }
            end = start;
        }
    }
    before.reverse();
    let start = at - before.len() as i64;
    let mut kept = before;
    kept.push(found(area, size, 1.0));
    kept.extend(after);
    crate::debug!(
        "subjects",
        "{}: a track of {} pictures in {:.2} s",
        source.display(),
        kept.len(),
        shown.started.elapsed().as_secs_f64()
    );
    Ok(Tracked {
        first: start,
        found: kept,
        whole,
    })
}
