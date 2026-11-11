use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use tract_onnx::prelude::*;
use vidcut_core::analysis::{Detections, Follow, Found, SCENE_SCORE};

use crate::analysis::SEED;
use crate::decode::{Input, Reading, VideoReader};
use crate::error::{Error, Result};
use crate::export::Progress;
use crate::ffmpeg::{frame, software::scaling};
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
pub const LOST: f32 = 0.3;

pub fn name() -> String {
    let weights: String = FILE.sha256.chars().take(8).collect();
    format!("vittrack-{weights}-{}", (LOST * 100.0).round())
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

fn upright(bgr: &frame::Video, turn: u16) -> Picture {
    let (pw, ph) = (bgr.width() as usize, bgr.height() as usize);
    let (w, h) = if turn % 180 == 90 { (ph, pw) } else { (pw, ph) };
    let (stride, data) = (bgr.stride(0), bgr.data(0));
    let mut out = vec![0u8; w * h * 3];
    for y in 0..ph {
        let row = &data[y * stride..y * stride + pw * 3];
        for (x, pixel) in row.as_chunks::<3>().0.iter().enumerate() {
            let (ux, uy) = match turn {
                90 => (ph - 1 - y, x),
                180 => (pw - 1 - x, ph - 1 - y),
                270 => (y, pw - 1 - x),
                _ => (x, y),
            };
            out[(uy * w + ux) * 3..][..3].copy_from_slice(pixel);
        }
    }
    Picture {
        size: (w, h),
        bgr: out,
    }
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

    fn step(&mut self, picture: &Picture) -> Result<Option<f32>> {
        let [cx, cy, w, h] = self.last;
        let side = ((w * h).sqrt() * AROUND).ceil().max(1.0);
        let search = crop(picture, [cx, cy], side, SEARCH);
        let [sure, size, offset] = self.model.look(&self.template, &search)?;
        let scores = sure.iter().zip(&self.window).map(|(s, w)| s * w);
        let (best, score) = scores
            .enumerate()
            .fold((0, f32::MIN), |m, (i, v)| if v > m.1 { (i, v) } else { m });
        if score < LOST {
            return Ok(None);
        }
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
        Ok(Some(score))
    }

    fn found(&self, (w, h): (usize, usize), score: f32) -> Found {
        let [cx, cy, bw, bh] = self.last;
        let (w, h) = (w as f64, h as f64);
        Found {
            center: [(cx / w) as f32, (cy / h) as f32],
            size: [(bw / w) as f32, (bh / h) as f32],
            rotation: 0.0,
            score,
        }
    }
}

struct Seen {
    slot: i64,
    last: i64,
    pts: f64,
    scene: f32,
    picture: Picture,
}

struct Shots {
    reader: VideoReader,
    tick: f64,
    step: f64,
    turn: u16,
    from: f64,
    every: f64,
    area: [f32; 4],
    seeing: Option<Seeing>,
    wanted: i64,
    read: Option<f64>,
}

impl Shots {
    fn go(&mut self, slot: i64) -> Result<()> {
        let time = self.from + slot as f64 * self.every;
        self.reader.seek((time / self.tick).floor() as i64)?;
        (self.wanted, self.read) = (slot, None);
        if let Some(seeing) = &mut self.seeing {
            seeing.before = None;
        }
        Ok(())
    }

    fn next_if(&mut self, on: bool) -> Result<Option<Seen>> {
        if on { self.next() } else { Ok(None) }
    }

    fn next(&mut self) -> Result<Option<Seen>> {
        loop {
            crate::cancel::stop()?;
            let Some(decoded) = self.reader.next_frame()? else {
                return Ok(None);
            };
            let pts = decoded.pts().map(|p| p as f64 * self.tick);
            let pts = pts.unwrap_or_else(|| self.read.map_or(self.from, |p| p + self.step));
            self.read = Some(pts);
            if pts < self.from + self.wanted as f64 * self.every {
                continue;
            }
            let last = ((pts - self.from) / self.every).floor() as i64;
            let last = last.max(self.wanted);
            let slot = if last < 0 {
                self.wanted
            } else {
                self.wanted.max(0)
            };
            self.wanted = last + 1;
            let (turn, area) = (self.turn, self.area);
            let seeing = (self.seeing).get_or_insert_with(|| Seeing::start(&decoded, turn, area));
            let (picture, scene) = seeing.picture(&decoded)?;
            return Ok(Some(Seen {
                slot,
                last,
                pts,
                scene,
                picture,
            }));
        }
    }
}

struct Seeing {
    size: (u32, u32),
    turn: u16,
    scaler: Option<scaling::Context>,
    measure: Option<scaling::Context>,
    before: Option<(Vec<u8>, f64)>,
}

impl Seeing {
    fn start(first: &frame::Video, turn: u16, area: [f32; 4]) -> Self {
        let (aw, ah) = crate::subjects::fitted(first, turn, 4096);
        let (long, short) = (f64::from(aw.max(ah)), f64::from(aw.min(ah)));
        let share = f64::from(area[2] * area[3]).max(1e-9);
        let wanted = (4096.0 * long / short.max(1.0) / share).sqrt();
        let native = f64::from(first.width().max(first.height()));
        let most = wanted.clamp(SIDES.0, SIDES.1).min(native.max(SIDES.0));
        Self {
            size: crate::subjects::fitted(first, turn, most.round() as u32),
            turn,
            scaler: None,
            measure: None,
            before: None,
        }
    }

    fn picture(&mut self, decoded: &frame::Video) -> Result<(Picture, f32)> {
        let (cw, ch) = self.size;
        let scaled = if self.turn % 180 == 90 {
            (ch, cw)
        } else {
            (cw, ch)
        };
        let bgr = crate::subjects::small(&mut self.scaler, decoded, scaled)?;
        let luma = crate::analysis::small_luma(&mut self.measure, &bgr)?;
        let (scene, moved) = match &self.before {
            Some((last, moved)) => {
                let now = crate::analysis::mafd(last, &luma);
                (crate::analysis::scene_score(now, *moved), now)
            }
            None => (0.0, 0.0),
        };
        self.before = Some((luma, moved));
        Ok((upright(&bgr, self.turn), scene))
    }
}

type Kept = BTreeMap<i64, (f64, Vec<Found>, f32)>;

struct Shown<'a> {
    path: &'a Path,
    started: Instant,
    home: f64,
    ahead: f64,
    all: f64,
}

