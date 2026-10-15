//! vidcut's own player: a render plan played with the FFmpeg libraries, every
//! picture the one export makes, the sound the clock
//! (docs/decisions/player.md).
//!
//! **Threads.** A player has a decode thread ([`video`]) and an audio
//! thread ([`audio`]); the audio output has its device's callback. The
//! caller never decodes: [`Player`]'s requests change what is wanted and
//! return at once, and [`Player::present`] only looks pictures up.
//!
//! **Requests merge.** The threads read the newest wanted state, not a
//! queue, so seeks that arrive while one decodes collapse into the last.
//! A decode the newest request no longer needs is cancelled: when it was
//! only decoding ahead or the clock runs, and otherwise once it has run a
//! second. A seek otherwise finishes, paused or playing before the sound
//! from there is heard, so a held key or a dragged playhead moves the
//! picture at the speed frames decode.
//!
//! **Pictures** are picked by the export's reader (the sampling rule) and
//! kept as decoded, in YUV, for a display's shader to fit and convert
//! ([`frame`]); kept by what they show within a memory budget ([`store`]):
//! ahead of the clock in the direction of play, those just shown, and
//! windows decoded from a keyframe for going back.
//!
//! **Clock.** The audio thread mixes the plan's lanes into a ring the
//! device's callback reads; the callback records which samples it took and
//! when they are heard ([`clock`]). The position is the sample heard now,
//! so pictures follow the sound as the listener hears it, output latency
//! included, and stop when the sound stops. Backward, the sound plays
//! reversed.
//!
//! **Scrubbing.** A frame step, and a seek while paused, play that frame's
//! sound ([`scrub`]); [`Scrubber`] does it alone, for pictures shown by
//! something else.
//!
//! **The plan played next.** A viewer that shows one thing or another (the
//! source or the timeline) plays both through one player: the frame it
//! would show of the other is [`prepare`](Player::prepare)d, decoded while
//! nothing else is and held whatever the budget, so switching to it
//! ([`set_plan_at`](Player::set_plan_at)) shows it at once.

mod audio;
mod clock;
mod frame;
mod output;
mod plan;
mod ring;
mod scrub;
mod store;
mod video;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use vidcut_core::render::RenderPlan;

pub use frame::{Layout, Over, Picture, Shown};
pub use output::{
    AudioOutput, AudioSource, CpalOutput, ManualOutput, OutputDevice, PacedOutput, PlaysOn,
    Recording, output_device, output_devices,
};
pub use scrub::Scrubber;

use crate::error::{Error, Result};
use clock::{Heard, Markers};
use plan::Plan;
use ring::Ring;
use store::Store;

/// Memory for kept pictures: in memory, and in video memory (pictures NVDEC
/// decoded for a display that shows them from there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub memory: usize,
    /// `None`: half the memory of the GPU that decodes them (1 GB until it
    /// is known).
    pub video: Option<usize>,
}

/// 1 GB of each, for players that don't ask for more (tests, tools).
pub const DEFAULT_BUDGET: Budget = Budget::both(GB);

const GB: usize = 1 << 30;

impl Budget {
    /// `bytes` of each.
    pub const fn both(bytes: usize) -> Self {
        Self {
            memory: bytes,
            video: Some(bytes),
        }
    }

    /// What a viewer keeps unless told otherwise: memory is there to be
    /// used, and the more pictures stay around the playhead the less it
    /// waits. The larger of 1 GB and half the computer's memory, and half
    /// the GPU's.
    pub fn automatic() -> Self {
        Self {
            memory: system_memory().map_or(GB, |m| (m / 2).max(GB)),
            video: None,
        }
    }
}

/// The computer's memory in bytes, where it can be read.
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

