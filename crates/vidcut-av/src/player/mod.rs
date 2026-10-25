mod audio;
mod clock;
mod frame;
mod output;
mod plan;
mod ring;
mod scrub;
mod store;
mod subtitles;
mod titles;
mod video;

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use vidcut_core::model::Title;
use vidcut_core::render::RenderPlan;

pub use frame::{Layout, Over, Picture, Shown, TitleOver, TitlePicture};
pub use output::{
    AudioOutput, AudioSource, CpalOutput, ManualOutput, OutputDevice, PacedOutput, PlaysOn,
    Recording, output_device, output_devices,
};
pub use subtitles::SubtitleView;

use crate::error::{Error, Result};
use clock::{Heard, Markers};
use plan::Plan;
use ring::Ring;
use store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub memory: usize,
    pub video: Option<usize>,
}

pub const SHARED_MEMORY: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

pub const DEFAULT_BUDGET: Budget = Budget::both(GB);

const GB: usize = 1 << 30;

impl Budget {
    pub const fn both(bytes: usize) -> Self {
        Self {
            memory: bytes,
            video: Some(bytes),
        }
    }

    pub fn part(self, parts: usize) -> Self {
        let gpu = || Self::gpu_video(crate::gpus::in_use().and_then(|g| g.memory_mb));
        let video = match self.video {
            None if SHARED_MEMORY => None,
            v => Some(v.unwrap_or_else(gpu) / parts.max(1)),
        };
        Self {
            memory: self.memory / parts.max(1),
            video,
        }
    }

    pub fn automatic() -> Self {
        Self {
            memory: system_memory().map_or(GB, |m| (m / 2).max(GB)),
            video: None,
        }
    }

    pub fn gpu_video(memory_mb: Option<u64>) -> usize {
        memory_mb.map_or(GB, |mb| ((mb as usize) << 19).max(GB))
    }
}

