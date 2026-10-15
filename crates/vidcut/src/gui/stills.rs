//! Exact single frames for the GUI (the Trim Editor's and Dynamic Trim's
//! two-up, on a copy of the project), rendered by the export engine so they
//! match export and never block the UI. The viewer itself shows the
//! player's pictures (docs/decisions/preview.md §4).
//!
//! Each cell showing frames has a thread of its own, so the cells render at
//! once, and a renderer that decodes each frame once whichever way the
//! position moves (docs/decisions/preview.md §9). A frame the renderer
//! keeps shows at once. Going back, the renderer plans the window before
//! the one it is in, and one more thread renders the cells' windows, one at
//! a time: they share the GPU's decoder. A cell renders the frames after
//! its position ahead of it, a few while it stands still and a quarter
//! second's while a run goes forward. A cell renders the newest frame it is
//! asked for, and shows its last picture until that one is ready.
//!
//! While Dynamic Trim runs, the two-up comes first (`vidcut_av::first`):
//! the filmstrip's and other jobs' decoding waits. A cell is asked for the
//! run's frames at the pace it shows them ([`every`]).

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use vidcut_av::export::renderer::{Kept, Prefetch, StillRenderer};
use vidcut_av::thumbs::Image;

use eframe::egui;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

/// A one-frame render: a piece (`frames == 1`) at a size.
#[derive(Debug, Clone, PartialEq)]
pub struct Still {
    pub piece: VideoPiece,
    /// Where the shown stretch of the file starts: frames behind it aren't
    /// worth rendering ahead (0: the whole file).
    pub floor: i64,
    pub fps: Rational,
    pub width: u32,
    pub height: u32,
}

/// Hashable identity of a [`Still`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key(String);

impl Still {
    fn key(&self) -> Key {
        Key(format!(
            "{}|{}|{}x{}",
            serde_json::to_string(&self.piece).unwrap_or_default(),
            self.fps,
            self.width,
            self.height
        ))
    }

    /// Its source position, where it has one: a run moves it.
    fn at(&self) -> Option<i64> {
        match &self.piece {
            VideoPiece::Source { source_frame, .. } => Some(*source_frame),
            _ => None,
        }
    }
}

enum State {
    Pending,
    Ready(egui::TextureHandle, Arc<Image>),
    Failed,
}

/// A frame a cell's thread is asked for.
struct Want {
    key: Key,
    still: Still,
    /// The run's frames a second then: forward above 0, back below, 0 for
    /// steps and jumps.
    rate: f64,
    /// The cell's count of frames asked for: which is newer.
    seq: u64,
}

/// What a cell's thread is asked for: the newest frame, which replaces one
/// it hadn't taken yet.
#[derive(Default)]
struct Slot {
    /// The frame, and whether the cell is gone.
    state: Mutex<(Option<Want>, bool)>,
    changed: Condvar,
}

/// The cell is gone: its thread ends.
struct Gone;

impl Slot {
    fn lock(&self) -> MutexGuard<'_, (Option<Want>, bool)> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Asks for `want`; the frame asked for before, if the thread hadn't
    /// taken it.
    fn put(&self, want: Want) -> Option<Want> {
        let old = self.lock().0.replace(want);
        self.changed.notify_one();
        old
    }

    /// The frame asked for, waiting for one up to `wait` (`None`: as long
    /// as it takes); `None` when none came.
    fn take(&self, wait: Option<Duration>) -> Result<Option<Want>, Gone> {
        let mut s = self.lock();
        let deadline = wait.map(|w| Instant::now() + w);
        loop {
            if let Some(want) = s.0.take() {
                return Ok(Some(want));
            }
            if s.1 {
                return Err(Gone);
            }
            s = match deadline {
                None => self.changed.wait(s).unwrap_or_else(|e| e.into_inner()),
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    self.changed
                        .wait_timeout(s, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
            };
        }
    }

    fn close(&self) {
        self.lock().1 = true;
        self.changed.notify_one();
    }
}

/// How many cells render now: a window waits until none does, as their
/// frames come first.
#[derive(Default)]
struct Busy {
    cells: Mutex<usize>,
    idle: Condvar,
}

impl Busy {
    fn lock(&self) -> MutexGuard<'_, usize> {
        self.cells.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, on: bool) {
        let mut n = self.lock();
        if on {
            *n += 1;
        } else {
            *n -= 1;
            if *n == 0 {
                self.idle.notify_all();
            }
        }
    }