/// How a player works.
#[derive(Clone)]
pub struct Config {
    /// Memory for kept pictures.
    pub budget: Budget,
    /// Frame steps, and seeks while paused, play the frame's sound (spec
    /// §4.5, decision D42). On unless set otherwise.
    pub audio_scrubbing: bool,
    /// Called on the decode thread when the picture of the frame the viewer
    /// is at lands (a display repaints), and when the prepared one does.
    pub on_picture: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Decode on the GPU the streams it decodes (NVDEC, VA-API, Direct3D
    /// 11, on the GPU in use: [`crate::hw::GpuDecoder::best`]), the frames
    /// copied down to memory at their own size
    /// ([`set_hardware`](Player::set_hardware)).
    pub hardware: bool,
    /// The decoders whose pictures stay in GPU memory (NV12, P010), for a
    /// display that shows them from there; kept ones take video memory
    /// then.
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

/// What a player does, for a person who wants to know (stats for nerds).
#[derive(Clone)]
pub struct Stats {
    /// The frame the viewer is at, and the picture shown last.
    pub position: i64,
    pub shown: Option<Shown>,
    /// Frames shown exactly; while playing, frames late and frames passed
    /// over ([`Status`]).
    pub frames: u64,
    pub late: u64,
    pub skipped: u64,
    /// Pictures kept right after the frame the viewer is at and right
    /// before it, a minute each way at most, and all of them.
    pub ahead: usize,
    pub behind: usize,
    pub kept: usize,
    /// Bytes kept in memory and in video memory, each with its budget.
    pub memory: (usize, usize),
    pub video: (usize, usize),
    /// Why the player decodes on the CPU, when it chose so: GPU decoding is
    /// off, or there is no GPU decoder. (A file a GPU won't decode says why
    /// in its picture's [`decoding`](Picture::decoding).)
    pub cpu: Option<&'static str>,
    /// The sound: the device, its rate and channels, and its latency.
    pub plays_on: PlaysOn,
    pub rate: u32,
    pub channels: u16,
    pub latency: Option<Duration>,
    /// Frames copied between GPU memory and memory since it started: down
    /// by its decoder and by a viewer showing it, up by the viewer
    /// ([`Player::copies`]). A switch of decoder keeps counting.
    pub copies: vidcut_core::editor::Copied,
}

/// What a player is doing.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// The timeline frame the viewer is at: where the clock is while
    /// playing.
    pub position: i64,
    pub playing: bool,
    /// Frames of timeline per frame of time; negative backwards, 0 paused.
    pub speed: f64,
    /// The picture of `position` is decoded: the next
    /// [`present`](Player::present) shows it.
    pub ready: bool,
    /// The picture of the frame [`prepare`](Player::prepare)d is decoded
    /// (false while none is): switching to it shows it at once.
    pub prepared: bool,
    /// The newest frame [`present`](Player::present) showed exactly.
    pub presented: Option<i64>,
    /// The plan's timeline frames, `[first, end)`.
    pub range: (i64, i64),
    /// While the clock runs: frames it reached before their pictures were
    /// decoded, each counted once.
    pub late: u64,
    /// While the clock runs at up to normal speed: frames it passed between
    /// two presents of one play request without being shown.
    pub skipped: u64,
    /// Pictures kept, and the memory they take.
    pub kept: (usize, usize),
    /// How long before it is heard the audio device takes the sound: its
    /// output latency, which the clock makes up for.
    pub latency: Option<Duration>,
    /// What decoded the picture of a file shown last.
    pub decoder: Option<Decoder>,
    /// The frame the viewer is at, when its picture failed to decode (it
    /// isn't tried again until something changes); `error` says why.
    pub failed: Option<i64>,
    /// The newest decoding or device failure.
    pub error: Option<String>,
}

/// The peaks of the sound heard now, while playing, linear per channel:
/// what the meters show of the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    /// The mix. The monitoring mute silences only the viewer's meter
    /// (spec D28), which the GUI does.
    pub mix: Vec<f32>,
    /// Each of the plan's audio lanes, in order, with its gain on top.
    pub lanes: Vec<Vec<f32>>,
}

/// What decodes a player's pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoder {
    /// The CPU, FFmpeg's software decoders.
    Cpu,
    /// A GPU ([`crate::hw`]): NVIDIA's NVDEC, whose frames may stay in GPU
    /// memory, or VA-API or Direct3D 11, whose frames come down to memory.
    Gpu(crate::hw::Kind),
}