impl Shown<'_> {
    fn report(&self, pts: f64) {
        let done = if pts >= self.home {
            pts - self.home
        } else {
            self.ahead + self.home - pts
        };
        let seen = Progress::measuring(done / self.all, self.started.elapsed());
        crate::progress::report(self.path, seen);
    }
}

fn backward(
    shots: &mut Shots,
    mut tracker: Tracker,
    (home, shown): (&Seen, &Shown),
    kept: &mut Kept,
) -> Result<()> {
    let size = home.picture.size;
    let most = (PART_BYTES / (size.0 * size.1 * 3).max(1)).max(1) as i64;
    let (mut end, mut cut) = (home.slot, home.scene);
    while end > 0 {
        let start = (end - most).max(0);
        shots.go(start - SEED)?;
        let mut part = Vec::new();
        while let Some(seen) = shots.next()? {
            if seen.slot >= end {
                break;
            }
            if seen.slot >= start {
                part.push(seen);
            }
        }
        for seen in part.iter().rev() {
            if f64::from(cut) >= SCENE_SCORE {
                kept.insert(seen.slot, (seen.pts, Vec::new(), cut));
                return Ok(());
            }
            let Some(score) = tracker.step(&seen.picture)? else {
                kept.insert(seen.slot, (seen.pts, Vec::new(), 0.0));
                return Ok(());
            };
            let found = tracker.found(size, score);
            kept.insert(seen.slot, (seen.pts, vec![found], seen.scene));
            cut = seen.scene;
            shown.report(seen.pts);
        }
        end = start;
    }
    Ok(())
}

