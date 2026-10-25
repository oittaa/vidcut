use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use vidcut_core::render::{RenderPlan, VideoPiece, WHOLE};
use vidcut_core::time::Rational;

use super::frame::{Over, Picture};
use super::plan::Plan;
use super::store::Kind;
use super::{Busy, Decoder as Decoding, Mode, Shared, Snap, lock, ms};
use crate::error::{Error, Result};
use crate::export::is_cancelled;
use crate::export::make::Output;
use crate::export::renderer::{at, window};
use crate::export::video::{Layer, VideoFrames};
use crate::ffmpeg::format;

const PAUSED_AHEAD: i64 = 3;
const FOLLOWED_AHEAD: i64 = 8;

const AHEAD_SECS: f64 = 0.25;

const DECODE_ON_SECS: f64 = 1.0;

const WINDOW_SECS: f64 = 2.0;

const WINDOWS_BEHIND: usize = 2;

const SPARE: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Reach {
    Forward,
    Window { min: i64, max: i64 },
    Seek { max: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Keep {
    Ahead,
    Window,
    Seek,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Direction {
    Paused,
    Forward(f64),
    Back,
}

struct Decoder {
    shared: Arc<Shared>,
    frames: Option<((Rational, u32, u32, bool), VideoFrames)>,
    hardware: bool,
    gpu: Option<Option<crate::hw::GpuDecoder>>,
    picture_bytes: usize,
    picture_kind: Kind,
    next: Option<i64>,
    plan: Arc<Plan>,
    plan_gen: Option<u64>,
    last_focus: Option<i64>,
    back: bool,
    failed: Option<(u64, i64)>,
    seek_cost: Duration,
    caught_up: Option<i64>,
    prepared_failed: Option<u64>,
    back_windows: Vec<(i64, i64)>,
    back_low: Option<i64>,
    next_window: Option<(i64, i64)>,
    windows: bool,
    small: bool,
}

static WINDOW: Mutex<()> = Mutex::new(());

static FORWARD: AtomicUsize = AtomicUsize::new(0);

static WINDOW_FLAGS: Mutex<Vec<Weak<AtomicBool>>> = Mutex::new(Vec::new());

#[derive(Default)]
struct Forward(bool);

impl Forward {
    fn set(&mut self, on: bool) {
        if on == self.0 {
            return;
        }
        self.0 = on;
        if on {
            FORWARD.fetch_add(1, Ordering::Relaxed);
            for flag in lock(&WINDOW_FLAGS).iter().filter_map(Weak::upgrade) {
                flag.store(true, Ordering::Relaxed);
            }
        } else {
            FORWARD.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.set(false);
    }
}

pub(crate) fn run(shared: Arc<Shared>) {
    let _cancel = crate::cancel::Guard::set(shared.interrupt.clone());
    let _counting = crate::copies::Guard::set(shared.copies.clone());
    let Some(first) = shared.snap() else { return };
    let mut d = Decoder::new(shared.clone(), &first, false);
    let mut claim = crate::first::Claim::default();
    let mut forward = Forward::default();
    while let Some(snap) = shared.snap() {
        let dir = d.take(&snap);
        if snap.followed {
            start_windows(&shared);
        }
        forward.set(snap.followed && matches!(dir, Direction::Forward(_)));
        let waits = !shared.store.contains(&snap.plan.id(snap.focus))
            && d.failed != Some((snap.version, snap.focus));
        claim.set(dir != Direction::Paused || waits);
        let result = d.work(&snap, dir);
        shared.set_busy(None);
        match result {
            Ok(true) => {}
            Ok(false) => {
                d.close_left();
                let wait = match snap.mode {
                    Mode::Paused { .. } => Duration::from_secs(1),
                    _ => Duration::from_secs_f64(0.5 / snap.plan.fps().as_f64())
                        .min(Duration::from_millis(10)),
                };
                shared.wait_change(snap.version, wait);
            }
            Err(e) if is_cancelled(&e) => d.lost_position(),
            Err(e) => {
                shared.fail(e.to_string());
                if !shared.store.contains(&snap.plan.id(snap.focus)) {
                    shared.failed(snap.focus);
                }
                d.failed = Some((snap.version, snap.focus));
                d.lost_position();
                shared.wait_change(snap.version, Duration::from_millis(50));
            }
        }
    }
    drop(forward);
    drop(claim);
    d.end("decode thread");
}

fn start_windows(shared: &Arc<Shared>) {
    let mut thread = lock(&shared.windows);
    if thread.is_none() {
        let s = shared.clone();
        *thread = std::thread::Builder::new()
            .name("vidcut-play-windows".into())
            .spawn(move || run_windows(s))
            .ok();
    }
}

fn run_windows(shared: Arc<Shared>) {
    let _cancel = crate::cancel::Guard::set(shared.window_interrupt.clone());
    let _counting = crate::copies::Guard::set(shared.copies.clone());
    {
        let mut flags = lock(&WINDOW_FLAGS);
        flags.retain(|f| f.strong_count() > 0);
        flags.push(Arc::downgrade(&shared.window_interrupt));
    }
    let Some(first) = shared.snap_windows() else {
        return;
    };
    let mut d = Decoder::new(shared.clone(), &first, true);
    while let Some(snap) = shared.snap_windows() {
        let dir = d.take(&snap);
        if FORWARD.load(Ordering::Relaxed) > 0 {
            shared.wait_change(snap.version, Duration::from_millis(10));
            continue;
        }
        let result = {
            let _one = lock(&WINDOW);
            if shared.window_interrupt.load(Ordering::Relaxed) {
                continue;
            }
            d.windows_work(&snap, dir)
        };
        shared.set_window_busy(None);
        match result {
            Ok(true) => {}
            Ok(false) => {
                d.close_left();
                shared.wait_change(snap.version, Duration::from_millis(100));
            }
            Err(e) => {
                if !is_cancelled(&e) {
                    crate::debug!("player", "a window failed: {e}");
                }
                d.lost_position();
                shared.wait_change(snap.version, Duration::from_millis(50));
            }
        }
    }
    d.end("windows thread");
}

impl Decoder {
    fn end(mut self, thread: &str) {
        let started = Instant::now();
        let mut converters = Duration::ZERO;
        let mut reader = Duration::ZERO;
        if let Some((_, mut frames)) = self.frames.take() {
            let at = Instant::now();
            frames.clear_stages();
            converters = at.elapsed();
            let at = Instant::now();
            drop(frames);
            reader = at.elapsed();
        }
        let at = Instant::now();
        drop(self.gpu.take());
        crate::buffers::give_back();
        crate::debug!(
            "player",
            "player {}'s {thread} ended; its reader went in {:.0} ms: the converters in {:.0} ms, \
             the open files and their decoders in {:.0} ms, the GPU's decoder in {:.0} ms",
            self.shared.id,
            ms(started.elapsed()),
            ms(converters),
            ms(reader),
            ms(at.elapsed())
        );
    }
}

impl Decoder {
    fn new(shared: Arc<Shared>, first: &Snap, windows: bool) -> Self {
        let p = &first.plan.plan;
        Self {
            shared,
            frames: None,
            hardware: first.hardware,
            gpu: None,
            picture_bytes: (p.width as usize * p.height as usize * 3).div_ceil(2),
            picture_kind: Kind::Memory,
            next: None,
            plan: first.plan.clone(),
            plan_gen: None,
            last_focus: None,
            back: false,
            failed: None,
            seek_cost: Duration::ZERO,
            caught_up: None,
            prepared_failed: None,
            back_windows: Vec::new(),
            back_low: None,
            next_window: None,
            windows,
            small: false,
        }
    }

    fn take(&mut self, snap: &Snap) -> Direction {
        self.small = snap.small.is_some();
        if self.plan_gen != Some(snap.plan_gen) {
            self.plan = snap.plan.clone();
            self.plan_gen = Some(snap.plan_gen);
            self.next = None;
            self.last_focus = None;
            self.back = false;
            self.forget_windows();
        }
        if self.hardware != snap.hardware {
            self.hardware = snap.hardware;
            if !self.windows {
                self.shared.store.clear();
            }
            self.failed = None;
        }
        let dir = match snap.mode {
            Mode::Paused { .. } if snap.motion > 0.0 => Direction::Forward(snap.motion),
            Mode::Paused { .. } if snap.motion < 0.0 => Direction::Back,
            Mode::Paused { .. } => Direction::Paused,
            Mode::Playing { speed, .. } if speed > 0.0 => Direction::Forward(speed),
            Mode::Playing { .. } => Direction::Back,
        };
        match dir {
            Direction::Paused => {
                if let Some(last) = self.last_focus
                    && snap.focus != last
                {
                    self.back = snap.focus < last;
                }
            }
            Direction::Forward(_) => {
                if self.last_focus.is_some_and(|l| snap.focus < l) {
                    self.caught_up = None;
                }
                self.back = false;
            }
            Direction::Back => self.back = true,
        }
        if dir != Direction::Back {
            self.forget_windows();
        }
        self.last_focus = Some(snap.focus);
        dir
    }

    fn busy(&self, busy: Option<Busy>) {
        if self.windows {
            self.shared.set_window_busy(busy);
        } else {
            self.shared.set_busy(busy);
        }
    }

    fn windows_work(&mut self, snap: &Snap, dir: Direction) -> Result<bool> {
        let plan = self.plan.clone();
        let shared = self.shared.clone();
        let missing = |t: i64| !shared.store.contains(&plan.id(t));
        let focus = snap.focus;
        match dir {
            _ if !snap.followed => Ok(false),
            Direction::Forward(_) => Ok(false),
            Direction::Back if missing(focus) => {
                let first = self.decode_behind(snap, focus, self.windows(2).1)?;
                self.back_windows.push((first, focus));
                Ok(true)
            }
            Direction::Back => self.window_behind(snap),
            Direction::Paused if self.back && missing(focus) => {
                self.decode_behind(snap, focus, self.chunk())?;
                Ok(true)
            }
            Direction::Paused => {
                let (min, max) = self.windows(2);
                let behind = (plan.clamp(focus - min + 1)..focus)
                    .rev()
                    .filter(|&t| plan.fill_at(t).behind)
                    .find(|&t| missing(t));
                match behind {
                    Some(t) => {
                        self.decode_behind(snap, t, self.chunk().min(max))?;
                        Ok(true)
                    }
                    None => Ok(false),
                }
            }
        }
    }

    fn forget_windows(&mut self) {
        self.back_windows.clear();
        self.back_low = None;
        self.next_window = None;
    }

    fn close_left(&mut self) {
        if let Some((_, f)) = self.frames.as_mut() {
            f.close_left();
        }
    }

    fn lost_position(&mut self) {
        if let Some((_, f)) = self.frames.as_mut() {
            f.forget_position();
        }
        self.next = None;
    }

    fn work(&mut self, snap: &Snap, dir: Direction) -> Result<bool> {
        let plan = self.plan.clone();
        let store = &self.shared.store;
        let missing = |t: i64| !store.contains(&plan.id(t));
        let focus = snap.focus;
        if self.shared.titles.draw(&plan, focus) | self.shared.subtitles.draw(&plan, focus) {
            self.shared.drew_titles();
            return Ok(true);
        }
        let anchor = match (dir, self.caught_up) {
            (Direction::Forward(_), Some(c)) if c > focus && !missing(c) => c,
            _ => {
                self.caught_up = None;
                focus
            }
        };
        if anchor == focus && missing(focus) {
            if self.failed == Some((snap.version, focus)) {
                return Ok(false);
            }
            self.busy(Some(Busy::Focus));
            let started = Instant::now();
            let seeks = !self.reads_on(focus) && plan.fill_at(focus).lead;
            let target = match dir {
                Direction::Forward(speed) if seeks && snap.heard => {
                    let lead = self.seek_cost.as_secs_f64() * plan.fps().as_f64() * speed;
                    after(&plan, snap.looping, focus, lead.ceil() as i64)
                }
                _ => focus,
            };
            let reach = match dir {
                Direction::Back => Reach::Window {
                    min: 1,
                    max: self.windows(2).1,
                },
                _ if self.back => Reach::Window {
                    min: 1,
                    max: self.chunk(),
                },
                Direction::Paused => Reach::Seek {
                    max: self.windows(2).1,
                },
                _ => Reach::Forward,
            };
            if snap.followed && matches!(reach, Reach::Window { .. }) {
                return Ok(false);
            }
            let first = self.decode(snap, target, reach)?;
            if dir == Direction::Back {
                self.back_windows.push((first, target));
            }
            if seeks && plan.fill_at(target).lead {
                let took = started.elapsed();
                self.seek_cost = (self.seek_cost * 3 + took) / 4;
            }
            if target > focus {
                crate::debug!("player", "caught up at {target}, the clock at {focus}");
                self.caught_up = Some(target);
            }
            return Ok(true);
        }
        self.busy(Some(Busy::Ahead));
        let fps = plan.fps().as_f64();
        let wanted = match dir {
            Direction::Forward(speed) => {
                let ahead = ((AHEAD_SECS * fps * speed).ceil() as i64).max(2);
                let budget = store.budget(self.picture_kind);
                let ahead = ahead.min(self.pictures_in(budget / 4).max(2));
                (1..=ahead)
                    .map(|k| after(&plan, snap.looping, anchor, k))
                    .find(|&t| t != anchor && missing(t))
                    .map(|t| (t, Reach::Forward))
            }
            Direction::Back if snap.followed => return Ok(false),
            Direction::Back => return self.window_behind(snap),
            Direction::Paused => (focus + 1
                ..=plan.clamp(
                    focus
                        + if snap.followed {
                            FOLLOWED_AHEAD
                        } else {
                            PAUSED_AHEAD
                        },
                ))
                .find(|&t| missing(t))
                .filter(|&t| self.next == Some(t) && plan.fill_at(t).ahead)
                .map(|t| (t, Reach::Forward)),
        };
        if let Some((t, reach)) = wanted {
            self.decode(snap, t, reach)?;
            return Ok(true);
        }
        if dir != Direction::Paused {
            return Ok(false);
        }
        let (min, max) = self.windows(2);
        let behind = (plan.clamp(focus - min + 1)..focus)
            .rev()
            .filter(|&t| plan.fill_at(t).behind)
            .find(|&t| missing(t));
        if !self.back && self.prepare(snap)? {
            return Ok(true);
        }
        if let Some(t) = behind.filter(|_| !snap.followed) {
            self.decode_behind(snap, t, self.chunk().min(max))?;
            return Ok(true);
        }
        if self.back {
            self.prepare(snap)
        } else {
            Ok(false)
        }
    }

    fn window_behind(&mut self, snap: &Snap) -> Result<bool> {
        let plan = self.plan.clone();
        let shared = self.shared.clone();
        let store = &shared.store;
        let focus = snap.focus;
        let (min, max) = self.windows(2);
        let from = self
            .back_low
            .filter(|&l| l <= focus && store.contains(&plan.id(l)))
            .unwrap_or(focus);
        let reach = plan.clamp(from - 3 * max);
        let run = store.kept_run((reach..from).rev().map(|t| plan.id(t)));
        let low = from - run as i64;
        self.back_low = Some(low);
        self.back_windows.retain(|&(first, _)| first <= focus);
        let behind = self
            .back_windows
            .iter()
            .filter(|&&(first, last)| last < focus && first >= low)
            .count();
        let next = low - 1;
        if behind >= WINDOWS_BEHIND || next < 0 || next < reach {
            return Ok(false);
        }
        let room = self.pictures_in(store.budget(self.picture_kind)) - (focus - low + 1) - SPARE;
        let first = if behind == 0 {
            if room < 1 {
                return Ok(false);
            }
            self.window_start(next, 1, max.min(room))?
        } else {
            let first = match self.next_window {
                Some((last, first)) if last == next => first,
                _ => self.window_start(next, min, max)?,
            };
            if next - first + 1 > room {
                self.next_window = Some((next, first));
                return Ok(false);
            }
            first
        };
        self.next_window = None;
        crate::debug!(
            "player",
            "window {first}..={next} behind, {behind} waiting there"
        );
        store.touch((low..=focus).map(|t| plan.id(t)));
        self.busy(Some(Busy::Window {
            from: first,
            to: next,
            plan_gen: snap.plan_gen,
        }));
        self.decode_from(snap, Some(first), next, Keep::Window)?;
        self.back_windows.push((first, next));
        Ok(true)
    }

    fn decode_behind(&mut self, snap: &Snap, t: i64, max: i64) -> Result<i64> {
        self.reader();
        let from = if self.reads_on(t) {
            None
        } else {
            Some(self.window_start(t, 1, max)?)
        };
        let first = from.or(self.next).unwrap_or(t);
        self.busy(Some(Busy::Window {
            from: first,
            to: t,
            plan_gen: snap.plan_gen,
        }));
        self.decode_from(snap, from, t, Keep::Window)?;
        Ok(first)
    }

    fn prepare(&mut self, snap: &Snap) -> Result<bool> {
        let Some((plan, t)) = &snap.prepared else {
            return Ok(false);
        };
        let id = plan.id(*t);
        if self.shared.store.contains(&id) || self.prepared_failed == Some(snap.version) {
            return Ok(false);
        }
        crate::debug!("player", "prepare {t} of the plan played next");
        self.shared.titles.draw(plan, *t);
        self.shared.subtitles.draw(plan, *t);
        let picture = (|| {
            let mut frames = self.new_reader(&plan.plan);
            frames.read_pieces(&plan.pieces_from(*t));
            let layers = frames
                .next_layers()?
                .ok_or_else(|| Error::Invalid(format!("no frame {t} to prepare")))?;
            Ok(picture(layers))
        })();
        match picture {
            Ok(p) => {
                let p = if p.holds_a_pool() {
                    p.copied_down().unwrap_or(p)
                } else {
                    p
                };
                self.shared.store.put(id, Arc::new(p));
                crate::hw::let_unused_frames_go();
                crate::buffers::give_back();
                self.shared.prepared();
            }
            Err(e) if is_cancelled(&e) => {}
            Err(e) => {
                crate::debug!("player", "preparing {t} failed: {e}");
                self.prepared_failed = Some(snap.version);
            }
        }
        Ok(true)
    }

    fn reads_on(&self, t: i64) -> bool {
        let limit = (DECODE_ON_SECS * self.plan.fps().as_f64()).ceil() as i64;
        self.next.is_some_and(|r| r <= t && t - r <= limit)
    }

    fn windows(&self, parts: usize) -> (i64, i64) {
        let budget = self.shared.store.budget(self.picture_kind);
        let max = self.pictures_in(budget / parts.max(1)).max(1);
        let min = ((self.plan.fps().as_f64() * WINDOW_SECS).round() as i64).clamp(1, max);
        (min, max)
    }

    fn chunk(&self) -> i64 {
        match self.picture_kind {
            Kind::Video => self.windows(2).1,
            Kind::Memory if self.small => self.windows(2).1,
            Kind::Memory => self.memory_chunk(),
        }
    }

    fn memory_chunk(&self) -> i64 {
        let (_, max) = self.windows(2);
        ((self.plan.fps().as_f64() * 0.5).ceil() as i64).clamp(8, max.max(8))
    }

    fn pictures_in(&self, bytes: usize) -> i64 {
        (bytes / self.picture_bytes.max(1)) as i64
    }

    fn reader(&mut self) -> &mut VideoFrames {
        let plan = self.plan.clone();
        let p = &plan.plan;
        let key = (p.fps, p.width, p.height, self.hardware);
        if self.frames.as_ref().is_none_or(|(k, _)| *k != key) {
            let frames = self.new_reader(p);
            if self.frames.replace((key, frames)).is_some() {
                crate::buffers::give_back();
            }
            self.next = None;
        }
        &mut self.frames.as_mut().expect("just made").1
    }

    fn new_reader(&mut self, p: &RenderPlan) -> VideoFrames {
        let frames = VideoFrames::new(&[], p.fps, p.width, p.height, format::Pixel::YUV420P, None);
        let decoded = |keep| Output::Decoded { keep };
        if !self.hardware {
            crate::debug!(
                "player",
                "a new reader decodes on the CPU: GPU decoding is off"
            );
            self.shared.decodes_on_cpu(Some("GPU decoding is off"));
            return frames.output(decoded(Vec::new()));
        }
        let gpu = self.gpu.get_or_insert_with(crate::hw::GpuDecoder::best);
        let Some(gpu) = gpu else {
            crate::debug!(
                "player",
                "a new reader decodes on the CPU: no GPU decoder here"
            );
            self.shared.decodes_on_cpu(Some("no GPU decoder here"));
            return frames.output(decoded(Vec::new()));
        };
        self.shared.decodes_on_cpu(None);
        let shows = self.shared.gpu_pictures.clone();
        crate::debug!(
            "player",
            "a new reader decodes on {gpu} where it can; the viewer shows {shows:?} pictures \
             from GPU memory, the rest come down to memory"
        );
        if let Some(found) = gpu.gpu() {
            self.shared.gpu_found(found);
        }
        frames.output(decoded(shows)).decode_on(gpu.clone())
    }

    fn decode(&mut self, snap: &Snap, t: i64, reach: Reach) -> Result<i64> {
        self.reader();
        let from = if self.reads_on(t) {
            None
        } else {
            Some(match reach {
                Reach::Window { min, max } => self.window_start(t, min, max)?,
                Reach::Seek { max } => self.window_start(t, 1, max)?,
                Reach::Forward => t,
            })
        };
        let first = from.or(self.next).unwrap_or(t);
        let keep = match reach {
            Reach::Forward => Keep::Ahead,
            Reach::Window { .. } => Keep::Window,
            Reach::Seek { .. } => Keep::Seek,
        };
        self.decode_from(snap, from, t, keep)?;
        Ok(first)
    }

    fn decode_from(&mut self, snap: &Snap, from: Option<i64>, t: i64, keep: Keep) -> Result<()> {
        let plan = self.plan.clone();
        if let Some(from) = from {
            self.reader().read_pieces(&plan.pieces_from(from));
            self.next = Some(from);
        }
        let r = self.next.expect("positioned");
        crate::debug!(
            "player",
            "decode {r}..={t} (viewer at {}{})",
            snap.focus,
            if keep == Keep::Ahead {
                ""
            } else {
                ", keeping a window"
            }
        );
        let started = Instant::now();
        let mut seek_keeps: Option<i64> = None;
        for i in r..=t {
            let window = match keep {
                Keep::Ahead => false,
                Keep::Window => true,
                Keep::Seek => seek_keeps.is_none_or(|from| i >= from),
            };
            let kept = i == t
                || window
                || ((i >= snap.focus || snap.followed) && !self.shared.store.contains(&plan.id(i)));
            let whole =
                snap.motion == 0.0 && matches!(snap.mode, Mode::Paused { .. }) && i >= snap.focus;
            let small = snap.small.filter(|_| !whole);
            let frames = self.reader();
            frames.shrink_to(small);
            if kept {
                let layers = frames
                    .next_layers()?
                    .ok_or_else(|| Error::Invalid(format!("no frame {i} to play")))?;
                let picture = picture(layers);
                self.next = Some(i + 1);
                self.picture_bytes = picture.bytes();
                self.picture_kind = Kind::of(&picture);
                if keep == Keep::Seek && seek_keeps.is_none() {
                    seek_keeps = Some(t - self.chunk() + 1);
                }
                let id = if small.is_some() {
                    self.shared.store.small(&plan.id(i))
                } else {
                    plan.id(i)
                };
                self.shared.titles.draw(&plan, i);
                self.shared.subtitles.draw(&plan, i);
                self.shared.store.put(id.clone(), Arc::new(picture));
                if i == snap.focus {
                    self.shared.landed(snap.plan_gen, id);
                }
            } else {
                frames.skip_frame()?;
                self.next = Some(i + 1);
            }
        }
        if crate::debug() {
            let took = started.elapsed().as_secs_f64() * 1000.0;
            let n = t - r + 1;
            let on = self
                .reader()
                .decoded_on()
                .map_or("the CPU", crate::hw::Kind::label);
            let store = &self.shared.store;
            let (kept, bytes) = store.usage();
            crate::debug!(
                "player",
                "decoded {r}..={t} on {on}: {n} frames in {took:.1} ms, {:.2} ms each; \
                 {kept} pictures kept in {} MB (budgets {} MB of memory, {} MB of video \
                 memory)",
                took / n as f64,
                bytes >> 20,
                store.budget(Kind::Memory) >> 20,
                store.budget(Kind::Video) >> 20,
            );
        }
        Ok(())
    }

    fn window_start(&mut self, t: i64, min: i64, max: i64) -> Result<i64> {
        let plan = self.plan.clone();
        let Some((piece, piece_start)) = plan.piece_at(t) else {
            return Ok(t);
        };
        let first = piece
            .layers()
            .into_iter()
            .map(|(l, _)| l)
            .find(|l| matches!(l, VideoPiece::Source { image: None, .. }))
            .cloned();
        let piece = first.unwrap_or(piece);
        let VideoPiece::Source {
            source_frame: pos,
            image: None,
            pace,
            ..
        } = piece
        else {
            return Ok(t);
        };
        if !pace.is_normal() {
            if pace.speed == 0.0 {
                return Ok(t);
            }
            let n = ((plan.fps().as_f64() * WINDOW_SECS).round() as i64)
                .max(min)
                .min(max)
                .max(1);
            return Ok((t - n + 1).max(piece_start));
        }
        let floor = pos - (t - piece_start);
        let frames = self.reader();
        let start = window(
            frames,
            &at(&piece, pos, 1),
            pos,
            floor,
            plan.fps(),
            min,
            max,
            true,
        )?;
        self.next = None;
        Ok(t - (pos - start))
    }
}

fn after(plan: &Plan, looping: Option<(i64, i64)>, from: i64, k: i64) -> i64 {
    let t = from + k;
    plan.clamp(match looping {
        Some((a, b)) if from < b && t >= b => a + (t - b) % (b - a),
        Some((a, b)) if t > plan.last() => a + (t - plan.last() - 1) % (b - a),
        _ => t,
    })
}

fn picture(layers: Vec<Layer>) -> Picture {
    let made = |l: Layer| {
        let decoder = l
            .decoding
            .is_some()
            .then(|| l.decoder.map_or(Decoding::Cpu, Decoding::Gpu));
        let frame = l.frame.expect("frames as decoded, black too");
        Picture::new(frame, l.rotation, decoder, l.decoding)
    };
    let mut layers = layers.into_iter();
    let under = made(layers.next().expect("a frame's picture"));
    let over = layers
        .map(|l| {
            let weight = l.weight.unwrap_or(WHOLE);
            let wipe = l.wipe;
            Over {
                picture: Arc::new(made(l)),
                weight,
                wipe,
            }
        })
        .collect();
    under.with_over(over)
}