impl Decoder {
    /// A short name: `cpu`, or the GPU decoder's (`nvdec`, `vaapi`,
    /// `d3d11va`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu(kind) => kind.name(),
        }
    }
}

/// What is wanted now, read by the decode and audio threads.
pub(crate) struct Control {
    pub plan: Arc<Plan>,
    /// Counts plan changes.
    pub plan_gen: u64,
    pub mode: Mode,
    /// Counts every change the decode thread cares about.
    pub version: u64,
    /// Counts play requests, and when the latest was made.
    epoch: u64,
    epoch_at: Instant,
    /// Lane gains on top of the plan's (faders being moved), dB.
    pub trims: Vec<f64>,
    pub trims_gen: u64,
    pub muted: bool,
    /// Steps and paused seeks play the frame's sound.
    scrubbing: bool,
    /// The frame whose sound to play, numbered so each request is new.
    pub scrub: Option<(i64, u64)>,
    /// Decode on NVDEC where it can.
    pub hardware: bool,
    /// The plan played next and the frame of it to keep ready.
    pub prepared: Option<(Arc<Plan>, i64)>,
    pub closed: bool,
    error: Option<String>,
}

impl Control {
    /// Asks for the sound of frame `t`, when scrubbing.
    fn scrub(&mut self, t: i64) {
        if self.scrubbing {
            let n = self.scrub.map_or(0, |(_, n)| n) + 1;
            self.scrub = Some((t, n));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    /// Showing timeline frame `at`.
    Paused { at: i64 },
    /// Playing from sample `from` (counted from the plan's start) at
    /// `speed`; `epoch` tells this request's sound from older ones'.
    Playing { from: i64, speed: f64, epoch: u64 },
}

/// What the decode thread is busy with, and since when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Busy {
    /// The picture of the frame the viewer is at.
    Focus,
    /// Pictures ahead of it.
    Ahead,
    /// A window behind it, timeline frames `[from, to]` of the plan of
    /// change `plan_gen`: a step back into it waits for it to land there.
    Window { from: i64, to: i64, plan_gen: u64 },
}

#[derive(Default)]
struct Presenter {
    shown: Option<Shown>,
    /// The latest landing (`Shared::landed`) when the shown picture was
    /// presented.
    shown_landing: u64,
    presented: Option<i64>,
    /// The play request whose running clock last showed a frame exactly.
    presented_in: Option<u64>,
    /// Frames shown exactly, each counted once in a row.
    frames: u64,
    /// What decoded the picture of a file shown last.
    decoder: Option<Decoder>,
    late: u64,
    /// The frame counted late last.
    late_at: Option<i64>,
    skipped: u64,
}

impl Presenter {
    /// The clock is at `pos`, and its picture is shown. `clock`: the play
    /// request whose clock runs at up to normal speed. The frames it passed
    /// since it showed one before were skipped. A seek or a scrub starts
    /// another request, so its jump isn't.
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