pub fn system_memory() -> Option<usize> {
    #[cfg(unix)]
    {
        // SAFETY: sysconf reads system constants.
        let (pages, size) = unsafe {
            (
                libc::sysconf(libc::_SC_PHYS_PAGES),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        (pages > 0 && size > 0).then(|| (pages as usize).saturating_mul(size as usize))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: fills the struct it is given, its length set first.
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        (unsafe { GlobalMemoryStatusEx(&mut status) } != 0)
            .then(|| usize::try_from(status.ullTotalPhys).unwrap_or(usize::MAX))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

#[derive(Clone)]
pub struct Config {
    pub budget: Budget,
    pub audio_scrubbing: bool,
    pub on_picture: Option<Arc<dyn Fn() + Send + Sync>>,
    pub hardware: bool,
    pub gpu_pictures: Vec<crate::hw::Kind>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            budget: DEFAULT_BUDGET,
            audio_scrubbing: true,
            on_picture: None,
            hardware: false,
            gpu_pictures: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Stats {
    pub position: i64,
    pub shown: Option<Shown>,
    pub super_scales: Vec<vidcut_core::model::SuperScale>,
    pub frames: u64,
    pub late: u64,
    pub skipped: u64,
    pub ahead: usize,
    pub behind: usize,
    pub kept: usize,
    pub memory: (usize, usize),
    pub video: (usize, usize),
    pub shared: bool,
    pub cpu: Option<&'static str>,
    pub plays_on: PlaysOn,
    pub rate: u32,
    pub channels: u16,
    pub latency: Option<Duration>,
    pub copies: vidcut_core::editor::Copied,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub position: i64,
    pub playing: bool,
    pub speed: f64,
    pub ready: bool,
    pub prepared: bool,
    pub presented: Option<i64>,
    pub range: (i64, i64),
    pub late: u64,
    pub skipped: u64,
    pub kept: (usize, usize),
    pub latency: Option<Duration>,
    pub decoder: Option<Decoder>,
    pub failed: Option<i64>,
    pub error: Option<String>,
    pub glide: Option<Glide>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    pub fraction: f64,
    pub rate: f64,
    pub at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub mix: Vec<f32>,
    pub lanes: Vec<Vec<f32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoder {
    Cpu,
    Gpu(crate::hw::Kind),
}

impl Decoder {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu(kind) => kind.name(),
        }
    }
}

pub(crate) struct Control {
    pub plan: Arc<Plan>,
    pub plan_gen: u64,
    pub mode: Mode,
    pub version: u64,
    changes: u64,
    epoch: u64,
    epoch_at: Instant,
    pub trims: Vec<f64>,
    pub trims_gen: u64,
    pub muted: bool,
    scrubbing: bool,
    pub scrub: Option<(i64, u64)>,
    pub looping: Option<(i64, i64)>,
    pub hardware: bool,
    pub motion: f64,
    pub followed: bool,
    pub small: Option<(u32, u32)>,
    pub prepared: Option<(Arc<Plan>, i64)>,
    pub closed: bool,
    error: Option<String>,
}

impl Control {
    fn scrub(&mut self, t: i64) {
        if self.scrubbing {
            let n = self.scrub.map_or(0, |(_, n)| n) + 1;
            self.scrub = Some((t, n));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Paused { at: i64 },
    Playing { from: i64, speed: f64, epoch: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Busy {
    Focus,
    Ahead,
    Window { from: i64, to: i64, plan_gen: u64 },
}

#[derive(Default)]
struct Presenter {
    shown: Option<Shown>,
    shown_landing: u64,
    presented: Option<i64>,
    presented_in: Option<u64>,
    frames: u64,
    decoder: Option<Decoder>,
    late: u64,
    late_at: Option<i64>,
    skipped: u64,
}

impl Presenter {
    fn shows(&mut self, clock: Option<u64>, pos: i64) {
        if let (Some(epoch), Some(before)) = (clock, self.presented)
            && self.presented_in == Some(epoch)
        {
            let passed = (pos - before).abs() - 1;
            if passed > 0 {
                self.skipped += passed as u64;
            }
        }
        if self.presented != Some(pos) {
            self.frames += 1;
        }
        self.presented = Some(pos);
        self.presented_in = clock;
    }

    fn missed(&mut self, runs: bool, pos: i64) {
        if runs && self.late_at != Some(pos) {
            self.late += 1;
            self.late_at = Some(pos);
        }
    }
}

pub(crate) struct Snap {
    pub plan: Arc<Plan>,
    pub plan_gen: u64,
    pub mode: Mode,
    pub version: u64,
    pub focus: i64,
    pub heard: bool,
    pub hardware: bool,
    pub motion: f64,
    pub followed: bool,
    pub small: Option<(u32, u32)>,
    pub prepared: Option<(Arc<Plan>, i64)>,
    pub looping: Option<(i64, i64)>,
}

pub(crate) struct AudioSnap {
    pub plan: Arc<Plan>,
    pub plan_gen: u64,
    pub mode: Mode,
    pub trims: Vec<f64>,
    pub trims_gen: u64,
    pub muted: bool,
    pub scrub: Option<(i64, u64)>,
    pub looping: Option<(i64, i64)>,
    pub changes: u64,
}

pub(crate) struct Shared {
    pub(crate) id: u64,
    control: Mutex<Control>,
    changed: Condvar,
    pub(crate) store: Store,
    pub(crate) ring: Arc<Ring>,
    pub(crate) heard: Arc<Heard>,
    pub(crate) markers: Markers,
    pub(crate) interrupt: Arc<AtomicBool>,
    busy: Mutex<Option<(Busy, Instant)>>,
    pub(crate) windows: Mutex<Option<JoinHandle<()>>>,
    pub(crate) window_interrupt: Arc<AtomicBool>,
    window_busy: Mutex<Option<(Busy, Instant)>>,
    failed: Mutex<Option<i64>>,
    landed: Mutex<(u64, u64, Option<plan::Id>)>,
    cpu: Mutex<Option<&'static str>>,
    on_picture: Option<Arc<dyn Fn() + Send + Sync>>,
    presenter: Mutex<Presenter>,
    device: Arc<Mutex<Option<String>>>,
    format: (u32, u16),
    pub(crate) gpu_pictures: Vec<crate::hw::Kind>,
    copies: Arc<crate::copies::Copies>,
    video_auto: AtomicBool,
    gpu_memory: std::sync::OnceLock<u64>,
    heard_epoch: std::sync::atomic::AtomicU64,
    levels: Mutex<std::collections::VecDeque<Level>>,
    pub(crate) titles: titles::Titles,
    pub(crate) subtitles: subtitles::Subtitles,
    placing: Mutex<Vec<(String, vidcut_core::model::Picture)>>,
}

pub(crate) struct Level {
    pub at: u64,
    pub frames: u64,
    pub mix: Vec<f32>,
    pub lanes: Vec<f32>,
}

const LEVELS_KEPT: usize = 160;

const LEVELS_SECS: f64 = 0.05;

const PATIENCE: Duration = Duration::from_secs(1);

static PLAYERS: AtomicU64 = AtomicU64::new(0);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Shared {
    fn control(&self) -> MutexGuard<'_, Control> {
        lock(&self.control)
    }

    fn clock_sample(&self, epoch: u64, now: Instant) -> Option<f64> {
        let counter = self.heard.counter_at(now)?;
        self.markers.sample_at(counter, epoch)
    }

    fn position(&self, ctl: &mut Control, now: Instant) -> i64 {
        let (t, ended) = self.clock_frame(ctl, now);
        if ended {
            ctl.mode = Mode::Paused { at: t };
            ctl.version += 1;
            ctl.changes += 1;
            self.changed.notify_all();
        }
        t
    }

    fn clock_frame(&self, ctl: &Control, at: Instant) -> (i64, bool) {
        let (from, speed, epoch) = match ctl.mode {
            Mode::Paused { at } => return (at, false),
            Mode::Playing { from, speed, epoch } => (from, speed, epoch),
        };
        let end = ctl.plan.samples as f64;
        let heard = self.clock_sample(epoch, at);
        if heard.is_some() && crate::debug() {
            self.heard_first(ctl, epoch);
        }
        let s = heard.unwrap_or(from as f64).clamp(0.0, end);
        let ended = ctl.looping.is_none() && if speed > 0.0 { s >= end } else { s <= 0.0 };
        (ctl.plan.frame_at_sample(s.floor() as i64), ended)
    }

    fn glide(&self, ctl: &Control, now: Instant) -> Option<Glide> {
        let Mode::Playing { speed, epoch, .. } = ctl.mode else {
            return None;
        };
        let plan = &ctl.plan;
        let s = self
            .clock_sample(epoch, now)?
            .clamp(0.0, plan.samples as f64);
        let t = plan.frame_at_sample(s.floor() as i64);
        let (a, b) = (plan.sample(t) as f64, plan.sample(t + 1) as f64);
        Some(Glide {
            fraction: ((s - a) / (b - a).max(1.0)).clamp(0.0, 1.0),
            rate: plan.fps().as_f64() * speed,
            at: now,
        })
    }

    fn heard_first(&self, ctl: &Control, epoch: u64) {
        if self
            .heard_epoch
            .fetch_max(epoch, std::sync::atomic::Ordering::Relaxed)
            < epoch
        {
            crate::debug!(
                "audio",
                "play request {epoch} heard {:.0} ms after it was made, the output's latency {:.0} ms",
                ms(ctl.epoch_at.elapsed()),
                ms(self.heard.latency().unwrap_or_default()),
            );
        }
    }

    fn change(&self, video: bool, f: impl FnOnce(&mut Control, i64, Option<f64>)) {
        let mut ctl = self.control();
        let now = Instant::now();
        let pos = self.position(&mut ctl, now);
        let sample = match ctl.mode {
            Mode::Playing { epoch, .. } => self.clock_sample(epoch, now),
            Mode::Paused { .. } => None,
        };
        let was_playing = matches!(ctl.mode, Mode::Playing { .. });
        f(&mut ctl, pos, sample);
        if video {
            ctl.version += 1;
            let playing = was_playing || matches!(ctl.mode, Mode::Playing { .. });
            let target = match ctl.mode {
                Mode::Paused { at } => at,
                Mode::Playing { from, .. } => ctl.plan.frame_at_sample(from),
            };
            let on_its_way = (1..=ctl.plan.fps().as_f64().ceil() as i64).contains(&(target - pos));
            let busy = *lock(&self.busy);
            let cancel = match busy {
                Some((Busy::Ahead, _)) => !on_its_way,
                Some((Busy::Window { from, to, plan_gen }, _)) => {
                    playing || plan_gen != ctl.plan_gen || !(from..=to).contains(&target)
                }
                Some((Busy::Focus, since)) => sample.is_some() || since.elapsed() >= PATIENCE,
                None => false,
            };
            let kept = self.store.contains(&ctl.plan.id(target));
            if cancel && !kept {
                self.interrupt
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some((Busy::Window { from, to, plan_gen }, _)) = *lock(&self.window_busy)
                && (ctl.motion > 0.0
                    || (!kept && (plan_gen != ctl.plan_gen || !(from..=to).contains(&target))))
            {
                self.window_interrupt
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        self.notify(ctl);
    }

    fn notify(&self, mut ctl: MutexGuard<'_, Control>) {
        ctl.changes += 1;
        drop(ctl);
        self.changed.notify_all();
    }

    pub(crate) fn snap(&self) -> Option<Snap> {
        self.snap_clearing(&self.interrupt)
    }

    pub(crate) fn snap_windows(&self) -> Option<Snap> {
        self.snap_clearing(&self.window_interrupt)
    }

    fn snap_clearing(&self, interrupt: &AtomicBool) -> Option<Snap> {
        let mut ctl = self.control();
        if ctl.closed {
            return None;
        }
        interrupt.store(false, std::sync::atomic::Ordering::Relaxed);
        let now = Instant::now();
        let focus = self.position(&mut ctl, now);
        let heard = match ctl.mode {
            Mode::Playing { epoch, .. } => self.clock_sample(epoch, now).is_some(),
            Mode::Paused { .. } => false,
        };
        Some(Snap {
            plan: ctl.plan.clone(),
            plan_gen: ctl.plan_gen,
            mode: ctl.mode,
            version: ctl.version,
            focus,
            heard,
            hardware: ctl.hardware,
            motion: ctl.motion,
            followed: ctl.followed,
            small: ctl.small,
            prepared: ctl.prepared.clone(),
            looping: ctl.looping,
        })
    }

    pub(crate) fn audio_snap(&self) -> Option<AudioSnap> {
        let ctl = self.control();
        if ctl.closed {
            return None;
        }
        Some(AudioSnap {
            plan: ctl.plan.clone(),
            plan_gen: ctl.plan_gen,
            mode: ctl.mode,
            trims: ctl.trims.clone(),
            trims_gen: ctl.trims_gen,
            muted: ctl.muted,
            scrub: ctl.scrub,
            looping: ctl.looping,
            changes: ctl.changes,
        })
    }

    pub(crate) fn wait_change(&self, version: u64, timeout: Duration) {
        let ctl = self.control();
        if ctl.version == version && !ctl.closed {
            let _ = self.changed.wait_timeout(ctl, timeout);
        }
    }

    pub(crate) fn wait_changes(&self, changes: u64, timeout: Duration) {
        let ctl = self.control();
        if ctl.changes == changes && !ctl.closed {
            let _ = self.changed.wait_timeout(ctl, timeout);
        }
    }

    pub(crate) fn set_busy(&self, busy: Option<Busy>) {
        Self::busy_as(&self.busy, busy);
    }

    pub(crate) fn set_window_busy(&self, busy: Option<Busy>) {
        Self::busy_as(&self.window_busy, busy);
    }

    fn busy_as(b: &Mutex<Option<(Busy, Instant)>>, busy: Option<Busy>) {
        let mut b = lock(b);
        *b = match (busy, *b) {
            (Some(kind), Some((was, since))) if kind == was => Some((kind, since)),
            (Some(kind), _) => Some((kind, Instant::now())),
            (None, _) => None,
        };
    }

    pub(crate) fn fail(&self, why: String) {
        self.control().error = Some(why);
    }

    pub(crate) fn failed(&self, frame: i64) {
        *lock(&self.failed) = Some(frame);
        if let Some(f) = &self.on_picture {
            f();
        }
    }

    pub(crate) fn decodes_on_cpu(&self, why: Option<&'static str>) {
        *lock(&self.cpu) = why;
    }

    pub(crate) fn landed(&self, plan_gen: u64, id: plan::Id) {
        {
            let mut landed = lock(&self.landed);
            *landed = (landed.0 + 1, plan_gen, Some(id));
        }
        lock(&self.failed).take();
        if let Some(f) = &self.on_picture {
            f();
        }
    }

    pub(crate) fn level(&self, level: Level) {
        let mut levels = lock(&self.levels);
        if levels.len() == LEVELS_KEPT {
            levels.pop_front();
        }
        levels.push_back(level);
    }

    pub(crate) fn forget_levels(&self) {
        lock(&self.levels).clear();
    }

    pub(crate) fn gpu_found(&self, gpu: &crate::gpus::Gpu) {
        if self.gpu_memory.get().is_none()
            && let Some(mb) = gpu.memory_mb
        {
            let _ = self.gpu_memory.set(mb);
            self.apply_video_budget();
        }
    }

    fn apply_video_budget(&self) {
        if !self.video_auto.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if SHARED_MEMORY {
            self.store.share(true);
        } else {
            let bytes = Budget::gpu_video(self.gpu_memory.get().copied());
            self.store.set_budget(store::Kind::Video, bytes);
        }
    }

    pub(crate) fn prepared(&self) {
        if let Some(f) = &self.on_picture {
            f();
        }
    }

    pub(crate) fn drew_titles(&self) {
        self.prepared();
    }
}

pub struct Player {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    output: Mutex<Option<Box<dyn AudioOutput>>>,
}

impl Player {
    pub fn new(plan: RenderPlan, config: Config, mut output: Box<dyn AudioOutput>) -> Result<Self> {
        crate::init();
        if plan.frames <= 0 {
            return Err(Error::Invalid("nothing to play".into()));
        }
        let (rate, channels) = (plan.sample_rate, plan.channels);
        let base = Instant::now();
        let ring = Arc::new(Ring::new(rate as usize, channels));
        let heard = Arc::new(Heard::new(base, rate));
        let source = AudioSource::new(ring.clone(), heard.clone(), rate, channels);
        let device = source.failure.clone();
        let at = plan.range.0;
        let shared = Arc::new(Shared {
            id: PLAYERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1,
            control: Mutex::new(Control {
                plan: Arc::new(Plan::new(plan)),
                plan_gen: 0,
                mode: Mode::Paused { at },
                version: 0,
                changes: 0,
                epoch: 0,
                epoch_at: Instant::now(),
                trims: Vec::new(),
                trims_gen: 0,
                muted: false,
                scrubbing: config.audio_scrubbing,
                scrub: None,
                looping: None,
                hardware: config.hardware,
                motion: 0.0,
                followed: false,
                small: None,
                prepared: None,
                closed: false,
                error: None,
            }),
            changed: Condvar::new(),
            store: Store::new(config.budget.memory, config.budget.video.unwrap_or(GB)),
            ring,
            heard,
            markers: Markers::default(),
            interrupt: Arc::new(AtomicBool::new(false)),
            busy: Mutex::new(None),
            windows: Mutex::new(None),
            window_interrupt: Arc::new(AtomicBool::new(false)),
            window_busy: Mutex::new(None),
            failed: Mutex::new(None),
            landed: Mutex::new((0, 0, None)),
            cpu: Mutex::new(None),
            on_picture: config.on_picture,
            presenter: Mutex::new(Presenter::default()),
            device,
            format: (rate, channels),
            gpu_pictures: config.gpu_pictures,
            copies: Arc::default(),
            video_auto: AtomicBool::new(config.budget.video.is_none()),
            gpu_memory: std::sync::OnceLock::new(),
            heard_epoch: std::sync::atomic::AtomicU64::new(0),
            levels: Mutex::new(std::collections::VecDeque::new()),
            titles: titles::Titles::default(),
            subtitles: subtitles::Subtitles::default(),
            placing: Mutex::new(Vec::new()),
        });
        shared.apply_video_budget();
        output.start(source)?;
        let mut player = Self {
            shared: shared.clone(),
            threads: Vec::new(),
            output: Mutex::new(Some(output)),
        };
        let s = shared.clone();
        player.threads.push(
            std::thread::Builder::new()
                .name("vidcut-play-video".into())
                .spawn(move || video::run(s))
                .map_err(Error::Io)?,
        );
        player.threads.push(
            std::thread::Builder::new()
                .name("vidcut-play-audio".into())
                .spawn(move || audio::run(shared))
                .map_err(Error::Io)?,
        );
        Ok(player)
    }

    pub fn set_plan(&self, plan: RenderPlan) -> Result<()> {
        self.check(&plan)?;
        if self.shared.control().plan.plan == plan {
            return Ok(());
        }
        self.shared.change(true, |ctl, pos, _| {
            replace_plan(ctl, plan);
            if let Mode::Paused { .. } = ctl.mode {
                ctl.mode = Mode::Paused {
                    at: ctl.plan.clamp(pos),
                };
            }
        });
        Ok(())
    }

    pub fn set_plan_at(&self, plan: RenderPlan, at: i64) -> Result<()> {
        self.check(&plan)?;
        let same = self.shared.control().plan.plan == plan;
        self.shared.change(true, |ctl, _, _| {
            if !same {
                replace_plan(ctl, plan);
            }
            move_to(ctl, at, false);
        });
        Ok(())
    }

    fn check(&self, plan: &RenderPlan) -> Result<()> {
        if plan.frames <= 0 {
            return Err(Error::Invalid("nothing to play".into()));
        }
        let (rate, channels) = self.shared.format;
        if (plan.sample_rate, plan.channels) != (rate, channels) {
            return Err(Error::Invalid(format!(
                "the player plays {rate} Hz in {channels} channels, not {} Hz in {}",
                plan.sample_rate, plan.channels
            )));
        }
        Ok(())
    }

    pub fn prepare(&self, next: Option<(RenderPlan, i64)>) {
        let next = next.filter(|(plan, _)| plan.frames > 0).map(|(plan, at)| {
            let plan = Plan::new(plan);
            let at = plan.clamp(at);
            (Arc::new(plan), at)
        });
        self.shared
            .store
            .hold(next.as_ref().map(|(plan, at)| plan.id(*at)));
        let mut ctl = self.shared.control();
        ctl.prepared = next;
        ctl.version += 1;
        self.shared.notify(ctl);
    }

    pub fn seek(&self, frame: i64) {
        self.seek_with(frame, true);
    }

    pub fn seek_quietly(&self, frame: i64) {
        self.seek_with(frame, false);
    }

    pub fn follow(&self, frame: i64, speed: f64) {
        self.shared.change(true, |ctl, _, _| {
            move_to(ctl, frame, false);
            ctl.motion = speed;
            ctl.followed = true;
        });
    }

    pub fn shrink_to(&self, size: Option<(u32, u32)>) {
        let near = |a: u32, b: u32| a.abs_diff(b) * 8 <= b;
        let mut ctl = self.shared.control();
        let same = match (ctl.small, size) {
            (Some((w0, h0)), Some((w, h))) => near(w, w0) && near(h, h0),
            (was, now) => was == now,
        };
        if same {
            return;
        }
        ctl.small = size;
        ctl.version += 1;
        ctl.changes += 1;
        drop(ctl);
        self.shared.store.forget_small();
        self.shared.changed.notify_all();
    }

    fn seek_with(&self, frame: i64, sound: bool) {
        self.shared
            .change(true, |ctl, _, _| move_to(ctl, frame, sound));
    }

    pub fn step(&self, frames: i64) {
        self.shared.change(true, |ctl, pos, _| {
            let t = ctl.plan.clamp(pos.saturating_add(frames));
            ctl.scrub(t);
            ctl.motion = 0.0;
            ctl.followed = false;
            ctl.mode = Mode::Paused { at: t };
        });
    }

    pub fn set_audio_scrubbing(&self, on: bool) {
        self.shared.change(false, |ctl, _, _| {
            ctl.scrubbing = on;
            if !on {
                ctl.scrub = None;
            }
        });
    }

    pub fn set_loop(&self, range: Option<(i64, i64)>) {
        let range = range.filter(|(a, b)| b > a);
        if self.shared.control().looping == range {
            return;
        }
        self.shared.change(false, |ctl, _, _| ctl.looping = range);
    }

    pub fn play(&self, speed: f64) {
        if speed == 0.0 || !speed.is_finite() {
            return self.pause();
        }
        self.shared.change(true, |ctl, pos, sample| {
            let from = match (ctl.mode, sample) {
                (Mode::Playing { .. }, Some(s)) => s.round() as i64,
                _ => {
                    let at = match ctl.mode {
                        Mode::Paused { at } if speed > 0.0 && at >= ctl.plan.last() => {
                            ctl.plan.first()
                        }
                        Mode::Paused { at } if speed < 0.0 && at <= ctl.plan.first() => {
                            ctl.plan.last()
                        }
                        _ => pos,
                    };
                    start_sample(&ctl.plan, at, speed)
                }
            };
            ctl.epoch += 1;
            ctl.epoch_at = Instant::now();
            ctl.motion = 0.0;
            ctl.followed = false;
            ctl.mode = Mode::Playing {
                from,
                speed,
                epoch: ctl.epoch,
            };
        });
    }

    pub fn pause(&self) {
        self.shared.change(true, |ctl, pos, _| {
            ctl.mode = Mode::Paused { at: pos };
            ctl.motion = 0.0;
            ctl.followed = false;
        });
    }

    pub fn set_lane_gain(&self, lane: usize, db: f64) {
        self.shared.change(false, |ctl, _, _| {
            if ctl.trims.len() <= lane {
                ctl.trims.resize(lane + 1, 0.0);
            }
            ctl.trims[lane] = db;
            ctl.trims_gen += 1;
        });
    }

    pub fn set_hardware(&self, on: bool) {
        if self.shared.control().hardware == on {
            return;
        }
        self.shared.change(true, |ctl, _, _| ctl.hardware = on);
    }

    pub fn set_budget(&self, budget: Budget) {
        let store = &self.shared.store;
        store.set_budget(store::Kind::Memory, budget.memory);
        self.shared
            .video_auto
            .store(budget.video.is_none(), std::sync::atomic::Ordering::Relaxed);
        match budget.video {
            Some(bytes) => {
                store.set_budget(store::Kind::Video, bytes);
                store.share(false);
            }
            None => self.shared.apply_video_budget(),
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.change(false, |ctl, _, _| {
            ctl.muted = muted;
            ctl.trims_gen += 1;
        });
    }

    pub fn present(&self, now: Instant) -> Option<Shown> {
        let (id, pos, speed, clock, plan_gen, plan) = {
            let mut ctl = self.shared.control();
            let pos = self.shared.position(&mut ctl, now);
            let (speed, clock) = match ctl.mode {
                Mode::Playing { speed, epoch, .. } => {
                    (speed, self.shared.clock_sample(epoch, now).map(|_| epoch))
                }
                Mode::Paused { .. } => (0.0, None),
            };
            (
                ctl.plan.id(pos),
                pos,
                speed,
                clock,
                ctl.plan_gen,
                ctl.plan.clone(),
            )
        };
        let placing = lock(&self.shared.placing).clone();
        let placed = plan.placed_at(pos, &placing);
        let crosses = plan.crosses_at(pos);
        let scales = plan.scales_at(pos);
        let colors = plan.plan.colors;
        let (mut titles, titled) = self.shared.titles.shown(&plan, pos, &placing);
        let (cues, cued) = self.shared.subtitles.shown(&plan, pos);
        titles.extend(cues);
        let drawn = titled && cued;
        let picture = self.shared.store.get(&id);
        let landed = lock(&self.shared.landed).clone();
        let mut p = lock(&self.shared.presenter);
        match picture {
            Some(picture) => {
                p.shows(clock.filter(|_| speed.abs() <= 1.0), pos);
                if let Some(d) = picture.decoder() {
                    p.decoder = Some(d);
                }
                let shown = Shown {
                    frame: pos,
                    picture,
                    exact: drawn,
                    titles,
                    placed,
                    crosses,
                    scales,
                    colors,
                };
                p.shown = Some(shown.clone());
                p.shown_landing = landed.0;
                Some(shown)
            }
            None => {
                p.missed(clock.is_some(), pos);
                if let (n, g, Some(newer)) = landed
                    && n > p.shown_landing
                    && g == plan_gen
                    && let Some(picture) = self.shared.store.get(&newer)
                {
                    p.shown_landing = n;
                    p.shown = Some(Shown {
                        frame: pos,
                        picture,
                        exact: false,
                        titles: titles.clone(),
                        placed: placed.clone(),
                        crosses: crosses.clone(),
                        scales: scales.clone(),
                        colors,
                    });
                }
                p.shown.clone().map(|s| Shown {
                    frame: pos,
                    exact: false,
                    titles,
                    placed,
                    crosses,
                    scales,
                    colors,
                    ..s
                })
            }
        }
    }

    pub fn stats(&self) -> Stats {
        let (plan, position) = {
            let mut ctl = self.shared.control();
            let position = self.shared.position(&mut ctl, Instant::now());
            (ctl.plan.clone(), position)
        };
        let store = &self.shared.store;
        let reach = (plan.fps().as_f64() * 60.0).ceil() as i64;
        let ahead = (position + 1..=plan.clamp(position + reach)).map(|t| plan.id(t));
        let behind = (plan.clamp(position - reach)..position)
            .rev()
            .map(|t| plan.id(t));
        let (shown, frames, late, skipped) = {
            let p = lock(&self.shared.presenter);
            (p.shown.clone(), p.frames, p.late, p.skipped)
        };
        let (rate, channels) = self.shared.format;
        Stats {
            position,
            shown,
            super_scales: plan.super_scales_at(position),
            frames,
            late,
            skipped,
            ahead: store.kept_run(ahead),
            behind: store.kept_run(behind),
            kept: store.usage().0,
            memory: (
                store.bytes(store::Kind::Memory),
                store.budget(store::Kind::Memory),
            ),
            video: (
                store.bytes(store::Kind::Video),
                store.budget(store::Kind::Video),
            ),
            shared: store.shared(),
            cpu: *lock(&self.shared.cpu),
            plays_on: self.plays_on(),
            rate,
            channels,
            latency: self.shared.heard.latency(),
            copies: self.shared.copies.counted(),
        }
    }

    pub fn copies(&self) -> Arc<crate::copies::Copies> {
        self.shared.copies.clone()
    }

    pub fn plays_on(&self) -> PlaysOn {
        lock(&self.output)
            .as_ref()
            .map_or(PlaysOn::Default, |o| o.plays_on())
    }

    pub fn levels(&self, now: Instant) -> Option<Levels> {
        if !matches!(self.shared.control().mode, Mode::Playing { .. }) {
            return None;
        }
        let counter = self.shared.heard.counter_at(now)?;
        let (rate, channels) = self.shared.format;
        let ch = usize::from(channels).max(1);
        let from = counter.saturating_sub((f64::from(rate) * LEVELS_SECS) as u64);
        let levels = lock(&self.shared.levels);
        let mut heard = levels
            .iter()
            .filter(|l| l.at < counter && l.at + l.frames > from)
            .peekable();
        heard.peek()?;
        let mut mix = vec![0.0f32; ch];
        let mut lanes: Vec<Vec<f32>> = Vec::new();
        for l in heard {
            for (m, p) in mix.iter_mut().zip(&l.mix) {
                *m = m.max(*p);
            }
            for (i, peaks) in l.lanes.chunks(ch).enumerate() {
                if lanes.len() <= i {
                    lanes.push(vec![0.0; ch]);
                }
                for (a, b) in lanes[i].iter_mut().zip(peaks) {
                    *a = a.max(*b);
                }
            }
        }
        Some(Levels { mix, lanes })
    }

    pub fn status(&self) -> Status {
        let (id, prepared, position, mode, range, error, glide) = {
            let mut ctl = self.shared.control();
            let now = Instant::now();
            let position = self.shared.position(&mut ctl, now);
            let glide = self.shared.glide(&ctl, now);
            let plan = &ctl.plan.plan;
            (
                ctl.plan.id(position),
                ctl.prepared.as_ref().map(|(p, t)| p.id(*t)),
                position,
                ctl.mode,
                (plan.range.0, plan.range.0 + plan.frames),
                ctl.error.clone(),
                glide,
            )
        };
        let device = lock(&self.shared.device).clone();
        let p = lock(&self.shared.presenter);
        let (playing, speed) = match mode {
            Mode::Playing { speed, .. } => (true, speed),
            Mode::Paused { .. } => (false, 0.0),
        };
        Status {
            position,
            playing,
            speed,
            ready: self.shared.store.contains(&id),
            prepared: prepared.is_some_and(|id| self.shared.store.contains(&id)),
            presented: p.presented,
            range,
            late: p.late,
            skipped: p.skipped,
            kept: self.shared.store.usage(),
            latency: self.shared.heard.latency(),
            decoder: p.decoder,
            failed: *lock(&self.shared.failed),
            error: device.or(error),
            glide,
        }
    }

    pub fn preview_pictures(&self, pictures: Vec<(String, vidcut_core::model::Picture)>) {
        *lock(&self.shared.placing) = pictures;
    }

    pub fn preview_title(&self, title: Option<(String, Title)>) {
        if self.shared.titles.preview(title) {
            self.shared.prepared();
            self.shared.changed.notify_all();
        }
    }

    pub fn set_subtitles(&self, view: Option<SubtitleView>) {
        if self.shared.subtitles.set(view) {
            self.shared.prepared();
            self.shared.changed.notify_all();
        }
    }

    pub fn wait_ready(&self, frame: i64, timeout: Duration) -> bool {
        let id = self.shared.control().plan.id(frame);
        self.shared
            .store
            .wait(&id, Instant::now() + timeout)
            .is_some()
    }

    pub fn frame_at(&self, at: Instant) -> i64 {
        let ctl = self.shared.control();
        self.shared.clock_frame(&ctl, at).0
    }

    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        let ctl = self.shared.control();
        let Mode::Playing { speed, epoch, .. } = ctl.mode else {
            return None;
        };
        let plan = &ctl.plan;
        let s = self.shared.clock_sample(epoch, now)?;
        let t = plan.frame_at_sample(s.floor() as i64);
        let next = if speed > 0.0 {
            plan.sample(t + 1) as f64
        } else {
            plan.sample(t) as f64 - 1.0
        };
        let secs = (next - s).abs() / (f64::from(plan.rate()) * speed.abs());
        Some(Duration::from_secs_f64(secs.clamp(0.0, 1.0)))
    }
}

fn replace_plan(ctl: &mut Control, plan: RenderPlan) {
    ctl.plan = Arc::new(Plan::new(plan));
    ctl.plan_gen += 1;
    ctl.error = None;
    ctl.trims.clear();
    ctl.trims_gen += 1;
}

fn move_to(ctl: &mut Control, frame: i64, sound: bool) {
    let t = ctl.plan.clamp(frame);
    ctl.motion = 0.0;
    ctl.followed = false;
    ctl.mode = match ctl.mode {
        Mode::Paused { .. } => {
            if sound {
                ctl.scrub(t);
            }
            Mode::Paused { at: t }
        }
        Mode::Playing { speed, .. } => {
            ctl.epoch += 1;
            ctl.epoch_at = Instant::now();
            Mode::Playing {
                from: start_sample(&ctl.plan, t, speed),
                speed,
                epoch: ctl.epoch,
            }
        }
    };
}

fn start_sample(plan: &Plan, t: i64, speed: f64) -> i64 {
    if speed > 0.0 {
        plan.sample(t)
    } else {
        (plan.sample(t + 1) - 1).max(0)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let started = Instant::now();
        {
            let mut ctl = self.shared.control();
            ctl.closed = true;
            ctl.version += 1;
            ctl.changes += 1;
        }
        for flag in [&self.shared.interrupt, &self.shared.window_interrupt] {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.shared.changed.notify_all();
        self.shared.store.wake();
        let mut joins = [Duration::ZERO; 2];
        for (i, t) in self.threads.drain(..).enumerate() {
            let at = Instant::now();
            let _ = t.join();
            if let Some(j) = joins.get_mut(i) {
                *j = at.elapsed();
            }
        }
        if let Some(windows) = lock(&self.shared.windows).take() {
            crate::ending::leave(windows);
        }
        let (pictures, bytes) = self.shared.store.usage();
        let at = Instant::now();
        self.shared.store.clear();
        let freed = at.elapsed();
        let at = Instant::now();
        lock(&self.output).take();
        crate::debug!(
            "player",
            "player {} ended in {:.0} ms: the decode thread joined in {:.0} ms, the audio thread \
             in {:.0} ms, {pictures} kept pictures ({} MB) freed in {:.0} ms, the audio output \
             stopped in {:.0} ms",
            self.shared.id,
            ms(started.elapsed()),
            ms(joins[0]),
            ms(joins[1]),
            bytes >> 20,
            ms(freed),
            ms(at.elapsed())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ManualOutput, Player, Presenter};
    use crate::ffmpeg::{format::Pixel, frame};
    use vidcut_core::editor::Copied;
    use vidcut_core::render::{RenderPlan, VideoPiece};
    use vidcut_core::time::Rational;

    #[test]
    fn a_frame_copied_down_for_a_player_shows_in_its_stats() {
        let plan = RenderPlan {
            fps: Rational::new(30, 1).unwrap(),
            width: 64,
            height: 36,
            sample_rate: 48_000,
            channels: 2,
            frames: 1,
            video: vec![VideoPiece::Black { frames: 1 }],
            audio: Vec::new(),
            ducking: Vec::new(),
            range: (0, 1),
            name: "copies".into(),
            colors: None,
        };
        let config = Config {
            audio_scrubbing: false,
            ..Config::default()
        };
        let player = Player::new(plan, config, Box::new(ManualOutput::new())).unwrap();
        {
            let _counting = crate::copies::Guard::set(player.copies());
            crate::copies::down(&frame::Video::new(Pixel::YUV422P10LE, 3840, 2160));
        }
        let want = Copied {
            down: 1,
            down_bytes: 33_177_600,
            up: 0,
            up_bytes: 0,
        };
        assert_eq!(player.stats().copies, want);
    }

    #[test]
    fn a_late_frame_counts_once() {
        let mut p = Presenter::default();
        for _ in 0..5 {
            p.missed(true, 40);
        }
        p.missed(true, 41);
        assert_eq!(p.late, 2);
    }

    #[test]
    fn a_seek_skips_no_frames() {
        let mut p = Presenter::default();
        p.shows(Some(1), 40);
        p.shows(Some(2), 50);
        assert_eq!(p.skipped, 0);
    }

    #[test]
    fn frames_the_running_clock_passes_are_skipped() {
        let mut p = Presenter::default();
        p.shows(Some(1), 40);
        p.shows(Some(1), 43);
        assert_eq!(p.skipped, 2);
    }

    #[test]
    fn nothing_is_late_before_the_clock_runs() {
        let mut p = Presenter::default();
        p.missed(false, 40);
        assert_eq!(p.late, 0);
    }
}