    fn wait_idle(&self) {
        let mut n = self.lock();
        while *n > 0 {
            n = self.idle.wait(n).unwrap_or_else(|e| e.into_inner());
        }
    }
}

enum Outcome {
    /// A frame asked for (its key, `Want::seq` and source position),
    /// rendered or failed, and the seconds it took.
    Rendered(Key, u64, Option<i64>, Result<Arc<Image>, String>, f64),
    /// Where a cell's new renderer keeps its pictures.
    Kept(Kept),
}

/// How long a cell keeps its file and decoder open without being asked for
/// a frame: a decoder holds frame buffers and a thread per core.
const IDLE: Duration = Duration::from_secs(10);

/// Memory for a cell's kept pictures.
const BUDGET: usize = 256 << 20;

/// Textures kept for frames shown before: the renderer keeps the pictures,
/// so one shown again is only uploaded again.
const TEXTURES: usize = 24;

/// Render times a cell's pace goes by: its last frames'.
const PACE_OF: usize = 5;

/// What a cell shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shown {
    /// The frame asked for.
    Exact(egui::TextureId),
    /// The cell's last picture, while the frame asked for renders.
    Previous(egui::TextureId),
}

impl Shown {
    pub fn texture(self) -> egui::TextureId {
        match self {
            Shown::Exact(t) | Shown::Previous(t) => t,
        }
    }
}

/// A cell's thread, where its renderer keeps pictures, and what it shows.
struct Cell {
    slot: Arc<Slot>,
    kept: Option<Kept>,
    /// The frame shown, its picture, and the `seq` of the frame asked for
    /// last when it was shown: a frame asked for before then is older.
    shown: Option<(Key, egui::TextureHandle, Arc<Image>)>,
    shown_seq: u64,
    /// Frames asked for so far, and the last one waited for, until a frame
    /// asked for then or since lands.
    seq: u64,
    waiting: Option<u64>,
    /// During a run: the source position asked for or shown last, the
    /// seconds its last frames took to render (0: kept), and every how
    /// many of the run's frames it is asked for one ([`every`]).
    asked_at: Option<i64>,
    took: VecDeque<f64>,
    every: i64,
}

/// Frames the cells were asked for, rendered, and passed over for a newer
/// one before they rendered: too slow to keep their pace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub asked: u64,
    pub rendered: u64,
    pub passed_over: u64,
}

/// How the cells kept up with a run, the latest one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    /// The run's frames a second: forward above 0.
    pub rate: f64,
    /// How long it went on, so far.
    pub seconds: f64,
    pub cells: [RunCell; 2],
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RunCell {
    /// Frames it showed.
    pub shown: u64,
    /// The longest it showed one picture, in seconds: from the run's start
    /// on, and to its end.
    pub longest: f64,
    /// Every how many of the run's frames it was asked for one, last.
    pub every: i64,
}

/// A run going on, or the last.
struct Running {
    rate: f64,
    started: Instant,
    ended: Option<Instant>,
    cells: [RunCell; 2],
    /// When each cell's picture last changed.
    last: [Instant; 2],
}

pub struct Stills {
    ctx: egui::Context,
    cells: Vec<Cell>,
    finished: Sender<(usize, Outcome)>,
    done: Receiver<(usize, Outcome)>,
    /// The thread rendering the cells' windows going back, once no cell
    /// renders.
    windows: Sender<(usize, Prefetch)>,
    busy: Arc<Busy>,
    cache: HashMap<Key, State>,
    counts: Counts,
    /// While the two-up shows: the run's frames a second (forward above 0,
    /// 0 for steps and jumps).
    rate: Option<f64>,
    /// The two-up comes first while a run goes on.
    claim: vidcut_av::first::Claim,
    running: Option<Running>,
}