    /// The clock is at `pos`, and its picture isn't decoded. Late once the
    /// clock runs, each frame once however often the viewer looks.
    fn missed(&mut self, runs: bool, pos: i64) {
        if runs && self.late_at != Some(pos) {
            self.late += 1;
            self.late_at = Some(pos);
        }
    }
}

/// The decode thread's view of what is wanted.
pub(crate) struct Snap {
    pub plan: Arc<Plan>,
    pub plan_gen: u64,
    pub mode: Mode,
    pub version: u64,
    /// The frame the viewer is at.
    pub focus: i64,
    /// Playing, the sound of the latest play request is heard, and the
    /// clock moves. Until it is, the clock stands where play starts.
    pub heard: bool,
    /// Decode on NVDEC where it can.
    pub hardware: bool,
    /// The plan played next and the frame of it to keep ready.
    pub prepared: Option<(Arc<Plan>, i64)>,
}

/// The audio thread's view.
pub(crate) struct AudioSnap {
    pub plan: Arc<Plan>,
    pub plan_gen: u64,
    pub mode: Mode,
    pub trims: Vec<f64>,
    pub trims_gen: u64,
    pub muted: bool,
    pub scrub: Option<(i64, u64)>,
}

pub(crate) struct Shared {
    control: Mutex<Control>,
    /// Something wanted changed.
    changed: Condvar,
    pub(crate) store: Store,
    pub(crate) ring: Arc<Ring>,
    pub(crate) heard: Arc<Heard>,
    pub(crate) markers: Markers,
    /// The decode thread's cancel flag (`crate::cancel`).
    pub(crate) interrupt: Arc<AtomicBool>,
    busy: Mutex<Option<(Busy, Instant)>>,
    /// The frame whose picture failed to decode last, until one lands.
    failed: Mutex<Option<i64>>,
    /// The picture of the frame the viewer was at that landed last, by
    /// the plan's change: the landings counted, the change, its id.
    landed: Mutex<(u64, u64, Option<plan::Id>)>,
    /// Why the decode thread's reader decodes on the CPU, when it chose so.
    cpu: Mutex<Option<&'static str>>,
    on_picture: Option<Arc<dyn Fn() + Send + Sync>>,
    presenter: Mutex<Presenter>,
    /// The audio device's failure.
    device: Arc<Mutex<Option<String>>>,
    /// The sample rate and channels the device was opened for.
    format: (u32, u16),
    /// The decoders whose pictures stay in GPU memory.
    pub(crate) gpu_pictures: Vec<crate::hw::Kind>,
    /// The frames copied between GPU memory and memory since it started
    /// (`Stats::copies`): the decode thread counts here.
    copies: Arc<crate::copies::Copies>,
    /// The video memory budget is half the GPU's.
    video_auto: AtomicBool,
    /// The GPU's memory, once the decode thread opened it.
    gpu_memory: std::sync::OnceLock<usize>,
    /// The latest play request whose sound was heard, for `VIDCUT_DEBUG`.
    heard_epoch: std::sync::atomic::AtomicU64,
    /// The peaks of the sound queued lately, oldest first ([`Level`]).
    levels: Mutex<std::collections::VecDeque<Level>>,
}

/// The peaks of a run of sound queued for the device, from ring frame `at`
/// on: the mix's per channel, and each lane's, `channels` a lane.
pub(crate) struct Level {
    pub at: u64,
    pub frames: u64,
    pub mix: Vec<f32>,
    pub lanes: Vec<f32>,
}

/// Runs of sound whose peaks are kept: well over a second of 10 ms chunks,
/// more than the device holds queued.
const LEVELS_KEPT: usize = 160;

/// How far back the levels heard now reach.
const LEVELS_SECS: f64 = 0.05;

/// A seek that has decoded this long holds a newer one up no more.
const PATIENCE: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn control(&self) -> MutexGuard<'_, Control> {
        lock(&self.control)
    }

    /// The timeline sample heard now in play request `epoch`.
    fn clock_sample(&self, epoch: u64, now: Instant) -> Option<f64> {
        let counter = self.heard.counter_at(now)?;
        self.markers.sample_at(counter, epoch)
    }

    /// The frame the viewer is at. A play that has reached the end of the
    /// plan (forward) or its start (backward) pauses there.
    fn position(&self, ctl: &mut Control, now: Instant) -> i64 {
        let (t, ended) = self.clock_frame(ctl, now);
        if ended {
            ctl.mode = Mode::Paused { at: t };
            ctl.version += 1;
            self.changed.notify_all();
        }
        t
    }

    /// The frame the clock is at at `at`, and whether a play has reached
    /// its end (forward) or start (backward) by then.
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
        let ended = if speed > 0.0 { s >= end } else { s <= 0.0 };
        (ctl.plan.frame_at_sample(s.floor() as i64), ended)
    }

    /// Says once how long the sound of play request `epoch` took to be
    /// heard, for `VIDCUT_DEBUG`.
    fn heard_first(&self, ctl: &Control, epoch: u64) {
        if self
            .heard_epoch
            .fetch_max(epoch, std::sync::atomic::Ordering::Relaxed)
            < epoch
        {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            crate::debug!(
                "audio",
                "play request {epoch} heard {:.0} ms after it was made, the output's latency {:.0} ms",
                ms(ctl.epoch_at.elapsed()),
                ms(self.heard.latency().unwrap_or_default()),
            );
        }
    }

    /// Changes what is wanted; `video` when the decode thread should look
    /// again. What it decodes is cancelled when the viewer's new frame
    /// isn't decoded and the decode may hold it up: a picture ahead,
    /// anything while the clock runs, or a seek that has run a second.
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
            let cancel = match *lock(&self.busy) {
                Some((Busy::Ahead, _)) => true,
                Some((Busy::Window { from, to, plan_gen }, _)) => {
                    playing || plan_gen != ctl.plan_gen || !(from..=to).contains(&target)
                }
                Some((Busy::Focus, since)) => sample.is_some() || since.elapsed() >= PATIENCE,
                None => false,
            };
            if cancel && !self.store.contains(&ctl.plan.id(target)) {
                self.interrupt
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        drop(ctl);
        self.changed.notify_all();
    }

    /// The decode thread's view; `None` once the player closes. Clears the
    /// cancel flag: whatever it was raised for is in this view.
    pub(crate) fn snap(&self) -> Option<Snap> {
        let mut ctl = self.control();
        if ctl.closed {
            return None;
        }
        self.interrupt
            .store(false, std::sync::atomic::Ordering::Relaxed);
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
            prepared: ctl.prepared.clone(),
        })
    }

    /// The audio thread's view; `None` once the player closes.
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
        })
    }

    /// Waits up to `timeout` for a change after `version`.
    pub(crate) fn wait_change(&self, version: u64, timeout: Duration) {
        let ctl = self.control();
        if ctl.version == version && !ctl.closed {
            let _ = self.changed.wait_timeout(ctl, timeout);
        }
    }

    /// Waits up to `timeout` for any change.
    pub(crate) fn wait_any(&self, timeout: Duration) {
        let ctl = self.control();
        if !ctl.closed {
            let _ = self.changed.wait_timeout(ctl, timeout);
        }
    }

    pub(crate) fn set_busy(&self, busy: Option<Busy>) {
        let mut b = lock(&self.busy);
        *b = match (busy, *b) {
            (Some(kind), Some((was, since))) if kind == was => Some((kind, since)),
            (Some(kind), _) => Some((kind, Instant::now())),
            (None, _) => None,
        };
    }

    pub(crate) fn fail(&self, why: String) {
        self.control().error = Some(why);
    }

    /// The picture of the frame the viewer is at, `frame`, failed to decode.
    pub(crate) fn failed(&self, frame: i64) {
        *lock(&self.failed) = Some(frame);
        if let Some(f) = &self.on_picture {
            f();
        }
    }

    /// The decode thread's new reader decodes on the CPU for `why` (GPU
    /// decoding off, no GPU decoder), or where it can on the GPU (`None`).
    pub(crate) fn decodes_on_cpu(&self, why: Option<&'static str>) {
        *lock(&self.cpu) = why;
    }

    /// The picture of the frame the viewer is at landed: `id`, of the plan
    /// of change `plan_gen`.
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

    /// The audio thread queued sound with these peaks.
    pub(crate) fn level(&self, level: Level) {
        let mut levels = lock(&self.levels);
        if levels.len() == LEVELS_KEPT {
            levels.pop_front();
        }
        levels.push_back(level);
    }

    /// What was queued won't be heard (a new play request, a pause).
    pub(crate) fn forget_levels(&self) {
        lock(&self.levels).clear();
    }

    /// The decode thread decodes on `device`: an automatic video memory
    /// budget is half its memory from now on.
    pub(crate) fn gpu_found(&self, device: &crate::cuda::CudaDevice) {
        if self.gpu_memory.get().is_none()
            && let Ok(bytes) = device.memory()
        {
            let _ = self.gpu_memory.set(bytes);
            self.apply_video_budget();
        }
    }

    fn apply_video_budget(&self) {
        if self.video_auto.load(std::sync::atomic::Ordering::Relaxed) {
            let bytes = self.gpu_memory.get().map_or(GB, |m| m / 2);
            self.store.set_budget(store::Kind::Video, bytes);
        }
    }

    /// The prepared picture landed: `Status::prepared` changed.
    pub(crate) fn prepared(&self) {
        if let Some(f) = &self.on_picture {
            f();
        }
    }
}

