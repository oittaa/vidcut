//! The live session: the running GUI's core thread.
//!
//! One thread owns the open project, the viewer's player and everything the
//! viewer shows. The GUI, IPC clients and background jobs reach it only
//! through messages ([`Handle`]). After every change it publishes an
//! immutable [`Snapshot`], which the UI thread reads without waiting: the UI
//! never blocks on the session. The player is vidcut's own engine
//! (`vidcut_av::player`), one for the source and the timeline: the session
//! sends it requests, which return at once, and the renderer presents its
//! pictures.
//!
//! Requests are the same [`Request`]s the CLI runs headless. Project edits go
//! through the project's [`Editor`](vidcut_core::Editor) and are saved before
//! the reply; live-only requests (viewer, player, selection, screenshots)
//! act on the session. Requests that wait for something (a seek waits until
//! its frame is on screen, a screenshot for the picture, an export for the
//! encoder) reply later, without blocking other requests.

mod jobs;
mod missing;
mod player;
mod project;
pub mod snapshot;
mod workers;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use vidcut_core::editor::{media_context, resolve_clip};
use vidcut_core::model::{Clip, Settings};
use vidcut_core::render::{self, RenderPlan, TapeEntry};
use vidcut_core::request::{GuiPanel, OneOrMany, ScreenshotTarget, ViewerMode};
use vidcut_core::time::{Keyword, Rational, TimeContext, TimeSpec, format_timecode};
use vidcut_core::{Envelope, Error, Project, Request, Response, Result};

pub use jobs::{JobInfo, JobOutput, JobState};
pub use player::{MakeOutput, debug};
pub use snapshot::*;
pub use vidcut_av::thumbs::Image;

use crate::env::AvEnv;
use crate::ipc::{self, Instance};
use crate::paths;
use crate::threads::Work;
use player::Player;
use project::OpenProject;
use workers::Workers;

/// Where a request came from (shown in the command log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The GUI. `drag`: the request ends a drag on the timeline. A media
    /// drop is an edit but not an edit function, so the playhead stays
    /// (decision D15).
    Gui {
        drag: bool,
    },
    Ipc,
    Session,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gui { .. } => "gui",
            Self::Ipc => "ipc",
            Self::Session => "session",
        }
    }
}

/// Called with a captured picture (or why there is none).
pub type Capture = Box<dyn FnOnce(Result<Image>) + Send>;

/// Called once a player was presented, with the frame it showed exactly
/// (`None`: the frame's picture wasn't decoded yet).
pub type Presented = Box<dyn FnOnce(Option<i64>) + Send>;

/// What the session needs from whoever renders the viewer: the GUI, or a
/// test harness.
pub trait Renderer: Send + Sync {
    /// The engine the viewer shows from now on (`None`: nothing, black).
    /// The renderer presents it: draws the picture `present` gives as the
    /// clock moves.
    fn attach(&self, _engine: Option<Arc<vidcut_av::player::Player>>) {}
    /// Draw the engine's current picture now, then call `done` with the
    /// frame shown. The session asks once a seek or load has its picture:
    /// the viewer then shows its frame (docs/decisions/gui.md §4).
    fn present(&self, done: Presented);
    /// The picture the viewer shows, as displayed.
    fn capture_viewer(&self, done: Capture);
    /// The whole window.
    fn capture_window(&self, done: Capture);
    /// Change the window.
    fn window(&self, change: WindowChange);
    /// Close the window (and so the session).
    fn quit(&self);
    /// Open a window for the person at the GUI.
    fn show(&self, _panel: GuiPanel) {}
}

/// A `gui.window` request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowChange {
    pub size: Option<[f32; 2]>,
    pub minimized: Option<bool>,
    pub maximized: Option<bool>,
    pub fullscreen: Option<bool>,
}

pub struct Config {
    pub project: Option<PathBuf>,
    /// Create the project file if it doesn't exist.
    pub create: bool,
    /// The viewer's player; `None` runs without preview.
    pub player: Option<PlayerConfig>,
    /// Why there is no preview, if so.
    pub preview_error: Option<String>,
    pub renderer: Arc<dyn Renderer>,
    /// Asks the GUI to repaint (a new snapshot is ready).
    pub notify: Arc<dyn Fn() + Send + Sync>,
    /// The IPC socket this instance serves, for the instance registry.
    pub socket: Option<PathBuf>,
}

/// What the viewer's player needs.
pub struct PlayerConfig {
    /// Makes its audio output: the system's device in the GUI (a silent
    /// stand-in when it won't open), a stand-in in tests.
    pub audio: MakeOutput,
    /// Memory for its pictures: what is shown, and the frame of what isn't.
    pub budget: vidcut_av::player::Budget,
    /// The renderer shows pictures in GPU memory from there: those NVDEC or
    /// VA-API decodes stay on the GPU (and kept ones in video memory).
    pub gpu_pictures: bool,
}

/// Progress and the final answer of one request.
pub(crate) enum ReplyEvent {
    Progress(Value),
    Done(Result<Response>),
}

pub(crate) struct Reply {
    tx: Sender<ReplyEvent>,
    /// Set when the client went away; long requests cancel.
    cancel: Arc<AtomicBool>,
}

/// A request being answered.
pub(crate) struct Job {
    method: String,
    params: Value,
    origin: Origin,
    reply: Option<Reply>,
    /// The project's revision when a request answered at once started: a
    /// different one at the end means it changed the project.
    rev: Option<u64>,
    /// A dry run: it changes nothing whatever it reports.
    dry_run: bool,
}

pub(crate) enum Msg {
    /// The media whose files are missing now ([`missing`]).
    Missing(HashSet<String>),
    Request {
        envelope: Envelope,
        origin: Origin,
        /// The project the client expects to be open.
        project: Option<PathBuf>,
        reply: Option<Reply>,
    },
    /// The picture the viewer is at landed (or failed).
    Picture,
    /// The renderer presented the engine after `motion` was asked, showing
    /// `frame` exactly.
    Presented {
        motion: u64,
        frame: Option<i64>,
    },
    Captured {
        id: u64,
        result: Result<Image>,
    },
    TaskDone {
        job: Job,
        result: Result<Response>,
    },
    /// What a request needed is measured: now it runs. `encode` is the job
    /// a proxy encode ran as.
    Measured {
        job: Job,
        envelope: Envelope,
        result: Result<()>,
        encode: Option<String>,
    },
    FileChanged,
    /// The preferences file changed.
    PreferencesChanged,
    /// `wanted`: what it was made for (`media_version`), so a late answer
    /// for another file is dropped.
    Thumbs {
        media: String,
        wanted: String,
        result: std::result::Result<Filmstrip, String>,
    },
    Waveform {
        media: String,
        wanted: String,
        result: std::result::Result<vidcut_av::waveform::Waveform, String>,
    },
    /// An export job planned its files: paths and lengths in frames.
    JobPlanned {
        id: String,
        outputs: Vec<(PathBuf, i64)>,
    },
    JobProgress {
        id: String,
        output: usize,
        frames: i64,
        total: i64,
        /// The run over the range, of `passes` (decision D45).
        pass: u8,
        passes: u8,
        /// Measuring light, before encoding (decision D46).
        measuring: bool,
    },
    JobDone {
        id: String,
        result: Result<Response>,
    },
    Shutdown,
}

/// Talks to the session thread; cheap to clone.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<Msg>,
    shared: Arc<Mutex<Arc<Snapshot>>>,
}

impl Handle {
    /// The latest snapshot.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Sends a request from the GUI; errors show up as notices.
    pub fn send(&self, envelope: Envelope) {
        self.send_from(envelope, false);
    }

    fn send_from(&self, envelope: Envelope, drag: bool) {
        let _ = self.tx.send(Msg::Request {
            envelope,
            origin: Origin::Gui { drag },
            project: None,
            reply: None,
        });
    }

    /// Builds and sends a GUI request from a method name and params.
    pub fn request(&self, method: &str, params: Value) {
        self.request_from(method, params, false);
    }

    /// [`request`](Self::request) for the request that ends a timeline drag.
    pub fn request_drag(&self, method: &str, params: Value) {
        self.request_from(method, params, true);
    }

    fn request_from(&self, method: &str, params: Value, drag: bool) {
        match Envelope::from_json(json!({ "method": method, "params": params })) {
            Ok(e) => self.send_from(e, drag),
            Err(e) => {
                // A GUI bug: report it like any other failure.
                let _ = self.tx.send(Msg::TaskDone {
                    job: Job {
                        method: method.into(),
                        params,
                        origin: Origin::Gui { drag },
                        reply: None,
                        rev: None,
                        dry_run: false,
                    },
                    result: Err(Error::internal(e)),
                });
            }
        }
    }

    /// Runs a request and waits for its answer. `progress` receives progress
    /// notifications (`Some`) and returns false to cancel. `None` only asks
    /// whether the client is still there: a closed connection then sets the
    /// same flag a failed progress write sets, so work that never reports
    /// progress (a frame, a storyboard, an analysis, the loudness measure
    /// before an export) stops too.
    pub fn call(
        &self,
        envelope: Envelope,
        origin: Origin,
        project: Option<PathBuf>,
        progress: &mut dyn FnMut(Option<Value>) -> bool,
    ) -> Result<Response> {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        // Already gone: the worker sees the flag before it starts.
        if !progress(None) {
            cancel.store(true, Ordering::Relaxed);
        }
        self.tx
            .send(Msg::Request {
                envelope,
                origin,
                project,
                reply: Some(Reply {
                    tx,
                    cancel: cancel.clone(),
                }),
            })
            .map_err(|_| Error::no_instance("the vidcut GUI is shutting down"))?;
        loop {
            match rx.recv_timeout(CLIENT_POLL) {
                Ok(ReplyEvent::Progress(v)) => {
                    if !progress(Some(v)) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
                Ok(ReplyEvent::Done(r)) => return r,
                Err(RecvTimeoutError::Timeout) => {
                    if !progress(None) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::no_instance("the vidcut GUI is shutting down"));
                }
            }
        }
    }