impl Stills {
    pub fn new(ctx: egui::Context) -> Self {
        let (finished, done) = channel();
        let (windows, behind) = channel::<(usize, Prefetch)>();
        let busy = Arc::new(Busy::default());
        let waits = busy.clone();
        // One window at a time, and once no cell renders: the cells share
        // the GPU's decoder, and the frames they render come first.
        let _ = crate::threads::spawn(
            "vidcut-stills-windows",
            crate::threads::Work::Interactive,
            move || {
                while let Ok((n, p)) = behind.recv() {
                    waits.wait_idle();
                    let started = Instant::now();
                    if vidcut_av::debug() {
                        eprintln!(
                            "[stills] {:.3} s: cell {n}: window behind starts",
                            vidcut_av::debug_clock()
                        );
                    }
                    let r = p.run();
                    if vidcut_av::debug() {
                        let ms = started.elapsed().as_secs_f64() * 1000.0;
                        let what = match r {
                            Ok(r) if r.whole() => {
                                format!("{}..={} in {ms:.1} ms", r.first, r.last)
                            }
                            Ok(r) => format!(
                                "{}..={} dropped after {} frames, {ms:.1} ms",
                                r.first, r.last, r.rendered
                            ),
                            Err(e) => format!("failed after {ms:.1} ms: {e}"),
                        };
                        eprintln!(
                            "[stills] {:.3} s: cell {n}: window behind {what}",
                            vidcut_av::debug_clock()
                        );
                    }
                }
            },
        );
        Self {
            ctx,
            cells: Vec::new(),
            finished,
            done,
            windows,
            busy,
            cache: HashMap::new(),
            counts: Counts::default(),
            rate: None,
            claim: vidcut_av::first::Claim::default(),
            running: None,
        }
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// How the cells kept up with the latest run, if there was one.
    pub fn run(&self) -> Option<Run> {
        let r = self.running.as_ref()?;
        let end = r.ended.unwrap_or_else(Instant::now);
        let mut cells = r.cells;
        for (i, c) in cells.iter_mut().enumerate() {
            c.longest = c
                .longest
                .max(end.saturating_duration_since(r.last[i]).as_secs_f64());
            c.every = self.cells.get(i).map_or(1, |c| c.every);
        }
        Some(Run {
            rate: r.rate,
            seconds: end.saturating_duration_since(r.started).as_secs_f64(),
            cells,
        })
    }

    /// The picture cell `cell` shows: the frame [`get`](Self::get) last
    /// asked for, or the one before while it renders.
    pub fn picture(&self, cell: usize) -> Option<Arc<Image>> {
        self.cells
            .get(cell)?
            .shown
            .as_ref()
            .map(|(.., p)| p.clone())
    }

    /// Each frame, before [`get`](Self::get): what the two-up does now.
    /// `rate`: the run's frames a second, forward above 0, 0 for steps and
    /// jumps; `None` while the two-up isn't shown.
    pub fn begin(&mut self, rate: Option<f64>) {
        self.take_in();
        let running = |r: Option<f64>| r.filter(|r| *r != 0.0);
        if running(rate) != running(self.rate) {
            let now = Instant::now();
            if let Some(r) = self.running.as_mut().filter(|r| r.ended.is_none()) {
                r.ended = Some(now);
                for (c, last) in r.cells.iter_mut().zip(r.last) {
                    c.longest = c.longest.max((now - last).as_secs_f64());
                }
                if vidcut_av::debug() {
                    eprintln!("[stills] {:.3} s: run ends", vidcut_av::debug_clock());
                }
            }
            if let Some(r) = running(rate) {
                self.running = Some(Running {
                    rate: r,
                    started: now,
                    ended: None,
                    cells: [RunCell::default(); 2],
                    last: [now; 2],
                });
                for c in &mut self.cells {
                    c.asked_at = None;
                    c.took.clear();
                    c.every = 1;
                }
                if vidcut_av::debug() {
                    eprintln!(
                        "[stills] {:.3} s: run at {r} frames a second",
                        vidcut_av::debug_clock()
                    );
                }
            }
        }
        self.claim.set(running(rate).is_some());
        self.rate = rate;
    }

    /// Takes in the frames rendered since.
    fn take_in(&mut self) {
        while let Ok((n, outcome)) = self.done.try_recv() {
            let (key, seq, at, r, took) = match outcome {
                Outcome::Kept(kept) => {
                    self.cell(n).kept = Some(kept);
                    continue;
                }
                Outcome::Rendered(key, seq, at, r, took) => (key, seq, at, r, took),
            };
            let c = self.cell(n);
            if c.waiting.is_some_and(|w| seq >= w) {
                c.waiting = None;
            }
            // Not waited for: shown already from the kept pictures, or told
            // where the run is.
            if !matches!(self.cache.get(&key), Some(State::Pending)) {
                continue;
            }
            let Ok(img) = r else {
                self.keep(key, State::Failed);
                continue;
            };
            self.counts.rendered += 1;
            let texture = self.texture(&img);
            self.paced(n, took);
            // Asked for before the frame the cell shows now: older.
            if seq > self.cell(n).shown_seq {
                self.cell(n).shown_seq = seq;
                self.show(n, key.clone(), at, texture.clone(), img.clone());
            }
            self.keep(key, State::Ready(texture, img));
        }
    }

    /// Notes `key`'s state. Over [`TEXTURES`], the textures of frames shown
    /// before go: the renderers keep their pictures.
    fn keep(&mut self, key: Key, state: State) {
        if self.cache.len() > TEXTURES {
            self.cache.retain(|_, s| matches!(s, State::Pending));
        }
        self.cache.insert(key, state);
    }

    fn texture(&self, img: &Image) -> egui::TextureHandle {
        self.ctx.load_texture(
            format!("still-{}", self.cache.len()),
            egui::ColorImage::from_rgba_unmultiplied(
                [img.width as usize, img.height as usize],
                &img.rgba,
            ),
            egui::TextureOptions::LINEAR,
        )
    }

    /// Cell `cell` shows `key`'s picture, of source position `at`: one more
    /// frame shown, if it is another.
    fn show(
        &mut self,
        cell: usize,
        key: Key,
        at: Option<i64>,
        texture: egui::TextureHandle,
        img: Arc<Image>,
    ) {
        let c = self.cell(cell);
        let new = c.shown.as_ref().is_none_or(|(k, ..)| *k != key);
        c.shown = Some((key, texture, img));
        if new && vidcut_av::debug() {
            eprintln!(
                "[stills] {:.3} s: cell {cell} shows source frame {}",
                vidcut_av::debug_clock(),
                at.unwrap_or(-1)
            );
        }
        if let Some(r) = self.running.as_mut().filter(|r| r.ended.is_none())
            && new
            && cell < 2
        {
            let now = Instant::now();
            let c = &mut r.cells[cell];
            c.shown += 1;
            c.longest = c.longest.max((now - r.last[cell]).as_secs_f64());
            r.last[cell] = now;
        }
    }

    /// One more of cell `cell`'s frames took `took` seconds: its pace
    /// follows while a run goes on.
    fn paced(&mut self, cell: usize, took: f64) {
        let Some(rate) = self.rate.filter(|r| *r != 0.0) else {
            return;
        };
        let c = self.cell(cell);
        c.took.push_back(took);
        if c.took.len() > PACE_OF {
            c.took.pop_front();
        }
        c.every = every(c.took.make_contiguous(), rate);
    }

    /// Cell `i`, starting its thread the first time.
    fn cell(&mut self, i: usize) -> &mut Cell {
        while self.cells.len() <= i {
            let n = self.cells.len();
            let slot = Arc::new(Slot::default());
            let (s, done, windows, busy, ctx) = (
                slot.clone(),
                self.finished.clone(),
                self.windows.clone(),
                self.busy.clone(),
                self.ctx.clone(),
            );
            let _ = crate::threads::spawn(
                format!("vidcut-stills-{n}"),
                crate::threads::Work::Interactive,
                move || render(n, &s, &done, &windows, &busy, &ctx),
            );
            self.cells.push(Cell {
                slot,
                kept: None,
                shown: None,
                shown_seq: 0,
                seq: 0,
                waiting: None,
                asked_at: None,
                took: VecDeque::new(),
                every: 1,
            });
        }
        &mut self.cells[i]
    }

    /// The picture for `cell`: the still's texture, rendering it in the
    /// background if needed, and until then the newest picture rendered for
    /// the cell (the position moves on before each frame is ready). During
    /// a run, a frame not kept is asked for at the cell's pace.
    pub fn get(&mut self, cell: usize, still: &Still) -> Option<Shown> {
        let key = still.key();
        let rate = self.rate.unwrap_or(0.0);
        self.cell(cell);
        match self.cache.get(&key) {
            Some(State::Ready(t, img)) => {
                let (t, img) = (t.clone(), img.clone());
                let id = t.id();
                let c = self.cell(cell);
                c.shown_seq = c.seq;
                self.show(cell, key, still.at(), t, img);
                return Some(Shown::Exact(id));
            }
            Some(State::Failed) => return None,
            Some(State::Pending) => {}
            None => {
                let kept = self.cells[cell]
                    .kept
                    .as_ref()
                    .and_then(|k| k.get(&still.piece, still.fps, still.width, still.height));
                if let Some(img) = kept {
                    self.counts.asked += 1;
                    self.counts.rendered += 1;
                    let texture = self.texture(&img);
                    let id = texture.id();
                    self.paced(cell, 0.0);
                    self.keep(key.clone(), State::Ready(texture.clone(), img.clone()));
                    // Running forward, the cell renders on ahead of it: told
                    // where the run is, unless a frame it renders will.
                    if rate > 0.0 && self.cells[cell].waiting.is_none() {
                        self.ask(cell, key.clone(), still, rate);
                    }
                    let c = self.cell(cell);
                    c.asked_at = still.at();
                    c.shown_seq = c.seq;
                    self.show(cell, key, still.at(), texture, img);
                    return Some(Shown::Exact(id));
                }
                let c = &self.cells[cell];
                // Its pace isn't known before its first frame of the run:
                // one frame on its way until then.
                let first = rate != 0.0 && c.took.is_empty() && c.waiting.is_some();
                let due = rate == 0.0
                    || match (c.asked_at, still.at()) {
                        (Some(a), Some(at)) => (at - a).abs() >= c.every,
                        _ => true,
                    };
                if due && !first {
                    self.keep(key.clone(), State::Pending);
                    self.counts.asked += 1;
                    self.ask(cell, key, still, rate);
                    let c = self.cell(cell);
                    c.asked_at = still.at();
                    c.waiting = Some(c.seq);
                }
            }
        }
        self.cells[cell]
            .shown
            .as_ref()
            .map(|(_, t, _)| Shown::Previous(t.id()))
    }

    /// Asks cell `cell`'s thread for `still`. The frame asked for before
    /// and not taken yet is passed over, unless it is shown already.
    fn ask(&mut self, cell: usize, key: Key, still: &Still, rate: f64) {
        let c = self.cell(cell);
        c.seq += 1;
        let want = Want {
            key,
            still: still.clone(),
            rate,
            seq: c.seq,
        };
        if let Some(old) = c.slot.put(want)
            && matches!(self.cache.get(&old.key), Some(State::Pending))
        {
            self.cache.remove(&old.key);
            self.counts.passed_over += 1;
        }
    }
}

impl Drop for Stills {
    fn drop(&mut self) {
        for c in &self.cells {
            c.slot.close();
        }
    }
}

/// Every how many of a run's frames a cell is asked for one, at `rate`
/// frames a second: as many as go by while it renders one, by the median
/// of the seconds its last frames took (`took`; 0 for one it kept), and at
/// least one. At most a quarter second's: slower than that it keeps no
/// pace, and the frames it passes over count.
fn every(took: &[f64], rate: f64) -> i64 {
    let rate = rate.abs();
    let most = ((rate / 4.0).round() as i64).max(1);
    let mut t = took.to_vec();
    t.sort_by(f64::total_cmp);
    let median = t.get(t.len() / 2).copied().unwrap_or(0.0);
    ((median * rate).ceil() as i64).clamp(1, most)
}

/// Frames a cell renders ahead of its position: a quarter second's of a
/// run going forward at `rate` frames a second, and while it stands still
/// (`rate` 0) a few, so a run or a step forward starts from kept frames.
fn lead(rate: f64) -> i64 {
    ((rate.abs() / 4.0).ceil() as i64).max(8)
}

/// A cell's thread: renders the frames it is asked for, and those after
/// them ([`lead`]) but while the position runs back.
fn render(
    n: usize,
    slot: &Slot,
    done: &Sender<(usize, Outcome)>,
    windows: &Sender<(usize, Prefetch)>,
    busy: &Busy,
    ctx: &egui::Context,
) {
    let mut decoding = crate::preferences::gpu_decoding();
    let mut renderer = cell_renderer(&decoding);
    if done.send((n, Outcome::Kept(renderer.kept()))).is_err() {
        return;
    }
    let mut working = Working {
        busy,
        claim: vidcut_av::first::Claim::default(),
        on: false,
    };
    // Frames to render ahead of the position, up to this source position.
    let mut ahead: Option<i64> = None;
    loop {
        // Waiting for a frame to render, it lets others decode.
        working.set(ahead.is_some());
        let want = match slot.take(Some(if ahead.is_some() {
            Duration::ZERO
        } else {
            IDLE
        })) {
            Ok(Some(w)) => w,
            Err(Gone) => return,
            Ok(None) => {
                if let Some(until) = ahead {
                    let started = Instant::now();
                    match renderer.ahead(until) {
                        Ok(Some(at)) => {
                            if vidcut_av::debug() {
                                eprintln!(
                                    "[stills] {:.3} s: cell {n}: source frame {at} ahead in {:.1} ms",
                                    vidcut_av::debug_clock(),
                                    started.elapsed().as_secs_f64() * 1000.0
                                );
                            }
                        }
                        _ => ahead = None,
                    }
                    continue;
                }
                // Idle: the file, decoder and pictures go.
                renderer = cell_renderer(&decoding);
                if done.send((n, Outcome::Kept(renderer.kept()))).is_err() {
                    return;
                }
                match slot.take(None) {
                    Ok(Some(w)) => w,
                    _ => return,
                }
            }
        };
        // The preferences as each frame is asked for: a change reaches the
        // two-up at once.
        let now = crate::preferences::gpu_decoding();
        if now != decoding {
            renderer = cell_renderer(&now);
            decoding = now;
            if done.send((n, Outcome::Kept(renderer.kept()))).is_err() {
                return;
            }
        }
        let s = &want.still;
        renderer.running_forward(want.rate > 0.0);
        working.set(true);
        let started = Instant::now();
        let r = renderer
            .rgba(&s.piece, s.floor, s.fps, s.width, s.height)
            .map_err(|e| e.to_string());
        let took = started.elapsed().as_secs_f64();
        if vidcut_av::debug() {
            eprintln!(
                "[stills] {:.3} s: cell {n}: source frame {} in {:.1} ms{}",
                vidcut_av::debug_clock(),
                s.at().unwrap_or(-1),
                took * 1000.0,
                r.as_ref().err().map_or(String::new(), |e| format!(": {e}"))
            );
        }
        // Windows behind it only while it doesn't run forward.
        if let Some(p) = renderer.take_prefetch() {
            let _ = windows.send((n, p));
        }
        // Ahead of it, but while it runs back.
        ahead = (want.rate >= 0.0 && r.is_ok())
            .then(|| s.at().map(|at| at + lead(want.rate)))
            .flatten();
        if done
            .send((n, Outcome::Rendered(want.key, want.seq, s.at(), r, took)))
            .is_err()
        {
            return;
        }
        ctx.request_repaint();
    }
}

/// A cell at work, from taking a frame to render until it waits for the
/// next, the frames after it included: it comes first, before jobs
/// (`vidcut_av::first`) and before windows ([`Busy`]).
struct Working<'a> {
    busy: &'a Busy,
    claim: vidcut_av::first::Claim,
    on: bool,
}