/// Plays a render plan. Requests return at once; pictures come from
/// [`present`](Self::present), as the clock says. Dropping it stops the
/// threads and the audio output.
pub struct Player {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    /// Dropped after the threads end: the device stops last. In a mutex,
    /// so a player can be shared between threads (the session sends
    /// requests, the renderer presents).
    output: Mutex<Option<Box<dyn AudioOutput>>>,
}

impl Player {
    /// A player paused at the plan's first frame, its sound going to
    /// `output` (opened at the plan's sample rate and channels).
    pub fn new(plan: RenderPlan, config: Config, mut output: Box<dyn AudioOutput>) -> Result<Self> {
        crate::init();
        if plan.frames <= 0 {
            return Err(Error::Invalid("nothing to play".into()));
        }
        let (rate, channels) = (plan.sample_rate, plan.channels);
        let base = Instant::now();
        // A second of sound: the audio thread keeps a tenth of it queued.
        let ring = Arc::new(Ring::new(rate as usize, channels));
        let heard = Arc::new(Heard::new(base, rate));
        let source = AudioSource::new(ring.clone(), heard.clone(), rate, channels);
        let device = source.failure.clone();
        let at = plan.range.0;
        let shared = Arc::new(Shared {
            control: Mutex::new(Control {
                plan: Arc::new(Plan::new(plan)),
                plan_gen: 0,
                mode: Mode::Paused { at },
                version: 0,
                epoch: 0,
                epoch_at: Instant::now(),
                trims: Vec::new(),
                trims_gen: 0,
                muted: false,
                scrubbing: config.audio_scrubbing,
                scrub: None,
                hardware: config.hardware,
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
        });
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

    /// Plays `plan` instead: an edit. The position stays (within the new
    /// plan), and so do open files and kept pictures where the new plan
    /// shows the same frames. The sample rate and channels must stay.
    pub fn set_plan(&self, plan: RenderPlan) -> Result<()> {
        self.check(&plan)?;
        // The same plan: nothing changes, nothing is cancelled.
        if self.shared.control().plan.plan == plan {
            return Ok(());
        }
        self.shared.change(true, |ctl, pos, _| {
            replace_plan(ctl, plan);
            // Paused, the same frame. Playing, the sound goes on from the
            // same time; at another frame rate, the picture shown stays
            // until the new plan's is decoded.
            if let Mode::Paused { .. } = ctl.mode {
                ctl.mode = Mode::Paused {
                    at: ctl.plan.clamp(pos),
                };
            }
        });
        Ok(())
    }

    /// Plays `plan` from timeline frame `at`, without the frame's sound:
    /// another thing to show, or an edit seen from elsewhere. Paused, it
    /// shows `at`; playing, play goes on from there. Until `at`'s picture
    /// is decoded, the one shown before stays. One change, so nothing is
    /// decoded for the frame in between. The sample rate and channels must
    /// stay.
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

    /// Refuses a plan the player can't play: empty, or at another sample
    /// rate or channel count than the device was opened for. Nothing
    /// changes then, so nothing is cancelled.
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

    /// Keeps timeline frame `at` of `plan` ready: the plan played next
    /// ([`set_plan_at`](Self::set_plan_at)), such as what a viewer shows
    /// when it switches. Its picture decodes while the player is paused
    /// with nothing else to decode, on a reader of its own (the one the
    /// player reads with stays where it is), and is held whatever the
    /// budget until another is prepared. `None` lets it go.
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
        drop(ctl);
        self.shared.changed.notify_all();
    }

    /// Shows timeline frame `frame` (within the plan). Playing, play goes
    /// on from there; paused, the frame's sound plays when scrubbing (a
    /// drag across the timeline is seeks).
    pub fn seek(&self, frame: i64) {
        self.seek_with(frame, true);
    }

    /// [`seek`](Self::seek) without the frame's sound: a move the viewer
    /// makes by itself (opening at a remembered frame, after an edit).
    pub fn seek_quietly(&self, frame: i64) {
        self.seek_with(frame, false);
    }

    fn seek_with(&self, frame: i64, sound: bool) {
        self.shared
            .change(true, |ctl, _, _| move_to(ctl, frame, sound));
    }

    /// Pauses and moves `frames` frames from where the viewer is (negative
    /// back), playing the frame's sound when scrubbing: forward, whichever
    /// way the step went (decision D42).
    pub fn step(&self, frames: i64) {
        self.shared.change(true, |ctl, pos, _| {
            let t = ctl.plan.clamp(pos.saturating_add(frames));
            ctl.scrub(t);
            ctl.mode = Mode::Paused { at: t };
        });
    }

    /// Turns audio scrubbing on or off (on by default): whether steps and
    /// paused seeks play the frame's sound.
    pub fn set_audio_scrubbing(&self, on: bool) {
        self.shared.change(false, |ctl, _, _| {
            ctl.scrubbing = on;
            if !on {
                ctl.scrub = None;
            }
        });
    }

    /// Plays at `speed` frames of timeline per frame of time from where the
    /// viewer is: 1 normal, 2 twice as fast, negative backwards; 0 pauses.
    /// Playing on from the end of the plan (or backwards from its start)
    /// starts over from the other end.
    pub fn play(&self, speed: f64) {
        if speed == 0.0 || !speed.is_finite() {
            return self.pause();
        }
        self.shared.change(true, |ctl, pos, sample| {
            let from = match (ctl.mode, sample) {
                // Playing on at another speed: from the sample heard now.
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
            ctl.mode = Mode::Playing {
                from,
                speed,
                epoch: ctl.epoch,
            };
        });
    }

    /// Stops at the frame the viewer is at.
    pub fn pause(&self) {
        self.shared.change(true, |ctl, pos, _| {
            ctl.mode = Mode::Paused { at: pos };
        });
    }

    /// A gain on top of audio lane `lane`'s own, in dB (a fader being
    /// moved): heard within a tenth of a second plus the device's latency.
    /// A new plan starts without one.
    pub fn set_lane_gain(&self, lane: usize, db: f64) {
        self.shared.change(false, |ctl, _, _| {
            if ctl.trims.len() <= lane {
                ctl.trims.resize(lane + 1, 0.0);
            }
            ctl.trims[lane] = db;
            ctl.trims_gen += 1;
        });
    }

    /// Decodes on the GPU (NVDEC) the streams it decodes, or everything on
    /// the CPU. A change decodes the pictures anew, the one shown first, so
    /// what shows next is the new decoder's.
    pub fn set_hardware(&self, on: bool) {
        if self.shared.control().hardware == on {
            return;
        }
        self.shared.change(true, |ctl, _, _| ctl.hardware = on);
    }

    /// Keeps pictures within `budget` from now on: over it, those kept or
    /// looked at least recently go at once.
    pub fn set_budget(&self, budget: Budget) {
        let store = &self.shared.store;
        store.set_budget(store::Kind::Memory, budget.memory);
        self.shared
            .video_auto
            .store(budget.video.is_none(), std::sync::atomic::Ordering::Relaxed);
        match budget.video {
            Some(bytes) => store.set_budget(store::Kind::Video, bytes),
            None => self.shared.apply_video_budget(),
        }
    }

    /// Silences the sound (the viewer's monitoring mute); the clock runs on.
    pub fn set_muted(&self, muted: bool) {
        self.shared.change(false, |ctl, _, _| {
            ctl.muted = muted;
            ctl.trims_gen += 1;
        });
    }

    /// The picture to show at `now`: that of the frame the clock is at, or,
    /// while it isn't decoded yet, the newest picture a seek landed since
    /// the one shown, else the picture shown before. So a dragged playhead
    /// shows each frame a seek decodes, though it has moved on by then.
    /// `None` before the first picture is decoded.
    pub fn present(&self, now: Instant) -> Option<Shown> {
        let (id, pos, speed, clock, plan_gen) = {
            let mut ctl = self.shared.control();
            let pos = self.shared.position(&mut ctl, now);
            // The clock runs once the sound of the play request is heard.
            let (speed, clock) = match ctl.mode {
                Mode::Playing { speed, epoch, .. } => {
                    (speed, self.shared.clock_sample(epoch, now).map(|_| epoch))
                }
                Mode::Paused { .. } => (0.0, None),
            };
            (ctl.plan.id(pos), pos, speed, clock, ctl.plan_gen)
        };
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
                    exact: true,
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
                    });
                }
                p.shown.clone().map(|s| Shown {
                    frame: pos,
                    exact: false,
                    ..s
                })
            }
        }
    }

    /// What it does, for a person who wants to know ([`Stats`]).
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
            cpu: *lock(&self.shared.cpu),
            plays_on: self.plays_on(),
            rate,
            channels,
            latency: self.shared.heard.latency(),
            copies: self.shared.copies.counted(),
        }
    }

    /// Where the frames it copies between GPU memory and memory count: a
    /// viewer showing it installs it on its thread
    /// ([`copies::Guard`](crate::copies::Guard)) while it draws.
    pub fn copies(&self) -> Arc<crate::copies::Copies> {
        self.shared.copies.clone()
    }

    /// The device its sound plays on ([`AudioOutput::plays_on`]).
    pub fn plays_on(&self) -> PlaysOn {
        lock(&self.output)
            .as_ref()
            .map_or(PlaysOn::Default, |o| o.plays_on())
    }

    /// The peaks of the sound heard at `now`, over the last 50 ms; `None`
    /// while paused, or before any is heard.
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
        let (id, prepared, position, mode, range, error) = {
            let mut ctl = self.shared.control();
            let position = self.shared.position(&mut ctl, Instant::now());
            let plan = &ctl.plan.plan;
            (
                ctl.plan.id(position),
                ctl.prepared.as_ref().map(|(p, t)| p.id(*t)),
                position,
                ctl.mode,
                (plan.range.0, plan.range.0 + plan.frames),
                ctl.error.clone(),
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
        }
    }

    /// Waits until the picture of timeline frame `frame` is decoded, up to
    /// `timeout`; whether it is.
    pub fn wait_ready(&self, frame: i64, timeout: Duration) -> bool {
        let id = self.shared.control().plan.id(frame);
        self.shared
            .store
            .wait(&id, Instant::now() + timeout)
            .is_some()
    }

    /// The timeline frame the clock is at at `at`, changing nothing: now,
    /// the display's next refresh, or a moment ago (the clock remembers the
    /// device's last 64 buffers, about half a second).
    pub fn frame_at(&self, at: Instant) -> i64 {
        let ctl = self.shared.control();
        self.shared.clock_frame(&ctl, at).0
    }

    /// How long from `now` until the clock moves to another frame, while
    /// playing: when a display should look again.
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

/// Plays `plan` instead of the plan played now. Lane gains on top go: the
/// new plan has its own.
fn replace_plan(ctl: &mut Control, plan: RenderPlan) {
    ctl.plan = Arc::new(Plan::new(plan));
    ctl.plan_gen += 1;
    ctl.trims.clear();
    ctl.trims_gen += 1;
}

/// Moves to timeline frame `frame` (within the plan). Paused, its sound
/// plays when `sound` and scrubbing; playing, play goes on from there.
fn move_to(ctl: &mut Control, frame: i64, sound: bool) {
    let t = ctl.plan.clamp(frame);
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

/// The sample play at `speed` starts from at timeline frame `t`: its first
/// going forward, its last going back (so it shows for a frame's time).
fn start_sample(plan: &Plan, t: i64, speed: f64) -> i64 {
    if speed > 0.0 {
        plan.sample(t)
    } else {
        (plan.sample(t + 1) - 1).max(0)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        {
            let mut ctl = self.shared.control();
            ctl.closed = true;
            ctl.version += 1;
        }
        self.shared
            .interrupt
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.shared.changed.notify_all();
        self.shared.store.wake();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        lock(&self.output).take();
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
            range: (0, 1),
            name: "copies".into(),
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