    /// Stops the session thread.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
    }

    /// Stops the session thread and waits until it has ended (jobs
    /// cancelled, the instance unregistered), up to `timeout`.
    pub fn shutdown_and_wait(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.tx.send(Msg::Shutdown).is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Starts the session thread.
pub fn start(config: Config) -> (Handle, std::thread::JoinHandle<()>) {
    let (tx, rx) = channel();
    let pid = std::process::id();
    let (audio, budget, gpu_pictures) = match config.player {
        Some(p) => (Some(p.audio), p.budget, p.gpu_pictures),
        None => (None, vidcut_av::player::Budget::both(0), false),
    };
    let player = {
        let (tx, notify) = (tx.clone(), config.notify.clone());
        Player::new(
            audio,
            budget,
            gpu_pictures,
            config.renderer.clone(),
            Arc::new(move || {
                let _ = tx.send(Msg::Picture);
                notify();
            }),
        )
    };
    let instance = config.socket.clone().map(|socket| Instance {
        pid,
        socket,
        project: None,
        version: env!("CARGO_PKG_VERSION").into(),
    });
    let empty = Snapshot {
        seq: 0,
        pid,
        socket: config.socket.clone(),
        project: None,
        viewer: ViewerMode::Timeline,
        last_source: ViewerMode::SourceClip,
        source: PlayerSnap::default(),
        timeline: PlayerSnap::default(),
        source_media: None,
        tape: Arc::new(Vec::new()),
        selection: Selection::default(),
        pool: PoolView::default(),
        clipboard: 0,
        log: Arc::new(VecDeque::new()),
        notices: Arc::new(VecDeque::new()),
        thumbs: Arc::new(HashMap::new()),
        missing: Arc::new(HashSet::new()),
        waveforms: Arc::new(HashMap::new()),
        jobs: Arc::new(Vec::new()),
        audio_scrubbing: true,
        preview_error: config.preview_error.clone(),
    };
    let shared = Arc::new(Mutex::new(Arc::new(empty)));
    // The folder must be there to be watched.
    let preferences = crate::preferences::path();
    if let Some(dir) = preferences.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut live = Live {
        _preferences: project::watch(&preferences, tx.clone(), || Msg::PreferencesChanged),
        tx: tx.clone(),
        rx,
        shared: shared.clone(),
        notify: config.notify,
        renderer: config.renderer,
        instance,
        project: None,
        project_snap: None,
        viewer: ViewerMode::Timeline,
        last_source: ViewerMode::SourceClip,
        source_media: None,
        source_content: None,
        source_positions: HashMap::new(),
        tape_position: 0,
        tape: Arc::new(Vec::new()),
        tape_media: None,
        player,
        selection: Selection::default(),
        pool: PoolView::default(),
        clipboard: Vec::new(),
        log: Arc::new(VecDeque::new()),
        log_seq: 0,
        notices: Arc::new(VecDeque::new()),
        notice_seq: 0,
        thumbs: Arc::new(HashMap::new()),
        missing: Arc::new(HashSet::new()),
        missing_watch: missing::MissingWatch::start(tx.clone()),
        thumbs_wanted: HashMap::new(),
        waveforms: Arc::new(HashMap::new()),
        waveforms_wanted: HashMap::new(),
        workers: Workers::new(2),
        jobs: Arc::new(Vec::new()),
        job_cancels: HashMap::new(),
        job_replies: HashMap::new(),
        next_job: 1,
        audio_scrubbing: true,
        audio_noticed: None,
        playing: false,
        waits: Vec::new(),
        next_capture: 1,
        timeline_due: None,
        timeline_loaded_at: None,
        seq: 0,
        dirty: true,
        answers: Vec::new(),
        preview_error: config.preview_error,
    };
    let project = config.project;
    let create = config.create;
    let thread = crate::threads::spawn("vidcut-session", Work::Interactive, move || {
        if let Some(path) = project
            && let Err(e) = new_settings(&path, create, None, (None, None), None, None)
                .and_then(|settings| live.open_project(&path, settings))
        {
            live.notice(
                format!("can't open {}: {}", path.display(), e.message),
                true,
            );
        }
        live.register();
        live.run();
    })
    .expect("spawning the session thread");
    (Handle { tx, shared }, thread)
}

/// What the source has loaded.
#[derive(Debug, Clone, PartialEq)]
enum SourceContent {
    Clip(String),
    Tape,
}

/// What a request waits for before replying.
enum Until {
    /// Motion `motion` is on screen, showing the player's `target` as it
    /// was then. If a later seek or switch asks for another frame first
    /// (another client, a person scrubbing), that frame won't show: the
    /// request fails with CONFLICT. Playback moving on from it doesn't
    /// count.
    Shown { motion: u64, at: i64 },
    /// Job `id` ended (`jobs.wait`).
    Job { id: String },
    /// The viewer shows its latest motion (after an edit's reload, say);
    /// then it is captured to `out`.
    Settled { out: PathBuf },
    /// Capture `id` arrives; then it is written to `out`. For a viewer
    /// capture, `player` is what the viewer shows.
    Capture {
        id: u64,
        out: PathBuf,
        player: Option<PlayerId>,
    },
}

struct Wait {
    job: Job,
    until: Until,
    /// When to give up (none: `jobs.wait` without a timeout).
    deadline: Option<Instant>,
}

/// How long a request may wait for frames or captures.
const WAIT_LIMIT: Duration = Duration::from_secs(10);

/// How often an IPC call checks that its client is still connected, when the
/// work itself is not sending progress.
const CLIENT_POLL: Duration = Duration::from_millis(50);

/// Finished jobs kept for `jobs.list` and the jobs panel.
const JOBS_KEPT: usize = 50;

/// Entries kept in the command log and notices.
const LOG_CAP: usize = 500;
const NOTICE_CAP: usize = 20;

/// Waveform buckets per second (10 ms).
const WAVEFORM_RATE: u32 = 100;

/// Thumbnails per pool item and their maximum size.
const THUMBS: usize = 24;
const THUMB_SIZE: (u32, u32) = (192, 108);

/// What a media item's thumbnails and waveform are made from: its file
/// (path, size, modification time) and, for a sequence, its count.
fn media_version(m: &vidcut_core::model::Media) -> String {
    format!(
        "{}|{}|{}|{}",
        m.path.display(),
        m.size,
        m.mtime_ns,
        m.sequence.as_ref().map_or(0, |s| s.count)
    )
}

/// The pool's filmstrip of `media`: from the thumbnail cache when the
/// preferences keep one, decoded (and then kept) otherwise.
fn filmstrip(media: &vidcut_core::model::Media) -> std::result::Result<Filmstrip, String> {
    let prefs = crate::preferences::load().unwrap_or_default().thumbnails;
    let cached = prefs
        .disk_cache
        .then(|| crate::thumbcache::load(media, THUMBS, THUMB_SIZE))
        .flatten();
    let (images, times) = match cached {
        Some(strip) => strip,
        None => {
            let input = crate::env::input_of(media);
            // image2 would read every numbered file after the first. A
            // sequence is only its own count, even when more files follow.
            let strip = match media.sequence.as_ref().map(|s| s.count) {
                Some(pictures) => vidcut_av::thumbs::filmstrip_pictures(
                    &input,
                    pictures,
                    THUMBS,
                    THUMB_SIZE.0,
                    THUMB_SIZE.1,
                ),
                None => vidcut_av::thumbs::filmstrip(&input, THUMBS, THUMB_SIZE.0, THUMB_SIZE.1),
            }
            .map_err(|e| e.to_string())?;
            if prefs.disk_cache && !strip.0.is_empty() {
                crate::thumbcache::store(
                    media,
                    THUMBS,
                    THUMB_SIZE,
                    (&strip.0, &strip.1),
                    prefs.cache_limit_mb,
                );
            }
            strip
        }
    };
    Ok(Filmstrip {
        width: images.first().map_or(0, |i| i.width),
        height: images.first().map_or(0, |i| i.height),
        frames: images.into_iter().map(|i| i.rgba).collect(),
        times,
    })
}

/// Minimum time between timeline reloads while edits keep coming.
const RELOAD_EVERY: Duration = Duration::from_millis(100);

struct Live {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Watches the preferences file: GPU acceleration reaches the player
    /// at once, whoever changes it.
    _preferences: Option<notify::RecommendedWatcher>,
    shared: Arc<Mutex<Arc<Snapshot>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    renderer: Arc<dyn Renderer>,
    instance: Option<Instance>,
    project: Option<OpenProject>,
    project_snap: Option<ProjectSnap>,
    viewer: ViewerMode,
    last_source: ViewerMode,
    source_media: Option<String>,
    source_content: Option<SourceContent>,
    source_positions: HashMap<String, i64>,
    tape_position: i64,
    tape: Arc<Vec<TapeEntry>>,
    /// The media under the Source Tape playhead (selection sync).
    tape_media: Option<String>,
    /// The viewer's player: the source and the timeline.
    player: Player,
    selection: Selection,
    pool: PoolView,
    clipboard: Vec<vidcut_core::model::TrackClip>,
    log: Arc<VecDeque<LogEntry>>,
    log_seq: u64,
    notices: Arc<VecDeque<Notice>>,
    notice_seq: u64,
    thumbs: Arc<HashMap<String, Arc<Filmstrip>>>,
    /// Media whose files are missing, and what looks.
    missing: Arc<HashSet<String>>,
    missing_watch: missing::MissingWatch,
    /// What each media's thumbnails and waveform are being made, or were
    /// made, for: its file as `media_version` gives it (the waveform's with
    /// the channel count). Media ids are only a project's: another project,
    /// or a relinked or changed file, makes them again.
    thumbs_wanted: HashMap<String, String>,
    waveforms: Arc<HashMap<String, Arc<vidcut_av::waveform::Waveform>>>,
    waveforms_wanted: HashMap<String, String>,
    workers: Workers,
    /// Export jobs, oldest first.
    jobs: Arc<Vec<JobInfo>>,
    job_cancels: HashMap<String, Arc<AtomicBool>>,
    /// Requests answered when their job is planned (`background`) or done.
    job_replies: HashMap<String, (Job, bool)>,
    next_job: u64,
    /// Steps and paused seeks play their frame's sound (decision D42).
    audio_scrubbing: bool,
    /// What was last told about the sound ([`player::Player::audio_notice`]),
    /// so each new reason is told once.
    audio_noticed: Option<String>,
    /// Whether the viewer played when last looked at.
    playing: bool,
    waits: Vec<Wait>,
    next_capture: u64,
    timeline_due: Option<Instant>,
    timeline_loaded_at: Option<Instant>,
    seq: u64,
    dirty: bool,
    /// Answers held until the snapshot shows what they answer, so a client
    /// that looks once it hears back (the GUI, a test stepping it) sees it.
    answers: Vec<(Reply, Result<Response>)>,
    preview_error: Option<String>,
}

const SOURCE: PlayerId = PlayerId::Source;
const TIMELINE: PlayerId = PlayerId::Timeline;

impl Live {
    // ---------------------------------------------------------------- loop

    fn run(&mut self) {
        loop {
            // Timers may finish requests too; then everyone hears back with
            // the snapshot showing it, before the session waits again.
            let wake = self.tick(Instant::now());
            self.publish();
            for (r, result) in self.answers.drain(..) {
                let _ = r.tx.send(ReplyEvent::Done(result));
            }
            let msg = match wake {
                Some(at) => {
                    match self
                        .rx
                        .recv_timeout(at.saturating_duration_since(Instant::now()))
                    {
                        Ok(m) => Some(m),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match self.rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => break,
                },
            };
            if let Some(msg) = msg
                && !self.handle(msg)
            {
                break;
            }
            // Take whatever else is already queued before publishing.
            while let Ok(msg) = self.rx.try_recv() {
                if !self.handle(msg) {
                    self.shutdown();
                    return;
                }
            }
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        // Running exports cancel, removing their partial files.
        for c in self.job_cancels.values() {
            c.store(true, Ordering::Relaxed);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.job_cancels.is_empty() {
            match self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Msg::JobDone { id, result }) => self.job_done(id, result),
                // A proxy encode stopped: nothing is recorded.
                Ok(Msg::Measured {
                    job,
                    encode: Some(id),
                    ..
                }) => {
                    self.job_cancels.remove(&id);
                    reply(job.reply, Err(Error::no_instance("the vidcut GUI closed")));
                }
                // Only noted (the files of a job that gets stuck): its
                // client hears that the GUI closed, not that it started.
                Ok(Msg::JobPlanned { id, outputs }) => {
                    if let Some(j) = Arc::make_mut(&mut self.jobs)
                        .iter_mut()
                        .find(|j| j.id == id)
                    {
                        j.planned(outputs);
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        // A job still running is stuck in a decoder or an encoder, and the
        // process won't wait for it: its partial files go now. A job plans
        // its files before it writes them, and once cancelled starts none,
        // so this misses only a file whose creation began before the cancel
        // and is still blocked (on a hung network share, say).
        for job in self.jobs.iter() {
            if self.job_cancels.contains_key(&job.id) {
                for o in &job.outputs {
                    let part = vidcut_av::export::partial_path(&o.path);
                    // An .otiod bundle is written as a folder.
                    let _ = std::fs::remove_file(&part).or_else(|_| std::fs::remove_dir_all(&part));
                }
            }
        }
        for w in std::mem::take(&mut self.waits) {
            reply(
                w.job.reply,
                Err(Error::no_instance("the vidcut GUI closed")),
            );
        }
        for (_, (job, _)) in std::mem::take(&mut self.job_replies) {
            reply(job.reply, Err(Error::no_instance("the vidcut GUI closed")));
        }
        for (r, result) in self.answers.drain(..) {
            let _ = r.tx.send(ReplyEvent::Done(result));
        }
        if let Some(i) = &self.instance {
            ipc::registry::unregister(i.pid);
        }
    }

    /// Handles one message; false ends the session.
    fn handle(&mut self, msg: Msg) -> bool {
        // Most messages change what the GUI shows; the frequent no-ops below
        // say so, and never clear a change made by an earlier message.
        let mut changed = true;
        match msg {
            Msg::Request {
                envelope,
                origin,
                project,
                reply,
            } => self.request(envelope, origin, project, reply),
            Msg::Picture => {
                // Waits look again in `tick`; a present may be due. The
                // snapshot changes when the picture kept ready landed.
                self.present_due();
                changed = self.ready_changed();
            }
            Msg::Presented { motion, frame } => {
                self.player.presented(motion, frame);
                changed = false;
            }
            Msg::Captured { id, result } => self.captured(id, result),
            Msg::TaskDone { job, result } => self.finish(job, result),
            Msg::Measured {
                job,
                envelope,
                result,
                encode,
            } => {
                let (job, result) = match result {
                    Err(e) => (job, Err(e)),
                    Ok(()) => {
                        // The project may have changed meanwhile; this
                        // request's change is measured from now.
                        let rev = self.project.as_ref().map(|p| p.project().rev);
                        let job = Job { rev, ..job };
                        let result = self.edit(envelope, job.origin);
                        (job, result)
                    }
                };
                if let Some(id) = encode {
                    self.job_ended(&id, &result);
                }
                self.finish(job, result);
            }
            Msg::PreferencesChanged => {
                let gpu = crate::preferences::gpu_decoding();
                changed = self.player.follow_gpu(gpu);
                changed |= self.player.follow_audio(crate::preferences::audio_device());
                self.player.set_budget(crate::preferences::player_budget());
            }
            Msg::FileChanged => {
                let refreshed = match self.project.as_mut().map(OpenProject::reload) {
                    Some(Ok(changed)) => changed,
                    Some(Err(e)) => {
                        self.notice(format!("reloading the project: {}", e.message), true);
                        false
                    }
                    None => false,
                };
                if refreshed {
                    self.project_changed(None);
                }
                changed = refreshed;
            }
            Msg::Waveform { media, wanted, .. }
                if self.waveforms_wanted.get(&media) != Some(&wanted) =>
            {
                changed = false;
            }
            Msg::Waveform { media, result, .. } => match result {
                Ok(w) => {
                    let mut map = (*self.waveforms).clone();
                    map.insert(media, Arc::new(w));
                    self.waveforms = Arc::new(map);
                }
                Err(e) => self.notice(format!("waveform of {media}: {e}"), true),
            },
            Msg::Missing(missing) => self.missing = Arc::new(missing),
            Msg::Thumbs { media, wanted, .. }
                if self.thumbs_wanted.get(&media) != Some(&wanted) =>
            {
                changed = false;
            }
            Msg::Thumbs { media, result, .. } => match result {
                Ok(strip) => {
                    let mut map = (*self.thumbs).clone();
                    map.insert(media, Arc::new(strip));
                    self.thumbs = Arc::new(map);
                }
                Err(e) => self.notice(format!("thumbnails for {media}: {e}"), true),
            },
            Msg::JobPlanned { id, outputs } => self.job_planned(id, outputs),
            Msg::JobProgress {
                id,
                output,
                frames,
                total,
                pass,
                passes,
                measuring,
            } => {
                if let Some(j) = Arc::make_mut(&mut self.jobs)
                    .iter_mut()
                    .find(|j| j.id == id)
                {
                    j.progressed(output, frames, total, (pass, passes), measuring);
                }
            }
            Msg::JobDone { id, result } => self.job_done(id, result),
            Msg::Shutdown => return false,
        }
        self.dirty |= changed;
        true
    }

    /// Asks the renderer to present when a motion waits for its picture
    /// and the engine has it.
    fn present_due(&mut self) {
        let Some(motion) = self.player.present_due(Instant::now()) else {
            return;
        };
        if debug() {
            eprintln!("[{:?}] present motion {motion}", PlayerId::of(self.viewer));
        }
        let tx = self.tx.clone();
        self.renderer.present(Box::new(move |frame| {
            let _ = tx.send(Msg::Presented { motion, frame });
        }));
    }

    /// Timed work; returns when it next needs to run.
    fn tick(&mut self, now: Instant) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let mut soonest = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        if let Some(due) = self.timeline_due {
            if now >= due {
                self.timeline_due = None;
                self.refresh_timeline();
            } else {
                soonest(due);
            }
        }
        self.present_due();
        let p = &self.player;
        // A motion not on screen yet: look again soon (its picture may be
        // decoding, or the renderer busy).
        if p.shown < p.motion {
            soonest(now + Duration::from_millis(5));
        }
        // Playing: the playhead moves with the clock; the snapshot follows
        // it frame by frame, and shows when play stops by itself (at the
        // end).
        let playing = p.playing();
        if playing != self.playing {
            self.playing = playing;
            self.dirty = true;
        }
        if let Some(d) = p.next_change(now) {
            self.dirty = true;
            soonest(now + d.max(Duration::from_millis(2)));
        } else if playing {
            soonest(now + Duration::from_millis(10));
        }
        // Each new reason, once.
        let audio = self.player.audio_notice();
        if audio != self.audio_noticed {
            if let Some(n) = &audio {
                self.notice(n.clone(), true);
            }
            self.audio_noticed = audio;
        }
        self.sync_tape_selection();
        // Requests waiting for frames or captures.
        let mut i = 0;
        while i < self.waits.len() {
            if matches!(self.waits[i].until, Until::Settled { .. }) && self.settled() {
                let id = self.capture_viewer();
                if let Until::Settled { out } = &mut self.waits[i].until {
                    let out = std::mem::take(out);
                    self.waits[i].until = Until::Capture {
                        id,
                        out,
                        player: Some(PlayerId::of(self.viewer)),
                    };
                }
            }
            let w = &self.waits[i];
            let outcome = match w.until {
                Until::Shown { motion, at } => {
                    // The latest motion counts too: a reload after an edit
                    // replaces what was asked for, at the same frame.
                    let p = &self.player;
                    let overtaken = p.target != at;
                    if overtaken && matches!(w.job.origin, Origin::Gui { .. }) {
                        // The GUI's steps and seeks overtake each other as a
                        // key repeats or a drag goes on: not an error there.
                        Some(Ok(self.state_response()))
                    } else if overtaken {
                        Some(Err(Error::conflict(format!(
                            "another seek moved the viewer to frame {} before frame {at} \
                             showed",
                            p.target
                        ))))
                    } else if let Some(e) = p.failure() {
                        Some(Err(Error::media(format!("preview: {e}"))))
                    } else if p.shown >= p.motion && p.shown >= motion {
                        Some(match p.error() {
                            Some(e) => Err(Error::media(format!("preview: {e}"))),
                            None => Ok(self.state_response()),
                        })
                    } else {
                        None
                    }
                }
                Until::Capture { .. } | Until::Settled { .. } => None,
                Until::Job { ref id } => self
                    .jobs
                    .iter()
                    .find(|j| &j.id == id)
                    .filter(|j| !j.running())
                    .map(job_answer),
            };
            let expired = w.deadline.is_some_and(|d| now >= d);
            let outcome = match outcome {
                Some(r) => Some(r),
                None if expired => Some(match &w.until {
                    Until::Shown { .. } => {
                        Err(Error::internal("timed out waiting for the frame to show"))
                    }
                    Until::Capture { .. } => Err(Error::internal(
                        "timed out waiting for the picture (target=window needs a visible \
                         window; target=viewer works while it is hidden)",
                    )),
                    Until::Settled { .. } => Err(Error::internal(
                        "timed out waiting for the viewer to settle",
                    )),
                    // Time is up: the job as it is now (still running).
                    Until::Job { id } => self.job_json(id).map(answer),
                }),
                None => None,
            };
            match outcome {
                Some(result) => {
                    let w = self.waits.swap_remove(i);
                    self.finish(w.job, result);
                }
                None => {
                    if let Some(d) = w.deadline {
                        soonest(d);
                    }
                    i += 1;
                }
            }
        }
        next
    }

    /// Whether the published snapshot's `ready` is out of date: the
    /// picture there landed.
    fn ready_changed(&self) -> bool {
        let snap = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        snap.source.ready != self.player.snap(SOURCE).ready
            || snap.timeline.ready != self.player.snap(TIMELINE).ready
    }

    fn publish(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.seq += 1;
        let snap = Snapshot {
            seq: self.seq,
            pid: std::process::id(),
            socket: self.instance.as_ref().map(|i| i.socket.clone()),
            project: self.project_snap.clone(),
            viewer: self.viewer,
            last_source: self.last_source,
            source: self.player.snap(SOURCE),
            timeline: self.player.snap(TIMELINE),
            source_media: self.source_media.clone(),
            tape: self.tape.clone(),
            selection: self.selection.clone(),
            pool: self.pool.clone(),
            clipboard: self.clipboard.len(),
            log: self.log.clone(),
            notices: self.notices.clone(),
            thumbs: self.thumbs.clone(),
            missing: self.missing.clone(),
            waveforms: self.waveforms.clone(),
            jobs: self.jobs.clone(),
            audio_scrubbing: self.audio_scrubbing,
            preview_error: self.preview_error.clone(),
        };
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(snap);
        (self.notify)();
    }

    fn notice(&mut self, text: String, error: bool) {
        self.notice_seq += 1;
        let notices = Arc::make_mut(&mut self.notices);
        notices.push_back(Notice {
            seq: self.notice_seq,
            text,
            error,
        });
        while notices.len() > NOTICE_CAP {
            notices.pop_front();
        }
        self.dirty = true;
    }

    fn register(&self) {
        if let Some(i) = &self.instance {
            let mut i = i.clone();
            i.project = self.project.as_ref().map(|p| p.store.path().to_path_buf());
            if let Err(e) = ipc::registry::register(&i) {
                eprintln!("vidcut: can't register this instance: {e}");
            }
        }
    }

    // ---------------------------------------------------------------- requests

    fn request(
        &mut self,
        envelope: Envelope,
        origin: Origin,
        expected: Option<PathBuf>,
        reply: Option<Reply>,
    ) {
        let envelope = envelope.normalized();
        let (method, params) = match serde_json::to_value(&envelope.request) {
            Ok(Value::Object(mut m)) => (
                m.remove("method")
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
                m.remove("params").unwrap_or(Value::Null),
            ),
            _ => (envelope.request.method(), Value::Null),
        };
        let job = Job {
            method,
            params,
            origin,
            reply,
            rev: self.project.as_ref().map(|p| p.project().rev),
            dry_run: envelope.dry_run,
        };
        if let Some(expected) = expected {
            let open = self.project.as_ref().map(|p| p.store.path());
            let want = vidcut_core::dirs::canonical(&expected).unwrap_or(expected.clone());
            if open != Some(want.as_path()) {
                return self.finish(
                    job,
                    Err(Error::conflict(format!(
                        "this vidcut GUI has {} open, not {}",
                        open.map_or("no project".into(), |p| p.display().to_string()),
                        expected.display()
                    ))),
                );
            }
        }
        // `if_rev` holds for every request on the project, live ones (seeks,
        // selections, screenshots) too; exports and measurements then work
        // on the project as it is now.
        if let Some(open) = &self.project
            && let Err(e) = envelope.check_rev(open.project().rev)
        {
            return self.finish(job, Err(e));
        }
        if let Request::ExportStart(params) = &envelope.request
            && !crate::export::is_otio(&params.output)
        {
            return self.start_export(envelope, job);
        }
        let limit = match &envelope.request {
            Request::JobsWait { timeout, .. } => {
                match timeout.map(|t| Duration::try_from_secs_f64(t.max(0.0))) {
                    None => None,
                    Some(Ok(d)) => Some(d),
                    Some(Err(_)) => {
                        return self.finish(job, Err(Error::usage("timeout: invalid seconds")));
                    }
                }
            }
            _ => Some(WAIT_LIMIT),
        };
        let result = self.dispatch(envelope, &job);
        match result {
            Step::Now(r) => self.finish(job, r),
            // Other changes may come before these end.
            Step::Wait(until) => self.waits.push(Wait {
                job: Job { rev: None, ..job },
                until,
                deadline: limit.map(|l| Instant::now() + l),
            }),
            Step::Task(work, task) => self.spawn_task(Job { rev: None, ..job }, work, task),
            Step::Measure(measure, then) => {
                let tx = self.tx.clone();
                // The same flag a closed connection sets. Measuring is the
                // slow part of an edit that analyzes; it stops with the client.
                let cancel = job
                    .reply
                    .as_ref()
                    .map(|r| r.cancel.clone())
                    .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
                // Decoding whole files (Detect Scene Cuts, Ripple Delete
                // Silence): a job.
                let _ = crate::threads::spawn("vidcut-measure", Work::Job, move || {
                    let _guard = vidcut_av::cancel::Guard::set(cancel);
                    let result = measure();
                    let _ = tx.send(Msg::Measured {
                        job,
                        envelope: *then,
                        result,
                        encode: None,
                    });
                });
            }
            Step::Encode(encode, then, title) => self.start_encode(job, encode, *then, title),
        }
    }

    /// The measuring a request needs before it runs: the sound or pictures
    /// of the clips it names, whose measurements the media layer then keeps.
    fn measuring(&self, envelope: &Envelope) -> Option<Measure> {
        let p = self.project.as_ref()?.project();
        let playhead = Some(self.player.position(TIMELINE));
        let (clips, sound) = match &envelope.request {
            Request::EditRippleDeleteSilence { clips, .. } => (clips.to_vec(), true),
            Request::EditDetectSceneCuts { clips, .. } => (clips.to_vec(), false),
            _ => return None,
        };
        let mut needed = Vec::new();
        for r in &clips {
            let Some(c) = resolve_clip(p, r, playhead).ok().and_then(|id| p.clip(&id)) else {
                continue;
            };
            let Some(m) = p.media(&c.media) else { continue };
            match (&c.audio, sound) {
                (Some(a), true) => needed.push((m.clone(), Some(a.stream))),
                (_, false) if m.info.video.is_some() => needed.push((m.clone(), None)),
                _ => {}
            }
        }
        Some(Box::new(move || {
            use vidcut_core::Env;
            for (m, stream) in &needed {
                match stream {
                    Some(s) => AvEnv::new().audio_levels(m, *s).map(drop)?,
                    None => AvEnv::new().picture_stats(m).map(drop)?,
                }
            }
            Ok(())
        }))
    }

    // ---------------------------------------------------------------- jobs

    /// `export.start` (media): a job on its own thread. A background request
    /// is answered with the job once its files are planned; otherwise when
    /// it ends, with progress on the way.
    fn start_export(&mut self, envelope: Envelope, job: Job) {
        let Some(open) = &self.project else {
            return self.finish(job, Err(no_project()));
        };
        // The job encodes the project as it is now (its `if_rev` checked),
        // whatever edits follow.
        let project = open.project().clone();
        let project_path = open.store.path().to_path_buf();
        let context = open
            .project()
            .time_context(Some(self.player.position(TIMELINE)));
        let Request::ExportStart(mut params) = bind_playhead(envelope, &context).request else {
            unreachable!("checked by the caller");
        };
        let background = params.background || job.reply.is_none();
        params.background = false;
        let id = format!("j{}", self.next_job);
        self.next_job += 1;
        // The client going away cancels a foreground export.
        let cancel = match (&job.reply, background) {
            (Some(r), false) => r.cancel.clone(),
            _ => Arc::new(AtomicBool::new(false)),
        };
        let progress_tx = job
            .reply
            .as_ref()
            .filter(|_| !background)
            .map(|r| r.tx.clone());
        let info = JobInfo::new(id.clone(), job.origin.name(), job.params.clone());
        self.add_job(info, cancel.clone());
        if job.reply.is_some() {
            self.job_replies.insert(id.clone(), (job, background));
        } else {
            // GUI requests: logged now, the job shows the rest.
            self.finish(job, Ok(answer(json!({ "job": id }))));
        }
        let tx = self.tx.clone();
        let _ = crate::threads::spawn(format!("vidcut-{id}"), Work::Job, move || {
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut report = |i: usize, n: usize, out: &Path, p: vidcut_av::export::Progress| {
                if p.frames != p.total && last.elapsed() < Duration::from_millis(100) {
                    return;
                }
                last = Instant::now();
                let _ = tx.send(Msg::JobProgress {
                    id: id.clone(),
                    output: i,
                    frames: p.frames,
                    total: p.total,
                    pass: p.pass,
                    passes: p.passes,
                    measuring: p.measuring,
                });
                if let Some(ptx) = &progress_tx {
                    let _ = ptx.send(ReplyEvent::Progress(json!({
                        "job": id, "output": i, "outputs": n, "path": out,
                        "frames": p.frames, "total": p.total,
                        "pass": p.pass, "passes": p.passes, "measuring": p.measuring,
                        "elapsed": p.elapsed.as_secs_f64(),
                    })));
                }
            };
            let result = crate::session::export_media(
                &project,
                &project_path,
                &params,
                &mut crate::session::Hooks {
                    cancel: &cancel,
                    progress: &mut report,
                },
                &mut |outputs| {
                    let _ = tx.send(Msg::JobPlanned {
                        id: id.clone(),
                        outputs,
                    });
                },
            );
            let _ = tx.send(Msg::JobDone { id, result });
        });
    }

    /// Lists a new job, keeping running jobs and the most recent finished
    /// ones; `cancel` stops it.
    fn add_job(&mut self, info: JobInfo, cancel: Arc<AtomicBool>) {
        self.job_cancels.insert(info.id.clone(), cancel);
        let jobs = Arc::make_mut(&mut self.jobs);
        jobs.push(info);
        let finished = jobs.iter().filter(|j| !j.running()).count();
        if finished > JOBS_KEPT {
            let mut drop = finished - JOBS_KEPT;
            jobs.retain(|j| {
                let gone = drop > 0 && !j.running();
                drop -= usize::from(gone);
                !gone
            });
        }
        self.dirty = true;
    }

    /// `proxy.generate`: the encode runs on its own thread as a job, so the
    /// Jobs window and `jobs.*` show its progress and can cancel it, as they
    /// do an export's (decision D41). A client is answered when it ends,
    /// with progress on the way, and leaving cancels it; the GUI's own
    /// request is logged when it ends. The edit that records the proxies
    /// runs here once the files are written.
    fn start_encode(&mut self, job: Job, encode: Measure, then: Envelope, title: String) {
        let id = format!("j{}", self.next_job);
        self.next_job += 1;
        let cancel = job
            .reply
            .as_ref()
            .map(|r| r.cancel.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let info = JobInfo::proxies(id.clone(), job.origin.name(), job.params.clone(), title);
        self.add_job(info, cancel.clone());
        let progress = job.reply.as_ref().map(|r| r.tx.clone());
        let tx = self.tx.clone();
        let _ = crate::threads::spawn(format!("vidcut-{id}"), Work::Job, move || {
            let _guard = vidcut_av::cancel::Guard::set(cancel);
            let _report = {
                let (tx, id) = (tx.clone(), id.clone());
                let mut last = Instant::now() - Duration::from_secs(1);
                // Each proxy file reports its own frames; the job shows
                // the one being written.
                let mut file: Option<PathBuf> = None;
                let mut output = 0;
                vidcut_av::progress::Guard::set(Box::new(
                    move |out: &Path, p: vidcut_av::export::Progress| {
                        match &file {
                            Some(f) if f == out => {}
                            Some(_) => {
                                output += 1;
                                file = Some(out.to_path_buf());
                            }
                            None => file = Some(out.to_path_buf()),
                        }
                        if p.frames != p.total && last.elapsed() < Duration::from_millis(100) {
                            return;
                        }
                        last = Instant::now();
                        let _ = tx.send(Msg::JobProgress {
                            id: id.clone(),
                            output,
                            frames: p.frames,
                            total: p.total,
                            pass: p.pass,
                            passes: p.passes,
                            measuring: p.measuring,
                        });
                        if let Some(ptx) = &progress {
                            let _ = ptx.send(ReplyEvent::Progress(json!({
                                "job": id, "output": output, "path": out,
                                "frames": p.frames, "total": p.total,
                                "elapsed": p.elapsed.as_secs_f64(),
                            })));
                        }
                    },
                ))
            };
            let result = encode();
            let _ = tx.send(Msg::Measured {
                job,
                envelope: then,
                result,
                encode: Some(id),
            });
        });
    }

    fn job_planned(&mut self, id: String, outputs: Vec<(PathBuf, i64)>) {
        if let Some(j) = Arc::make_mut(&mut self.jobs)
            .iter_mut()
            .find(|j| j.id == id)
        {
            j.planned(outputs);
        }
        if self.job_replies.get(&id).is_some_and(|(_, bg)| *bg)
            && let Some((job, _)) = self.job_replies.remove(&id)
        {
            let r = self.job_json(&id).map(answer);
            self.finish(job, r);
        }
    }

    fn job_done(&mut self, id: String, result: Result<Response>) {
        self.job_ended(&id, &result);
        if let Some((job, _)) = self.job_replies.remove(&id) {
            self.finish(job, result);
        }
    }

    /// A job ended: its state, and a notice when the GUI started it or the
    /// session did (automatic proxies).
    fn job_ended(&mut self, id: &str, result: &Result<Response>) {
        self.job_cancels.remove(id);
        self.dirty = true;
        let Some(j) = Arc::make_mut(&mut self.jobs)
            .iter_mut()
            .find(|j| j.id == id)
        else {
            return;
        };
        j.ended(result);
        if j.origin != (Origin::Gui { drag: false }).name() && j.origin != Origin::Session.name() {
            return;
        }
        let kind = j.kind;
        match result {
            Ok(_) => self.notice(format!("{kind} {id} finished"), false),
            Err(e) if e.code == vidcut_core::ErrorCode::Cancelled => {
                self.notice(format!("{kind} {id} cancelled"), false)
            }
            Err(e) => self.notice(format!("{kind} {id} failed: {}", e.message), true),
        }
    }

    fn job_json(&self, id: &str) -> Result<Value> {
        self.jobs
            .iter()
            .find(|j| j.id == id)
            .map(JobInfo::to_json)
            .ok_or_else(|| Error::not_found(format!("no job {id}")))
    }

    /// Logs and answers a finished request.
    fn finish(&mut self, job: Job, result: Result<Response>) {
        let result = result.map(|mut r| {
            r.via = format!("gui:{}", std::process::id());
            r
        });
        let rev = self.project.as_ref().map_or(0, |p| p.project().rev);
        // Queries of live state, the GUI's own lookups and scrubbing seeks
        // would drown the log.
        let quiet = matches!(
            job.method.as_str(),
            "player.state" | "selection.get" | "gui.log"
        ) || (matches!(job.origin, Origin::Gui { .. })
            && matches!(
                job.method.as_str(),
                "export.plan"
                    | "presets.list"
                    | "presets.get"
                    | "encoders.list"
                    | "jobs.list"
                    | "preferences.get"
            ))
            || (job.method == "player.seek" && job.params["fast"] == json!(true))
            // A fader's drag: the level it ends at is a track.set.
            || (job.method == "mixer.preview" && matches!(job.origin, Origin::Gui { .. }))
            // Previews the GUI asks for (Ripple Delete Silence's stripes).
            || (matches!(job.origin, Origin::Gui { .. }) && job.dry_run);
        if !quiet {
            self.log_seq += 1;
            let entry = LogEntry {
                seq: self.log_seq,
                time: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64()),
                origin: job.origin.name().into(),
                method: job.method.clone(),
                params: job.params.clone(),
                summary: self.summary(&job, &result),
                ok: result.is_ok(),
                error: result.as_ref().err().map(|e| e.message.clone()),
                rev,
                // Track settings, bins and marks change the project without
                // touching media, clips or markers.
                changed: !job.dry_run
                    && (result.as_ref().is_ok_and(|r| !r.changes.is_empty())
                        || (result.is_ok()
                            && job.method != "project.open"
                            && job.rev.is_some_and(|before| before != rev))),
            };
            let log = Arc::make_mut(&mut self.log);
            log.push_back(entry);
            while log.len() > LOG_CAP {
                log.pop_front();
            }
        }
        // The GUI shows failures of what it doesn't wait for itself, except
        // its own seeks overtaken by the next click or scrub.
        if let (Err(e), Origin::Gui { .. }, None) = (&result, job.origin, &job.reply)
            && e.code != vidcut_core::ErrorCode::Conflict
        {
            self.notice(format!("{}: {}", job.method, e.message), true);
        }
        self.dirty = true;
        self.tool_notice(&job, &result);
        self.auto_proxies(&job, &result);
        if let Some(r) = job.reply {
            self.answers.push((r, result));
        }
    }

    /// A log entry's words: the request described, and for an undo or
    /// redo, the steps it took.
    fn summary(&self, job: &Job, result: &Result<Response>) -> String {
        let project = self.project.as_ref().map(|o| o.project());
        let said = vidcut_core::describe::describe(&job.method, &job.params, project);
        let steps = |key: &str| {
            result
                .as_ref()
                .ok()
                .and_then(|r| r.result[key].as_array())
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("; ")
                })
        };
        match job.method.as_str() {
            "history.undo" => steps("undone").map_or(said, |s| format!("Undo: {s}")),
            "history.redo" => steps("redone").map_or(said, |s| format!("Redo: {s}")),
            _ => said,
        }
    }

    /// What an automatic tool started from the GUI did, in the status bar:
    /// it may find nothing, and then nothing on screen changes.
    fn tool_notice(&mut self, job: &Job, result: &Result<Response>) {
        let Ok(r) = result else {
            return;
        };
        if !matches!(job.origin, Origin::Gui { .. }) || job.dry_run {
            return;
        }
        let count = |key: &str| r.result[key].as_array().map_or(0, Vec::len);
        let text = match job.method.as_str() {
            "edit.detect_scene_cuts" => match count("cuts") {
                0 => "Detect Scene Cuts: no shot changes in the selected clips".to_string(),
                1 => "Detect Scene Cuts: split at 1 shot change".to_string(),
                n => format!("Detect Scene Cuts: split at {n} shot changes"),
            },
            "edit.ripple_delete_silence" => match count("removed") {
                0 => "Ripple Delete Silence: no silences in the selected clips".to_string(),
                n => {
                    let secs = r.result["frames"].as_f64().unwrap_or(0.0)
                        / self
                            .project
                            .as_ref()
                            .map_or(1.0, |p| p.project().settings.fps.as_f64());
                    format!("Ripple Delete Silence: cut {n} silence(s), {secs:.1} s")
                }
            },
            _ => return,
        };
        self.notice(text, false);
    }

    /// While the project plays proxies, media the automatic rules pick get
    /// one without being asked (decision D41): those an import just added,
    /// and the pool's when proxies are turned on. The encode is the
    /// session's own `proxy.generate`, a job like any other; media already
    /// being encoded are left out.
    fn auto_proxies(&mut self, job: &Job, result: &Result<Response>) {
        let Ok(response) = result else {
            return;
        };
        let added: Option<Vec<&str>> = match job.method.as_str() {
            _ if job.dry_run => return,
            "media.add" | "timeline.import" => Some(
                response
                    .changes
                    .created
                    .iter()
                    .map(String::as_str)
                    .collect(),
            ),
            "project.set" if job.params["use_proxies"] == json!(true) => None,
            _ => return,
        };
        let Some(open) = &self.project else {
            return;
        };
        let p = open.project();
        if !p.settings.use_proxies {
            return;
        }
        let rules = match crate::preferences::load() {
            Ok(prefs) if prefs.proxies.auto => prefs.proxies,
            _ => return,
        };
        let busy = self.proxies_encoding(p);
        let ids: Vec<&str> = p
            .media
            .iter()
            .filter(|m| added.as_ref().is_none_or(|a| a.contains(&m.id.as_str())))
            .filter(|m| rules.wants(m) && m.fresh_proxy().is_none() && !busy.contains(&m.id))
            .map(|m| m.id.as_str())
            .collect();
        if ids.is_empty() {
            return;
        }
        let request =
            json!({ "method": "proxy.generate", "params": { "media": ids, "auto": true } });
        if let Ok(envelope) = Envelope::from_json(request) {
            let _ = self.tx.send(Msg::Request {
                envelope,
                origin: Origin::Session,
                project: None,
                reply: None,
            });
        }
    }

    /// The media running proxy jobs are encoding.
    fn proxies_encoding(&self, p: &Project) -> HashSet<String> {
        let mut ids = HashSet::new();
        for j in self
            .jobs
            .iter()
            .filter(|j| j.kind == "proxy" && j.running())
        {
            match &j.params["media"] {
                Value::String(id) => {
                    ids.insert(id.clone());
                }
                Value::Array(list) => {
                    ids.extend(list.iter().filter_map(|v| v.as_str().map(String::from)));
                }
                // No media named: every video in the pool.
                _ => ids.extend(p.media.iter().map(|m| m.id.clone())),
            }
        }
        ids
    }

    /// Runs `task` on its own thread; it reports back with `Msg::TaskDone`.
    fn spawn_task(&mut self, mut job: Job, work: Work, task: Task) {
        let tx = self.tx.clone();
        let reply = job.reply.take();
        let progress = reply.as_ref().map(|r| (r.tx.clone(), r.cancel.clone()));
        let _ = crate::threads::spawn("vidcut-task", work, move || {
            let result = task(progress);
            job.reply = reply;
            let _ = tx.send(Msg::TaskDone { job, result });
        });
    }

    fn dispatch(&mut self, envelope: Envelope, job: &Job) -> Step {
        let request = envelope.request.clone();
        // Live requests look at or move the player: they see the timeline
        // as edited, not as last loaded (a reload after edits waits up to
        // RELOAD_EVERY to coalesce them). Seeks and steps load at their
        // target themselves; opening a project replaces it.
        let loads = matches!(
            request,
            Request::PlayerSeek { .. } | Request::PlayerStep { .. } | Request::ProjectOpen { .. }
        );
        if request.is_live() && !loads && self.timeline_due.take().is_some() {
            self.refresh_timeline();
        }
        match request {
            r if r.is_projectless() => Step::Task(
                Work::Interactive,
                Box::new(move |_| crate::session::run(None, envelope)),
            ),
            Request::ProjectOpen {
                path,
                create,
                fps,
                width,
                height,
                drop_frame,
                start_tc,
            } => Step::Now(
                new_settings(
                    &path,
                    create,
                    fps.as_deref(),
                    (width, height),
                    drop_frame,
                    start_tc.as_ref(),
                )
                .and_then(|settings| self.open_project(&path, settings))
                .map(|()| self.state_response()),
            ),
            Request::ViewerSet { mode } => {
                let motion = self.set_viewer(mode);
                self.shown(motion)
            }
            Request::ViewerLoad { media, at } => match self.load_source(&media, at.as_ref()) {
                Ok(motion) => self.shown(motion),
                Err(e) => Step::Now(Err(e)),
            },
            Request::PlayerPlay { speed, viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                let speed = speed.unwrap_or(1.0);
                if !speed.is_finite() || speed == 0.0 || speed.abs() > 64.0 {
                    return Step::Now(Err(Error::usage("speed must be non-zero, within ±64")));
                }
                self.player.play(speed);
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerPause { viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                self.player.pause();
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerToggle { viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                if self.player.playing() {
                    self.player.pause();
                } else {
                    self.player.play(1.0);
                }
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerSeek { at, viewer, .. } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                let id = PlayerId::of(self.viewer);
                let target = match self.viewer_context() {
                    Ok(c) => c.resolve_from(&at, self.player.position(id)),
                    Err(e) => return Step::Now(Err(e)),
                };
                match target {
                    Ok(frame) => {
                        let motion = self.seek(id, frame, true);
                        self.shown(motion)
                    }
                    Err(e) => Step::Now(Err(e.into())),
                }
            }
            Request::PlayerStep { frames, viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                let id = PlayerId::of(self.viewer);
                if id == PlayerId::Timeline && self.timeline_due.take().is_some() {
                    self.refresh_timeline();
                }
                let motion = self.player.step(frames);
                self.shown(Some(motion))
            }
            Request::PlayerState {} => Step::Now(Ok(self.state_response())),
            Request::MixerPreview { track, volume_db } => {
                match self.player.preview_fader(&track, volume_db) {
                    None => Step::Now(Ok(answer(json!({ "live": false })))),
                    Some(true) => Step::Now(Ok(answer(json!({ "live": true })))),
                    Some(false) => Step::Now(Err(Error::media(format!(
                        "{track} has no sound in the timeline's mix"
                    )))),
                }
            }
            Request::PlayerAudioScrubbing { on } => {
                self.audio_scrubbing = on.unwrap_or(!self.audio_scrubbing);
                self.player.set_scrubbing(self.audio_scrubbing);
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerMute { mute } => {
                let mute = mute.unwrap_or(!self.player.muted());
                self.player.set_muted(mute);
                Step::Now(Ok(self.state_response()))
            }
            Request::PoolSet {
                bin,
                sort,
                descending,
            } => {
                if let Some(b) = bin {
                    let known = b.is_empty()
                        || self
                            .project
                            .as_ref()
                            .is_some_and(|o| o.project().bins.contains(&b));
                    if !known {
                        return Step::Now(Err(Error::not_found(format!("no bin {b:?}"))));
                    }
                    self.pool.bin = b;
                }
                if let Some(s) = sort {
                    self.pool.sort = s;
                }
                if let Some(d) = descending {
                    self.pool.descending = d;
                }
                self.refresh_source();
                Step::Now(Ok(answer(json!(self.pool))))
            }
            Request::SelectionGet {} => Step::Now(Ok(answer(json!(self.selection)))),
            Request::SelectionSet { clips, media } => Step::Now(
                self.set_selection(clips, media)
                    .map(|()| answer(json!(self.selection))),
            ),
            Request::GuiScreenshot { out, target } => match target {
                // The picture as shown once the player has settled (a seek
                // already waited for that; a reload may still be on its way).
                ScreenshotTarget::Viewer => {
                    if self.settled() {
                        let id = self.capture_viewer();
                        Step::Wait(Until::Capture {
                            id,
                            out,
                            player: Some(PlayerId::of(self.viewer)),
                        })
                    } else {
                        Step::Wait(Until::Settled { out })
                    }
                }
                ScreenshotTarget::Window => {
                    let id = self.next_capture;
                    self.next_capture += 1;
                    let tx = self.tx.clone();
                    self.renderer.capture_window(Box::new(move |result| {
                        let _ = tx.send(Msg::Captured { id, result });
                    }));
                    Step::Wait(Until::Capture {
                        id,
                        out,
                        player: None,
                    })
                }
            },
            Request::GuiLog { since, limit } => {
                let since = since.unwrap_or(0);
                let entries: Vec<&LogEntry> = self.log.iter().filter(|e| e.seq > since).collect();
                let limit = limit.unwrap_or(100).max(1);
                let skip = entries.len().saturating_sub(limit);
                Step::Now(Ok(answer(json!(entries[skip..]))))
            }
            Request::GuiWindow {
                size,
                minimized,
                maximized,
                fullscreen,
            } => {
                if let Some([w, h]) = size
                    && !(w >= 320.0 && h >= 240.0 && w <= 16384.0 && h <= 16384.0)
                {
                    return Step::Now(Err(Error::usage(
                        "size must be within 320x240..16384x16384",
                    )));
                }
                self.renderer.window(WindowChange {
                    size,
                    minimized,
                    maximized,
                    fullscreen,
                });
                Step::Now(Ok(answer(Value::Null)))
            }
            Request::GuiQuit {} => {
                self.renderer.quit();
                Step::Now(Ok(answer(Value::Null)))
            }
            Request::GuiShow { panel } => {
                self.renderer.show(panel);
                Step::Now(Ok(answer(Value::Null)))
            }
            Request::JobsList {} => Step::Now(Ok(answer(json!(
                self.jobs.iter().map(JobInfo::to_json).collect::<Vec<_>>()
            )))),
            Request::JobsGet { job } => Step::Now(self.job_json(&job).map(answer)),
            Request::JobsCancel { job } => {
                if let Some(c) = self.job_cancels.get(&job) {
                    c.store(true, Ordering::Relaxed);
                }
                Step::Now(self.job_json(&job).map(answer))
            }
            Request::JobsWait { job, .. } => match self.job_json(&job) {
                Ok(_) => Step::Wait(Until::Job { id: job }),
                Err(e) => Step::Now(Err(e)),
            },
            // Decoding work runs on a worker, on a copy of the project as it
            // is now.
            Request::ExportStart(_)
            | Request::ExportPlan(_)
            | Request::FrameRender { .. }
            | Request::StoryboardRender { .. }
            | Request::AnalyzeSilence { .. }
            | Request::AnalyzeScenes { .. }
            | Request::AnalyzeBlack { .. }
            | Request::AnalyzeLoudness { .. } => self.project_task(envelope, job),
            Request::EditRippleDeleteSilence { .. } | Request::EditDetectSceneCuts { .. } => {
                match self.measuring(&envelope) {
                    Some(measure) => Step::Measure(measure, Box::new(envelope)),
                    None => Step::Now(self.edit(envelope, job.origin)),
                }
            }
            // Proxies are encoded on a worker, by the same request on a copy
            // of the project; the edit then finds the files and records them
            // at once (decision D41).
            Request::ProxyGenerate { .. } if !envelope.dry_run => {
                let Some(open) = &self.project else {
                    return Step::Now(Err(no_project()));
                };
                let (project, path) = (open.project().clone(), open.store.path().to_path_buf());
                let title = proxies_title(&project, &envelope.request);
                let copy = envelope.clone();
                let encode = move || {
                    let env = AvEnv::for_project(&path);
                    vidcut_core::Editor::new(project, Default::default())
                        .apply(copy, &env, None)
                        .map(drop)
                };
                Step::Encode(Box::new(encode), Box::new(envelope), title)
            }
            // An `.otioz` bundle's media are unpacked on a worker first; the
            // edit then finds them there.
            Request::TimelineImport {
                path: Some(path),
                unpack,
                ..
            } if vidcut_core::editor::is_otioz(&path) && !envelope.dry_run => {
                let unpack_bundle = move || {
                    use vidcut_core::Env;
                    AvEnv::new()
                        .open_timeline(&path, unpack.as_deref(), true)
                        .map(drop)
                };
                Step::Measure(Box::new(unpack_bundle), Box::new(envelope))
            }
            _ => Step::Now(self.edit(envelope, job.origin)),
        }
    }

    /// Replies once `motion` is on screen.
    fn shown(&mut self, motion: Option<u64>) -> Step {
        match motion {
            Some(m) => Step::Wait(Until::Shown {
                motion: m,
                at: self.player.target,
            }),
            None => Step::Now(Ok(self.state_response())),
        }
    }

    /// Exports, frame renders and measurements run on their own thread, on
    /// a copy of the project as it is now (whatever edits follow), reusing
    /// the headless session code.
    fn project_task(&mut self, envelope: Envelope, _job: &Job) -> Step {
        let Some(open) = &self.project else {
            return Step::Now(Err(no_project()));
        };
        let project = open.project().clone();
        let path = open.store.path().to_path_buf();
        let context = open
            .project()
            .time_context(Some(self.player.position(TIMELINE)));
        let envelope = bind_playhead(envelope, &context);
        // Measurements decode whole files: jobs. Frames and plans are quick,
        // and someone waits for them.
        let work = match envelope.request {
            Request::AnalyzeSilence { .. }
            | Request::AnalyzeScenes { .. }
            | Request::AnalyzeBlack { .. }
            | Request::AnalyzeLoudness { .. }
            | Request::StoryboardRender { .. } => Work::Job,
            _ => Work::Interactive,
        };
        Step::Task(
            work,
            Box::new(move |progress| {
                let cancel_flag = progress
                    .as_ref()
                    .map(|(_, c)| c.clone())
                    .unwrap_or_default();
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut report =
                    |i: usize, n: usize, out: &Path, p: vidcut_av::export::Progress| {
                        if let Some((tx, _)) = &progress
                            && (p.frames == p.total || last.elapsed() >= Duration::from_millis(200))
                        {
                            last = Instant::now();
                            let _ = tx.send(ReplyEvent::Progress(json!({
                                "output": i, "outputs": n, "path": out,
                                "frames": p.frames, "total": p.total,
                                "pass": p.pass, "passes": p.passes, "measuring": p.measuring,
                                "elapsed": p.elapsed.as_secs_f64(),
                            })));
                        }
                    };
                crate::session::render(
                    &project,
                    &path,
                    &envelope,
                    &mut crate::session::Hooks {
                        cancel: &cancel_flag,
                        progress: &mut report,
                    },
                )
            }),
        )
    }

    /// Project requests: edits and queries through the editor.
    fn edit(&mut self, mut envelope: Envelope, origin: Origin) -> Result<Response> {
        let playhead = self.player.position(TIMELINE);
        match &mut envelope.request {
            Request::EditPaste { clips, .. } if clips.is_none() => {
                if self.clipboard.is_empty() {
                    return Err(Error::usage("the clipboard is empty"));
                }
                *clips = Some(self.clipboard.clone());
            }
            Request::MediaMarksSet {
                media,
                mark_in,
                out,
            } => {
                // `playhead` and relative times in source marks refer to the
                // source playhead when the viewer shows that media.
                let base = self.source_frame_of(media);
                for spec in [mark_in, out].into_iter().flatten() {
                    if let Some(frame) = base
                        && matches!(
                            spec,
                            TimeSpec::Keyword(Keyword::Playhead) | TimeSpec::Relative(..)
                        )
                    {
                        let resolved = self.project.as_ref().and_then(|p| {
                            let m = p.project().media(media)?;
                            media_context(p.project(), m, Some(frame))
                                .resolve_from(spec, frame)
                                .ok()
                        });
                        if let Some(f) = resolved {
                            *spec = TimeSpec::Frames(f);
                        }
                    }
                }
            }
            _ => {}
        }
        let env = self
            .project
            .as_ref()
            .map(|open| AvEnv::for_project(open.store.path()))
            .unwrap_or_default();
        let Some(open) = self.project.as_mut() else {
            return Err(no_project());
        };
        let request = envelope.request.clone();
        let (mut result, changed) = open.apply(envelope, &env, Some(playhead));
        if let Some(e) = open.history_error.take() {
            let warning = crate::session::history_unsaved(&e);
            self.notice(warning.clone(), true);
            if let Ok(r) = result.as_mut() {
                r.warnings.push(warning);
            }
        }
        if changed
            && let Request::BinsRename { path, to } = &request
            && let Some(rest) = self.pool.bin.strip_prefix(path.as_str())
            && (rest.is_empty() || rest.starts_with('/'))
        {
            // The pool keeps showing the bin under its new name.
            self.pool.bin = format!("{to}{rest}");
        }
        if changed {
            self.project_changed(result.as_ref().ok());
        }
        if let Ok(r) = &result {
            match &request {
                Request::EditCopy { .. } | Request::EditCut { .. } => {
                    if let Ok(clips) = serde_json::from_value(r.result["clips"].clone()) {
                        self.clipboard = clips;
                    }
                }
                _ => {}
            }
            // Decision D15, for edit functions: a media dropped on the
            // timeline leaves the playhead where it was.
            if !r.dry_run && is_edit_function(&request) && origin != (Origin::Gui { drag: true }) {
                self.playhead_after_edit(r);
            }
        }
        result
    }

    // ---------------------------------------------------------------- project

    /// Opens `path`, first creating it with `create`'s settings if it
    /// doesn't exist.
    fn open_project(&mut self, path: &Path, create: Option<Settings>) -> Result<()> {
        let open = OpenProject::open(path, create, self.tx.clone())?;
        self.project = Some(open);
        self.selection = Selection::default();
        self.source_media = None;
        self.source_content = None;
        self.source_positions.clear();
        self.tape_position = 0;
        self.player.unload(SOURCE);
        self.player.unload(TIMELINE);
        // The last project's pictures and sounds: its media ids name other
        // files here.
        self.thumbs = Arc::new(HashMap::new());
        self.thumbs_wanted.clear();
        self.waveforms = Arc::new(HashMap::new());
        self.waveforms_wanted.clear();
        self.remember_recent(path);
        self.register();
        self.project_changed(None);
        Ok(())
    }

    fn remember_recent(&self, path: &Path) {
        let file = paths::state_dir().join("recent.json");
        let canonical = vidcut_core::dirs::canonical(path).unwrap_or(path.to_path_buf());
        let mut recent: Vec<PathBuf> = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        recent.retain(|p| p != &canonical);
        recent.insert(0, canonical);
        recent.truncate(10);
        if std::fs::create_dir_all(paths::state_dir()).is_ok() {
            let _ = std::fs::write(&file, json!(recent).to_string());
        }
    }

    /// After any change to the project (ours or from outside).
    fn project_changed(&mut self, response: Option<&Response>) {
        let Some(open) = &self.project else {
            self.project_snap = None;
            self.missing_watch.watch(Vec::new());
            return;
        };
        let p = open.project();
        self.missing_watch.watch(
            p.media
                .iter()
                .map(|m| (m.id.clone(), m.path.clone()))
                .collect(),
        );
        let history = open.editor.history();
        self.project_snap = Some(ProjectSnap {
            path: open.store.path().to_path_buf(),
            project: Arc::new(p.clone()),
            undo: history.undo.iter().rev().map(|e| e.label.clone()).collect(),
            redo: history.redo.iter().rev().map(|e| e.label.clone()).collect(),
        });
        // Forget what no longer exists.
        let media: HashSet<String> = p.media.iter().map(|m| m.id.clone()).collect();
        self.selection.media.retain(|m| media.contains(m));
        self.selection.clips.retain(|c| p.clip(c).is_some());
        if self
            .source_media
            .as_ref()
            .is_some_and(|m| !media.contains(m))
        {
            self.source_media = None;
        }
        let _ = response;
        self.check_media();
        self.schedule_timeline();
        self.refresh_source();
        self.dirty = true;
    }

    /// Starts thumbnails and waveforms for new media.
    fn check_media(&mut self) {
        let Some(open) = &self.project else { return };
        let media: Vec<_> = open.project().media.clone();
        // The meters' envelopes are mixed to the project's channels, so a
        // change of those measures again.
        let channels = open.project().settings.channels;
        for m in media {
            let version = media_version(&m);
            if m.info.video.is_some() && self.thumbs_wanted.get(&m.id) != Some(&version) {
                self.thumbs_wanted.insert(m.id.clone(), version.clone());
                // The pictures of the file it was are not this one's.
                if self.thumbs.contains_key(&m.id) {
                    let mut map = (*self.thumbs).clone();
                    map.remove(&m.id);
                    self.thumbs = Arc::new(map);
                }
                let (tx, id, media) = (self.tx.clone(), m.id.clone(), m.clone());
                let wanted = version.clone();
                self.workers.run(move || {
                    let _ = tx.send(Msg::Thumbs {
                        media: id,
                        wanted,
                        result: filmstrip(&media),
                    });
                });
            }
            let sound = format!("{version}|{channels}ch");
            if !m.info.audio.is_empty() && self.waveforms_wanted.get(&m.id) != Some(&sound) {
                self.waveforms_wanted.insert(m.id.clone(), sound.clone());
                if self.waveforms.contains_key(&m.id) {
                    let mut map = (*self.waveforms).clone();
                    map.remove(&m.id);
                    self.waveforms = Arc::new(map);
                }
                let (tx, id, path) = (self.tx.clone(), m.id.clone(), m.path.clone());
                let wanted = sound;
                let cache = paths::media_cache("waveforms", &m, &format!("peaks-{channels}ch"));
                self.workers.run(move || {
                    let cached = std::fs::read(&cache)
                        .ok()
                        .and_then(|b| vidcut_av::waveform::Waveform::from_bytes(&b));
                    let result = match cached {
                        Some(w) => Ok(w),
                        None => vidcut_av::waveform::waveform(&path, 0, WAVEFORM_RATE, channels)
                            .map_err(|e| e.to_string())
                            .inspect(|w| {
                                if let Some(dir) = cache.parent()
                                    && std::fs::create_dir_all(dir).is_ok()
                                {
                                    let _ = std::fs::write(&cache, w.to_bytes());
                                }
                            }),
                    };
                    let _ = tx.send(Msg::Waveform {
                        media: id,
                        wanted,
                        result,
                    });
                });
            }
        }
    }

    // ---------------------------------------------------------------- previews

    /// What a player plays of `plan`: fresh proxies' pictures where the
    /// project plays proxies (decision D41); export plans never do this.
    fn preview_plan(&self, mut plan: RenderPlan) -> RenderPlan {
        if let Some(open) = &self.project {
            vidcut_core::preview::apply_proxies(&mut plan, open.project(), |path| path.is_file());
        }
        plan
    }

    /// Reloads the timeline soon: at once if the last reload was a while
    /// ago, otherwise when [`RELOAD_EVERY`] has passed (edits in quick
    /// succession coalesce).
    fn schedule_timeline(&mut self) {
        let now = Instant::now();
        let due = match self.timeline_loaded_at {
            Some(at) if now < at + RELOAD_EVERY => at + RELOAD_EVERY,
            _ => now,
        };
        self.timeline_due = Some(self.timeline_due.map_or(due, |d| d.min(due)));
    }

    fn refresh_timeline(&mut self) -> Option<u64> {
        self.refresh_timeline_at(None)
    }

    /// Reloads the timeline. `at` is a seek loading at its own frame;
    /// otherwise the player stays where it is, playing on if it plays.
    fn refresh_timeline_at(&mut self, at: Option<i64>) -> Option<u64> {
        let p = self.project.as_ref()?.project().clone();
        let end = p.end();
        if end == 0 {
            self.player.unload(TIMELINE);
            return None;
        }
        let (plan, lanes) = match render::flatten_lanes(&p, 0, end) {
            Ok(planned) => planned,
            Err(e) => {
                self.notice(format!("timeline preview: {}", e.message), true);
                return None;
            }
        };
        let plan = self.preview_plan(plan);
        self.timeline_loaded_at = Some(Instant::now());
        self.player.load(TIMELINE, plan, lanes, at)
    }

    /// Loads what the current source mode wants as the source (nothing
    /// happens if that is already loaded). Shown or not: the frame of what
    /// isn't shown is kept ready.
    fn refresh_source(&mut self) -> Option<u64> {
        let p = self.project.as_ref()?.project().clone();
        // Source Tape: the pool view's bin, in its order. A removed
        // bin's media went to its parent, so the pool shows that.
        while !self.pool.bin.is_empty() && !p.bins.contains(&self.pool.bin) {
            let parent = self.pool.bin.rsplit_once('/').map_or("", |(a, _)| a);
            self.pool.bin = parent.to_string();
        }
        let all: Vec<String> = self.pool.media(&p);
        let tape = render::tape(&p, &all).map(|(_, e)| e).unwrap_or_default();
        self.tape = Arc::new(tape);
        let want = match self.last_source {
            ViewerMode::SourceTape => SourceContent::Tape,
            _ => match &self.source_media {
                Some(m) => SourceContent::Clip(m.clone()),
                None => {
                    self.stash_source_position();
                    self.source_content = None;
                    self.player.unload(SOURCE);
                    return None;
                }
            },
        };
        let ids = match &want {
            SourceContent::Tape => all,
            SourceContent::Clip(m) => vec![m.clone()],
        };
        let Ok((plan, _)) = render::tape(&p, &ids) else {
            self.source_content = None;
            self.player.unload(SOURCE);
            return None;
        };
        // Same content (perhaps changed media): stay where it is, playing
        // on if it plays. Other content opens where it was left.
        let at = if self.source_content.as_ref() == Some(&want) {
            None
        } else {
            self.stash_source_position();
            Some(match &want {
                SourceContent::Tape => self.tape_position,
                SourceContent::Clip(m) => self.source_positions.get(m).copied().unwrap_or(0),
            })
        };
        let plan = self.preview_plan(plan);
        self.source_content = Some(want);
        self.player.load(SOURCE, plan, Vec::new(), at)
    }

    /// Remembers the source playhead of the content being replaced.
    fn stash_source_position(&mut self) {
        let pos = self.player.position(SOURCE);
        match &self.source_content {
            Some(SourceContent::Clip(m)) => {
                self.source_positions.insert(m.clone(), pos);
            }
            Some(SourceContent::Tape) => self.tape_position = pos,
            None => {}
        }
    }

    /// In Source Tape mode the clip under the playhead is selected in the
    /// pool (spec §4.1).
    fn sync_tape_selection(&mut self) {
        if self.viewer != ViewerMode::SourceTape || self.source_content != Some(SourceContent::Tape)
        {
            self.tape_media = None;
            return;
        }
        let under = tape_at(&self.tape, self.player.position(SOURCE)).map(|e| e.media.clone());
        if under != self.tape_media {
            self.tape_media = under.clone();
            if let Some(m) = under {
                self.selection.media = vec![m];
                self.dirty = true;
            }
        }
    }

    // ---------------------------------------------------------------- viewer

    /// Shows `mode`. Switching between the source and the timeline pauses
    /// the one left and shows the other where its playhead is.
    fn set_viewer(&mut self, mode: ViewerMode) -> Option<u64> {
        if mode == self.viewer && (!mode.is_source() || mode == self.last_source) {
            return None;
        }
        self.viewer = mode;
        if mode.is_source() {
            self.last_source = mode;
            // Loaded first: switching then plays it.
            let loaded = self.refresh_source();
            return self.player.show(SOURCE).or(loaded);
        }
        self.player.show(TIMELINE)
    }

    /// Opens `media` as Source Clip (double-click in the pool).
    fn load_source(&mut self, media: &str, at: Option<&TimeSpec>) -> Result<Option<u64>> {
        let open = self.project.as_ref().ok_or_else(no_project)?;
        let p = open.project();
        let m = p
            .media(media)
            .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
        let position = match at {
            Some(t) => {
                let current = self.source_positions.get(media).copied().unwrap_or(0);
                Some(media_context(p, m, Some(current)).resolve_from(t, current)?)
            }
            None => None,
        };
        self.stash_source_position();
        if let Some(pos) = position {
            self.source_positions.insert(media.to_string(), pos);
        }
        self.source_media = Some(media.to_string());
        self.viewer = ViewerMode::SourceClip;
        self.last_source = ViewerMode::SourceClip;
        let loaded = self.refresh_source();
        let motion = self.player.show(SOURCE).or(loaded);
        Ok(match (motion, position) {
            (Some(m), _) => Some(m),
            // Same clip already shown: just move to the requested frame.
            (None, Some(pos)) => self.seek(SOURCE, pos, true),
            (None, None) => None,
        })
    }

    /// Moves `id`'s playhead to `frame`; `sound`: a seek someone asked for,
    /// which plays its frame's sound while paused (audio scrubbing), where
    /// the viewer's own moves (after an edit) play none. The motion, when
    /// the viewer shows `id`.
    fn seek(&mut self, id: PlayerId, frame: i64, sound: bool) -> Option<u64> {
        if id == TIMELINE && self.timeline_due.take().is_some() {
            // A reload is pending: do it now, loading at the new position.
            if let Some(motion) = self.refresh_timeline_at(Some(frame.max(0))) {
                return Some(motion);
            }
        }
        self.player.seek(id, frame, sound)
    }

    /// The context for times typed at the viewer: timeline times, or source
    /// times of the shown clip or tape.
    fn viewer_context(&self) -> Result<TimeContext> {
        let open = self.project.as_ref().ok_or_else(no_project)?;
        let p = open.project();
        Ok(match self.viewer {
            ViewerMode::Timeline => p.time_context(Some(self.player.position(TIMELINE))),
            ViewerMode::SourceClip => {
                let position = self.player.position(SOURCE);
                match self.source_media.as_ref().and_then(|m| p.media(m)) {
                    Some(m) => media_context(p, m, Some(position)),
                    None => return Err(Error::usage("no source clip is open")),
                }
            }
            ViewerMode::SourceTape => {
                let position = self.player.position(SOURCE);
                let entry = tape_at(&self.tape, position);
                let marks = entry
                    .and_then(|e| Some((e, p.media(&e.media)?)))
                    .map(|(e, m)| {
                        (
                            m.marks.mark_in.map(|i| e.start + i),
                            m.marks.mark_out.map(|o| e.start + o),
                        )
                    })
                    .unwrap_or_default();
                TimeContext {
                    fps: p.settings.fps,
                    start_tc: 0,
                    drop_frame: p.settings.counts_drop_frame(),
                    end: self.tape.last().map_or(0, |e| e.start + e.frames),
                    mark_in: marks.0,
                    mark_out: marks.1,
                    playhead: Some(position),
                }
            }
        })
    }

    /// The source frame of `media` under the source playhead, if the viewer
    /// shows that media.
    fn source_frame_of(&self, media: &str) -> Option<i64> {
        let position = self.player.position(SOURCE);
        match (&self.source_content, self.viewer) {
            (Some(SourceContent::Clip(m)), ViewerMode::SourceClip) if m == media => Some(position),
            (Some(SourceContent::Tape), ViewerMode::SourceTape) => tape_at(&self.tape, position)
                .filter(|e| e.media == media)
                .map(|e| position - e.start),
            _ => None,
        }
    }

    /// Decision D15: after an edit function the timeline playhead moves to
    /// the frame after the new clip(s), ready for the next edit. Those are
    /// the clips its result names: `changes.created` also has what is left
    /// of a clip the edit cut into, after the new ones.
    fn playhead_after_edit(&mut self, r: &Response) {
        let Some(open) = &self.project else { return };
        let p = open.project();
        let placed: Vec<&str> = match &r.result["clips"] {
            Value::Array(ids) => ids.iter().filter_map(Value::as_str).collect(),
            _ => r.result["clip"].as_str().into_iter().collect(),
        };
        let end = placed
            .into_iter()
            .filter_map(|id| p.clip(id))
            .map(Clip::end)
            .max();
        if let Some(end) = end {
            self.seek(TIMELINE, end, false);
        }
    }

    fn set_selection(
        &mut self,
        clips: Option<OneOrMany<String>>,
        media: Option<OneOrMany<String>>,
    ) -> Result<()> {
        let open = self.project.as_ref().ok_or_else(no_project)?;
        let p = open.project();
        let playhead = Some(self.player.position(TIMELINE));
        let clips = clips
            .map(|c| {
                c.to_vec()
                    .iter()
                    .map(|r| resolve_clip(p, r, playhead))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        if let Some(m) = &media {
            for id in m.to_vec() {
                p.media(&id)
                    .ok_or_else(|| Error::not_found(format!("no media {id}")))?;
            }
        }
        if let Some(mut c) = clips {
            c.dedup();
            self.selection.clips = c;
        }
        if let Some(m) = media {
            self.selection.media = m.to_vec();
        }
        Ok(())
    }

    // ---------------------------------------------------------------- captures

    fn captured(&mut self, id: u64, result: Result<Image>) {
        let Some(i) = self
            .waits
            .iter()
            .position(|w| matches!(w.until, Until::Capture { id: c, .. } if c == id))
        else {
            return; // timed out already
        };
        let w = self.waits.swap_remove(i);
        let Until::Capture { out, player, .. } = w.until else {
            return;
        };
        // The viewer shows black when its player has nothing loaded (the
        // render target may still hold an older picture).
        let blank = player.is_some_and(|p| !self.player.is_loaded(p));
        let result = result.map(|mut image| {
            if blank {
                image.rgba.as_chunks_mut::<4>().0.fill([0, 0, 0, 255]);
            }
            image
        });
        let state = self.state_response();
        // A screenshot someone waits for.
        self.spawn_task(
            w.job,
            Work::Interactive,
            Box::new(move |_| {
                let image = result?;
                let png =
                    vidcut_av::export::still::png_rgba(image.width, image.height, &image.rgba)
                        .map_err(|e| Error::internal(e.to_string()))?;
                std::fs::write(&out, png)
                    .map_err(|e| Error::usage(format!("{}: {e}", out.display())))?;
                let mut r = state;
                r.result = json!({
                    "path": out,
                    "width": image.width,
                    "height": image.height,
                    "state": r.result,
                });
                Ok(r)
            }),
        );
    }

    /// Whether the viewer shows its latest motion (a seek, a step, an
    /// edit's reload, a switch).
    fn settled(&self) -> bool {
        let p = &self.player;
        p.shown >= p.motion || p.failure().is_some()
    }

    /// Asks the renderer for the viewer's picture; the answer comes as
    /// `Msg::Captured` with the returned id.
    fn capture_viewer(&mut self) -> u64 {
        let id = self.next_capture;
        self.next_capture += 1;
        let tx = self.tx.clone();
        self.renderer.capture_viewer(Box::new(move |result| {
            let _ = tx.send(Msg::Captured { id, result });
        }));
        id
    }

    // ---------------------------------------------------------------- state

    fn state_response(&self) -> Response {
        let rev = self.project.as_ref().map_or(0, |p| p.project().rev);
        let mut r = answer(self.state_json());
        r.rev = rev;
        r
    }

    fn state_json(&self) -> Value {
        let (late, skipped) = self.player.misses();
        let p = self.project.as_ref().map(|o| o.project());
        let fps = p.map_or(Rational::new(30, 1).expect("valid"), |p| p.settings.fps);
        let drop = p.is_some_and(|p| p.settings.counts_drop_frame());
        let tc = |f: i64, stc: i64| format_timecode(f, fps, stc, drop);
        let player = |id: PlayerId, stc: i64| {
            let snap = self.player.snap(id);
            json!({
                "loaded": snap.loaded,
                "frames": snap.frames,
                "position": snap.position,
                "position_tc": tc(snap.position, stc),
                "ready": snap.ready,
                "playing": snap.playing,
                "speed": snap.speed,
                "hwdec": snap.hwdec,
                "error": snap.error,
            })
        };
        let mut source = player(SOURCE, 0);
        let position = self.player.position(SOURCE);
        let under = match &self.source_content {
            Some(SourceContent::Clip(m)) => Some((m.clone(), position)),
            Some(SourceContent::Tape) => {
                tape_at(&self.tape, position).map(|e| (e.media.clone(), position - e.start))
            }
            None => None,
        };
        if let Some((m, f)) = under {
            source["media"] = json!(m);
            source["source_frame"] = json!(f);
        }
        source["content"] = json!(match &self.source_content {
            Some(SourceContent::Clip(_)) => "source_clip",
            Some(SourceContent::Tape) => "source_tape",
            None => "none",
        });
        json!({
            "project": self.project.as_ref().map(|o| o.store.path()),
            "viewer": self.viewer,
            "last_source": self.last_source,
            "source_media": self.source_media,
            "source": source,
            "timeline": player(TIMELINE, p.map_or(0, |p| p.settings.start_tc)),
            // The viewer's: whichever it played.
            "late": late,
            "skipped": skipped,
            "selection": self.selection,
            "pool": self.pool,
            "muted": self.player.muted(),
            // The audio device the sound plays on (the id of the one
            // `player.audio_device` chose), null for the system's default,
            // and why it isn't the one chosen.
            "audio_device": self.player.audio_device(),
            "audio_error": self.player.audio_error,
            "audio_scrubbing": self.audio_scrubbing,
            "clipboard": self.clipboard.len(),
            "preview": self.preview_error.as_deref().map_or(json!(true), |e| json!(e)),
        })
    }
}

/// A background task; gets the client's progress channel and cancel flag.
type Task =
    Box<dyn FnOnce(Option<(Sender<ReplyEvent>, Arc<AtomicBool>)>) -> Result<Response> + Send>;

enum Step {
    Now(Result<Response>),
    Wait(Until),
    /// A task on a thread of its own, at the priority its work calls for.
    Task(Work, Task),
    /// Measure media (or unpack a bundle's) on a worker, then run the
    /// request here (docs/decisions/agents.md §3).
    Measure(Measure, Box<Envelope>),
    /// Encode proxies on a worker as a job titled so (gui.md §9), then run
    /// the request here.
    Encode(Measure, Box<Envelope>, String),
}

/// Slow work on media that a request needs, done before it runs:
/// measuring, unpacking.
type Measure = Box<dyn FnOnce() -> Result<()> + Send>;

fn reply(reply: Option<Reply>, result: Result<Response>) {
    if let Some(r) = reply {
        let _ = r.tx.send(ReplyEvent::Done(result));
    }
}

/// What a `proxy.generate` job is titled: the pool item's name, or how
/// many it names.
fn proxies_title(project: &Project, request: &Request) -> String {
    let Request::ProxyGenerate { media, .. } = request else {
        return String::new();
    };
    match media
        .as_ref()
        .map(|m| m.to_vec())
        .unwrap_or_default()
        .as_slice()
    {
        [] => "proxies for the media pool".into(),
        [id] => project
            .media(id)
            .map_or_else(|| id.clone(), |m| m.name.clone()),
        ids => format!("proxies for {} media", ids.len()),
    }
}

fn answer(result: Value) -> Response {
    Response {
        rev: 0,
        changes: Default::default(),
        result,
        dry_run: false,
        via: String::new(),
        warnings: Vec::new(),
    }
}

/// `jobs.wait`'s answer for an ended job: the job, or how it failed.
fn job_answer(j: &JobInfo) -> Result<Response> {
    match &j.error {
        Some(e) => Err(Error::new(e.code, format!("job {}: {}", j.id, e.message))),
        None => Ok(answer(j.to_json())),
    }
}

/// What `project.open` creates: with `create` and no file at `path`, a
/// project with the format given; `None` opens an existing file. A format
/// needs `create`, and a file that isn't there yet.
fn new_settings(
    path: &Path,
    create: bool,
    fps: Option<&str>,
    (width, height): (Option<u32>, Option<u32>),
    drop_frame: Option<bool>,
    start_tc: Option<&TimeSpec>,
) -> Result<Option<Settings>> {
    let size = crate::session::size(width, height)?;
    let format = fps.is_some() || size.is_some() || drop_frame.is_some() || start_tc.is_some();
    if format && !create {
        return Err(Error::usage("a frame rate or size needs create"));
    }
    if !create {
        return Ok(None);
    }
    if path.exists() {
        if format {
            return Err(Error::invalid(format!(
                "{} already exists: open it, or create another file",
                path.display()
            )));
        }
        return Ok(None);
    }
    let mut settings = Settings::with_format(fps, size)?;
    if let Some(d) = drop_frame {
        settings.set_drop_frame(d)?;
    }
    if let Some(tc) = start_tc {
        settings.set_start_tc(tc)?;
    }
    Ok(Some(settings))
}

fn no_project() -> Error {
    Error::usage("no project is open in this vidcut GUI (project.open opens one)")
}

/// Edit functions that place new clips (spec §7).
fn is_edit_function(r: &Request) -> bool {
    matches!(
        r,
        Request::EditAppend(_)
            | Request::EditSmartInsert { .. }
            | Request::EditInsert { .. }
            | Request::EditOverwrite { .. }
            | Request::EditRippleOverwrite { .. }
            | Request::EditPaste { .. }
    )
}

/// Resolves playhead-based times of timeline frame renders against the live
/// playhead (the headless session code has none).
fn bind_playhead(mut envelope: Envelope, context: &TimeContext) -> Envelope {
    if let Request::FrameRender {
        at, media: None, ..
    } = &mut envelope.request
        && let Some(playhead) = context.playhead
        && matches!(
            at,
            TimeSpec::Keyword(Keyword::Playhead) | TimeSpec::Relative(..)
        )
        && let Ok(f) = context.resolve_from(at, playhead)
    {
        *at = TimeSpec::Frames(f);
    }
    envelope
}