impl Working<'_> {
    fn set(&mut self, on: bool) {
        if on != self.on {
            self.on = on;
            self.claim.set(on);
            self.busy.set(on);
        }
    }
}

impl Drop for Working<'_> {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// A renderer for a cell, decoding as `decoding` says: on the GPU the
/// viewer decodes on, on a decoder of its own, while the GPU decodes.
fn cell_renderer(decoding: &crate::preferences::GpuDecoding) -> StillRenderer {
    let renderer = StillRenderer::with_budget(BUDGET);
    match decoding.own_decoder() {
        Some(gpu) => renderer.decoding_on(gpu),
        None => renderer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_keeping_up_is_asked_for_every_frame() {
        assert_eq!(every(&[0.0, 0.0, 0.012], 60.0), 1);
    }

    #[test]
    fn a_slower_cell_is_asked_for_every_frame_it_can_render() {
        // 25 ms a frame at 60 frames a second: one in two.
        assert_eq!(every(&[0.024, 0.031, 0.025], 60.0), 2);
    }

    #[test]
    fn going_back_paces_alike() {
        assert_eq!(every(&[0.024, 0.031, 0.025], -60.0), 2);
    }

    #[test]
    fn one_slow_frame_does_not_set_the_pace() {
        assert_eq!(every(&[0.01, 0.2, 0.01], 60.0), 1);
    }

    #[test]
    fn the_pace_asks_for_a_frame_every_quarter_second_at_least() {
        assert_eq!(every(&[2.0], 60.0), 15);
    }

    #[test]
    fn a_cell_renders_a_quarter_second_of_a_run_ahead() {
        assert_eq!(lead(60.0), 15);
        assert_eq!(lead(120.0), 30);
    }

    #[test]
    fn a_cell_standing_still_renders_a_few_frames_ahead() {
        assert_eq!(lead(0.0), 8);
    }
}