fn laid(kept: &Kept, from: f64, to: f64, every: f64) -> Detections {
    let mut out = Detections::default();
    let mut last = f64::NEG_INFINITY;
    let slots = ((to - from) / every).floor().max(0.0) as i64;
    for slot in 0..=slots {
        let due = from + slot as f64 * every;
        let (pts, found, scene) = match kept.get(&slot) {
            Some((pts, found, scene)) => (*pts, found.clone(), *scene),
            None if due > last => (due, Vec::new(), 0.0),
            None => continue,
        };
        out.pts.push(pts);
        out.found.push(found);
        out.scene.push(scene);
        last = pts;
    }
    out
}

pub fn track(
    source: &Input,
    follow: &Follow,
    gpu: Option<&crate::hw::GpuDecoder>,
    dir: &Path,
) -> Result<Detections> {
    crate::cancel::stop()?;
    get_ready(dir, false)?;
    let model = Model::load(&dir.join(FILE.name))?;
    let _turn = crate::turns::cpu_analysis()?;
    let look = follow.look;
    let every = look.every.max(1e-9);
    let home = ((follow.at - look.from) / every).round().max(0.0) as i64;
    let reader = VideoReader::open_as(source, gpu, Reading::Stream)?;
    let rate = reader.frame_rate();
    let mut shots = Shots {
        tick: f64::from(reader.time_base()),
        step: rate.map_or(0.04, |r| 1.0 / f64::from(r).max(1.0)),
        turn: reader.rotation(),
        reader,
        from: look.from,
        every,
        area: follow.area,
        seeing: None,
        wanted: 0,
        read: None,
    };
    let started = Instant::now();
    let mut kept = Kept::new();
    shots.go(home - SEED)?;
    let first = loop {
        match shots.next()? {
            Some(seen) if seen.last < home => {}
            seen => break seen,
        }
    };
    let Some(first) = first else {
        return Ok(laid(&kept, look.from, look.to, every));
    };
    let size = first.picture.size;
    let (w, h) = (size.0 as f64, size.1 as f64);
    let [cx, cy, bw, bh] = follow.area.map(f64::from);
    let area = clipped([cx * w, cy * h, bw * w, bh * h], size);
    let mut tracker = Tracker::start(&model, &first.picture, area);
    let found = tracker.found(size, 1.0);
    kept.insert(first.slot, (first.pts, vec![found], first.scene));
    let span = |on: bool, length: f64| if on { length.max(0.0) } else { 0.0 };
    let ahead = span(follow.forward, look.to - first.pts);
    let behind = span(follow.backward, first.pts - look.from);
    let shown = Shown {
        path: source.path(),
        started,
        home: first.pts,
        ahead,
        all: (ahead + behind).max(shots.step),
    };
    while let Some(seen) = shots.next_if(follow.forward)? {
        if f64::from(seen.scene) >= SCENE_SCORE {
            kept.insert(seen.slot, (seen.pts, Vec::new(), seen.scene));
            break;
        }
        let Some(score) = tracker.step(&seen.picture)? else {
            kept.insert(seen.slot, (seen.pts, Vec::new(), 0.0));
            break;
        };
        let found = tracker.found(size, score);
        kept.insert(seen.slot, (seen.pts, vec![found], seen.scene));
        shown.report(seen.pts);
        if seen.pts >= look.to {
            break;
        }
    }
    if follow.backward {
        let tracker = Tracker::start(&model, &first.picture, area);
        backward(&mut shots, tracker, (&first, &shown), &mut kept)?;
    }
    crate::debug!(
        "subjects",
        "{}: a track of {} pictures in {:.2} s",
        source.path().display(),
        kept.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(laid(&kept, look.from, look.to, every))
}
