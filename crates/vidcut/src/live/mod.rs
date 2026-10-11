//! The live session: the running GUI's core thread.
//!
//! One thread owns the open project, both mpv players and everything the
//! viewer shows. The GUI, IPC clients and background jobs reach it only
//! through messages ([`Handle`]). After every change it publishes an
//! immutable [`Snapshot`], which the UI thread reads without waiting: the UI
//! never blocks on the session and never calls mpv's client API.
//!
//! Requests are the same [`Request`]s the CLI runs headless. Project edits go
//! through the project's [`Editor`](vidcut_core::Editor) and are saved before
//! the reply; live-only requests (viewer, players, selection, screenshots)
//! act on the session. Requests that wait for something (a seek waits until
//! its frame is on screen, a screenshot for the picture, an export for the
//! encoder) reply later, without blocking other requests.

mod jobs;
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
use vidcut_core::model::Clip;
use vidcut_core::preview::{self, Fillers};
use vidcut_core::render::{self, RenderPlan, TapeEntry};
use vidcut_core::request::{GuiPanel, OneOrMany, ScreenshotTarget, ViewerMode};
use vidcut_core::time::{Keyword, Rational, TimeContext, TimeSpec, format_timecode};
use vidcut_core::{Envelope, Error, Project, Request, Response, Result};
use vidcut_mpv::Mpv;

pub use jobs::{JobInfo, JobOutput, JobState};
pub use player::{debug, mpv_log_options, mpv_options};
pub use snapshot::*;
pub use vidcut_av::thumbs::Image;

use crate::env::AvEnv;
use crate::ipc::{self, Instance};
use crate::paths;
use player::Player;
use project::OpenProject;
use workers::Workers;

/// Where a request came from (shown in the command log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Gui,
    Ipc,
    Session,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::Ipc => "ipc",
            Self::Session => "session",
        }
    }
}

/// Called with a captured picture (or why there is none).
pub type Capture = Box<dyn FnOnce(Result<Image>) + Send>;

/// Called once a player was presented.
pub type Presented = Box<dyn FnOnce() + Send>;

/// What the session needs from whoever renders the players: the GUI, or a
/// test harness.
pub trait Renderer: Send + Sync {
    /// Draw `player`'s current picture now, even if mpv has nothing new for
    /// it, then call `done`. The session asks once a seek or load has
    /// finished: the viewer then shows its frame (docs/decisions/gui.md §4).
    fn present(&self, player: PlayerId, done: Presented);
    /// The picture `player` shows, as displayed.
    fn capture_viewer(&self, player: PlayerId, done: Capture);
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
    /// Source and timeline players; `None` runs without preview.
    pub players: Option<(Mpv, Mpv)>,
    /// Why there is no preview, if so.
    pub preview_error: Option<String>,
    pub renderer: Arc<dyn Renderer>,
    /// Asks the GUI to repaint (a new snapshot is ready).
    pub notify: Arc<dyn Fn() + Send + Sync>,
    /// The IPC socket this instance serves, for the instance registry.
    pub socket: Option<PathBuf>,
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
    Request {
        envelope: Envelope,
        origin: Origin,
        /// The project the client expects to be open.
        project: Option<PathBuf>,
        reply: Option<Reply>,
    },
    Mpv(PlayerId),
    /// The renderer presented `player` after `motion` finished.
    Presented {
        player: PlayerId,
        motion: u64,
    },
    Captured {
        id: u64,
        result: Result<Image>,
    },
    TaskDone {
        job: Job,
        result: Result<Response>,
    },
    /// What a request needed is measured: now it runs.
    Measured {
        job: Job,
        envelope: Envelope,
        result: Result<()>,
    },
    FileChanged,
    Thumbs {
        media: String,
        result: std::result::Result<Filmstrip, String>,
    },
    Waveform {
        media: String,
        result: std::result::Result<vidcut_av::waveform::Waveform, String>,
    },
    Remuxed {
        source: PathBuf,
        result: std::result::Result<PathBuf, String>,
    },
    /// Whether a media file is on a network file system.
    Storage {
        path: PathBuf,
        network: bool,
    },
    /// A tail check finished: the earliest timestamp mpv would drop, if any.
    TailChecked {
        key: TailKey,
        origin: f64,
        lost: std::result::Result<Option<f64>, String>,
    },
    /// A tail patch was rendered: its file and length in frames.
    TailRendered {
        key: TailKey,
        result: std::result::Result<(PathBuf, i64), String>,
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
        let _ = self.tx.send(Msg::Request {
            envelope,
            origin: Origin::Gui,
            project: None,
            reply: None,
        });
    }

    /// Builds and sends a GUI request from a method name and params.
    pub fn request(&self, method: &str, params: Value) {
        match Envelope::from_json(json!({ "method": method, "params": params })) {
            Ok(e) => self.send(e),
            Err(e) => {
                // A GUI bug: report it like any other failure.
                let _ = self.tx.send(Msg::TaskDone {
                    job: Job {
                        method: method.into(),
                        params,
                        origin: Origin::Gui,
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
    /// notifications and returns false to cancel.
    pub fn call(
        &self,
        envelope: Envelope,
        origin: Origin,
        project: Option<PathBuf>,
        progress: &mut dyn FnMut(Value) -> bool,
    ) -> Result<Response> {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
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
            match rx.recv() {
                Ok(ReplyEvent::Progress(v)) => {
                    if !progress(v) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
                Ok(ReplyEvent::Done(r)) => return r,
                Err(_) => return Err(Error::no_instance("the vidcut GUI is shutting down")),
            }
        }
    }

    /// Stops the session thread.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

/// Starts the session thread.
pub fn start(config: Config) -> (Handle, std::thread::JoinHandle<()>) {
    let (tx, rx) = channel();
    let pid = std::process::id();
    remove_stale_runtime_dirs();
    let edl_dir = ipc::runtime_dir().join(pid.to_string());
    let _ = ipc::private_dir(&edl_dir);
    let (source, timeline) = match config.players {
        Some((s, t)) => (Some(s), Some(t)),
        None => (None, None),
    };
    for (id, mpv) in [(PlayerId::Source, &source), (PlayerId::Timeline, &timeline)] {
        if let Some(m) = mpv {
            let tx = tx.clone();
            m.set_wakeup_callback(move || {
                let _ = tx.send(Msg::Mpv(id));
            });
        }
    }
    let players = [
        Player::new(PlayerId::Source, source, edl_dir.join("source.edl")),
        Player::new(PlayerId::Timeline, timeline, edl_dir.join("timeline.edl")),
    ];
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
        waveforms: Arc::new(HashMap::new()),
        jobs: Arc::new(Vec::new()),
        preview_error: config.preview_error.clone(),
    };
    let shared = Arc::new(Mutex::new(Arc::new(empty)));
    let mut live = Live {
        tx: tx.clone(),
        rx,
        shared: shared.clone(),
        notify: config.notify,
        renderer: config.renderer,
        instance,
        edl_dir,
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
        players,
        selection: Selection::default(),
        pool: PoolView::default(),
        clipboard: Vec::new(),
        log: Arc::new(VecDeque::new()),
        log_seq: 0,
        notices: Arc::new(VecDeque::new()),
        notice_seq: 0,
        thumbs: Arc::new(HashMap::new()),
        thumbs_wanted: HashSet::new(),
        waveforms: Arc::new(HashMap::new()),
        waveforms_wanted: HashSet::new(),
        fillers: HashMap::new(),
        remux: HashMap::new(),
        network: HashMap::new(),
        tails: HashMap::new(),
        tail_failures: HashMap::new(),
        tail_spots: [Vec::new(), Vec::new()],
        workers: Workers::new(2),
        jobs: Arc::new(Vec::new()),
        job_cancels: HashMap::new(),
        job_replies: HashMap::new(),
        next_job: 1,
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
    let thread = std::thread::Builder::new()
        .name("vidcut-session".into())
        .spawn(move || {
            if let Some(path) = project
                && let Err(e) = live.open_project(&path, create)
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

/// Generated filler media, by (width, height, fps).
pub(crate) type FillerKey = (u32, u32, Rational);

enum RemuxState {
    Pending,
    Ready(PathBuf),
    Failed,
}

/// A preview segment end: (file mpv plays, end in project frames from the
/// media's first frame, project fps, project size).
pub(crate) type TailKey = (PathBuf, i64, Rational, u32, u32);

/// Whether mpv would drop the last frames of segments ending there, and the
/// patch that replaces them (docs/decisions/preview.md §9).
enum TailState {
    Checking,
    Clean,
    Rendering,
    Ready { path: PathBuf, frames: i64 },
    Failed,
}

/// Longest tail patch: leading pictures span a few frames at most.
const MAX_TAIL: i64 = 64;

/// Attempts at a tail's check and patch before the preview gives up on it.
const TAIL_TRIES: u8 = 2;

/// Wait before trying a failed tail again.
const TAIL_RETRY: Duration = Duration::from_secs(2);

/// What the source player has loaded.
#[derive(Debug, Clone, PartialEq)]
enum SourceContent {
    Clip(String),
    Tape,
}

/// What a request waits for before replying.
enum Until {
    /// Motion `motion` of `player` is on screen, showing frame `at`. If
    /// the player moves elsewhere first (another client's seek, a person
    /// scrubbing), the frame won't show: the request fails with CONFLICT.
    Shown {
        player: PlayerId,
        motion: u64,
        at: i64,
    },
    /// Job `id` ended (`jobs.wait`).
    Job { id: String },
    /// `player` shows its latest motion with no tail patch pending near it;
    /// then the viewer is captured to `out` (a reload in flight, such as a
    /// patch arriving after a seek returned, may draw black for a moment).
    Settled { player: PlayerId, out: PathBuf },
    /// Capture `id` arrives; then it is written to `out`. For a viewer
    /// capture, `player` is the player shown.
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
            let strip =
                vidcut_av::thumbs::filmstrip(&media.path, THUMBS, THUMB_SIZE.0, THUMB_SIZE.1)
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
    shared: Arc<Mutex<Arc<Snapshot>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    renderer: Arc<dyn Renderer>,
    instance: Option<Instance>,
    edl_dir: PathBuf,
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
    players: [Player; 2],
    selection: Selection,
    pool: PoolView,
    clipboard: Vec<vidcut_core::model::TrackClip>,
    log: Arc<VecDeque<LogEntry>>,
    log_seq: u64,
    notices: Arc<VecDeque<Notice>>,
    notice_seq: u64,
    thumbs: Arc<HashMap<String, Arc<Filmstrip>>>,
    thumbs_wanted: HashSet<String>,
    waveforms: Arc<HashMap<String, Arc<vidcut_av::waveform::Waveform>>>,
    waveforms_wanted: HashSet<String>,
    /// Filler media by format (`None`: generating them failed).
    fillers: HashMap<FillerKey, Option<Fillers>>,
    remux: HashMap<PathBuf, RemuxState>,
    /// Media files on network file systems (true) or not, asked on a worker:
    /// a share that doesn't answer must not stall the session.
    network: HashMap<PathBuf, bool>,
    tails: HashMap<TailKey, TailState>,
    /// How often each tail's check or patch has failed: it is tried again
    /// up to [`TAIL_TRIES`] times (a share may have had a hiccup).
    tail_failures: HashMap<TailKey, u8>,
    /// Where each player's segments end (frames in what it plays) and the
    /// tail that ends there: a seek waits only for checks near its frame.
    tail_spots: [Vec<(i64, TailKey)>; 2],
    workers: Workers,
    /// Export jobs, oldest first.
    jobs: Arc<Vec<JobInfo>>,
    job_cancels: HashMap<String, Arc<AtomicBool>>,
    /// Requests answered when their job is planned (`background`) or done.
    job_replies: HashMap<String, (Job, bool)>,
    next_job: u64,
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

const SOURCE: usize = 0;
const TIMELINE: usize = 1;

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
                Ok(Msg::JobPlanned { id, outputs }) => self.job_planned(id, outputs),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        // A job still running is stuck in a decoder or an encoder, and the
        // process won't wait for it: its partial files go now. Cancelled
        // jobs start no file, so what one writes was planned by now.
        for job in self.jobs.iter() {
            if self.job_cancels.contains_key(&job.id) {
                for o in &job.outputs {
                    let _ = std::fs::remove_file(vidcut_av::export::partial_path(&o.path));
                }
            }
        }
        for w in std::mem::take(&mut self.waits) {
            reply(
                w.job.reply,
                Err(Error::no_instance("the vidcut GUI closed")),
            );
        }
        for (r, result) in self.answers.drain(..) {
            let _ = r.tx.send(ReplyEvent::Done(result));
        }
        if let Some(i) = &self.instance {
            ipc::registry::unregister(i.pid);
        }
        let _ = std::fs::remove_dir_all(&self.edl_dir);
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
            Msg::Mpv(id) => {
                changed = self.players[id.index()].pump();
                if let Some(motion) = self.players[id.index()].present_due() {
                    let tx = self.tx.clone();
                    self.renderer.present(
                        id,
                        Box::new(move || {
                            let _ = tx.send(Msg::Presented { player: id, motion });
                        }),
                    );
                }
            }
            Msg::Presented { player, motion } => {
                self.players[player.index()].presented(motion);
                changed = false;
            }
            Msg::Captured { id, result } => self.captured(id, result),
            Msg::TaskDone { job, result } => self.finish(job, result),
            Msg::Measured {
                job,
                envelope,
                result,
            } => match result {
                Err(e) => self.finish(job, Err(e)),
                Ok(()) => {
                    // The project may have changed meanwhile; this request's
                    // change is measured from now.
                    let rev = self.project.as_ref().map(|p| p.project().rev);
                    let job = Job { rev, ..job };
                    let result = self.edit(envelope);
                    self.finish(job, result);
                }
            },
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
            Msg::Waveform { media, result } => match result {
                Ok(w) => {
                    let mut map = (*self.waveforms).clone();
                    map.insert(media, Arc::new(w));
                    self.waveforms = Arc::new(map);
                }
                Err(e) => self.notice(format!("waveform of {media}: {e}"), true),
            },
            Msg::Thumbs { media, result } => match result {
                Ok(strip) => {
                    let mut map = (*self.thumbs).clone();
                    map.insert(media, Arc::new(strip));
                    self.thumbs = Arc::new(map);
                }
                Err(e) => self.notice(format!("thumbnails for {media}: {e}"), true),
            },
            Msg::Storage { path, network } => {
                self.network.insert(path, network);
                if network {
                    self.schedule_timeline();
                    self.refresh_source();
                } else {
                    changed = false;
                }
            }
            Msg::Remuxed { source, result } => {
                match result {
                    Ok(p) => {
                        self.remux.insert(source, RemuxState::Ready(p));
                    }
                    Err(e) => {
                        self.remux.insert(source.clone(), RemuxState::Failed);
                        self.notice(format!("preview remux of {}: {e}", source.display()), true);
                    }
                }
                self.refresh_timeline();
                self.refresh_source();
            }
            Msg::TailChecked { key, origin, lost } => match lost {
                Ok(None) => {
                    self.tails.insert(key, TailState::Clean);
                    changed = false;
                }
                Ok(Some(t)) => self.render_tail(key, origin, t),
                Err(e) => self.tail_failed(key, &format!("checking a preview cut: {e}")),
            },
            Msg::TailRendered { key, result } => {
                match result {
                    Ok((path, frames)) => {
                        self.tails.insert(key, TailState::Ready { path, frames });
                    }
                    Err(e) => self.tail_failed(key, &format!("preview patch: {e}")),
                }
                self.refresh_timeline();
                self.refresh_source();
            }
            Msg::JobPlanned { id, outputs } => self.job_planned(id, outputs),
            Msg::JobProgress {
                id,
                output,
                frames,
                total,
            } => {
                if let Some(j) = Arc::make_mut(&mut self.jobs)
                    .iter_mut()
                    .find(|j| j.id == id)
                {
                    j.progressed(output, frames, total);
                }
            }
            Msg::JobDone { id, result } => self.job_done(id, result),
            Msg::Shutdown => return false,
        }
        self.dirty |= changed;
        true
    }

    /// Timed work; returns when it next needs to run.
    fn tick(&mut self, now: Instant) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let mut soonest = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        for p in &mut self.players {
            if let Some(t) = p.tick(now) {
                soonest(t);
            }
        }
        if let Some(due) = self.timeline_due {
            if now >= due {
                self.timeline_due = None;
                self.refresh_timeline();
            } else {
                soonest(due);
            }
        }
        self.sync_tape_selection();
        // Requests waiting for frames or captures.
        let mut i = 0;
        while i < self.waits.len() {
            let settled = match &self.waits[i].until {
                Until::Settled { player, .. } if self.settled(*player) => Some(*player),
                _ => None,
            };
            if let Some(player) = settled {
                let id = self.capture_viewer(player);
                if let Until::Settled { out, .. } = &mut self.waits[i].until {
                    let out = std::mem::take(out);
                    self.waits[i].until = Until::Capture {
                        id,
                        out,
                        player: Some(player),
                    };
                }
            }
            let w = &self.waits[i];
            let outcome = match w.until {
                Until::Shown { player, motion, at } => {
                    // The latest motion counts too: a preview reload (say, a
                    // tail patch arriving) replaces what was asked for, at
                    // the same frame.
                    let p = &self.players[player.index()];
                    if p.position != at {
                        Some(Err(Error::conflict(format!(
                            "the viewer moved to frame {} before frame {at} showed \
                             (another seek, or playback)",
                            p.position
                        ))))
                    } else {
                        (p.shown >= motion && self.settled(player)).then(|| match &p.error {
                            Some(e) => Err(Error::media(format!("preview: {e}"))),
                            None => Ok(self.state_response()),
                        })
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
            source: self.players[SOURCE].snap(),
            timeline: self.players[TIMELINE].snap(),
            source_media: self.source_media.clone(),
            tape: self.tape.clone(),
            selection: self.selection.clone(),
            pool: self.pool.clone(),
            clipboard: self.clipboard.len(),
            log: self.log.clone(),
            notices: self.notices.clone(),
            thumbs: self.thumbs.clone(),
            waveforms: self.waveforms.clone(),
            jobs: self.jobs.clone(),
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
            let want = expected.canonicalize().unwrap_or(expected.clone());
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
            Step::Task(task) => self.spawn_task(Job { rev: None, ..job }, task),
            Step::Measure(measure, then) => {
                let tx = self.tx.clone();
                let _ = std::thread::Builder::new()
                    .name("vidcut-measure".into())
                    .spawn(move || {
                        let result = measure();
                        let _ = tx.send(Msg::Measured {
                            job,
                            envelope: *then,
                            result,
                        });
                    });
            }
        }
    }

    /// The measuring a request needs before it runs: the sound or pictures
    /// of the clips it names, whose measurements the media layer then keeps.
    fn measuring(&self, envelope: &Envelope) -> Option<Measure> {
        let p = self.project.as_ref()?.project();
        let playhead = Some(self.players[TIMELINE].position);
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
                    Some(s) => AvEnv.audio_levels(m, *s).map(drop)?,
                    None => AvEnv.picture_stats(m).map(drop)?,
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
        if let Err(e) = envelope.check_rev(open.project().rev) {
            return self.finish(job, Err(e));
        }
        // The job encodes the project as it is now, whatever edits follow.
        let project = open.project().clone();
        let context = open
            .project()
            .time_context(Some(self.players[TIMELINE].position));
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
        let jobs = Arc::make_mut(&mut self.jobs);
        jobs.push(info);
        // Keep running jobs and the most recent finished ones.
        let finished = jobs.iter().filter(|j| !j.running()).count();
        if finished > JOBS_KEPT {
            let mut drop = finished - JOBS_KEPT;
            jobs.retain(|j| {
                let gone = drop > 0 && !j.running();
                drop -= usize::from(gone);
                !gone
            });
        }
        self.job_cancels.insert(id.clone(), cancel.clone());
        if job.reply.is_some() {
            self.job_replies.insert(id.clone(), (job, background));
        } else {
            // GUI requests: logged now, the job shows the rest.
            self.finish(job, Ok(answer(json!({ "job": id }))));
        }
        self.dirty = true;
        let tx = self.tx.clone();
        let _ = std::thread::Builder::new()
            .name(format!("vidcut-{id}"))
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut report =
                    |i: usize, n: usize, out: &Path, p: vidcut_av::export::Progress| {
                        if p.frames != p.total && last.elapsed() < Duration::from_millis(100) {
                            return;
                        }
                        last = Instant::now();
                        let _ = tx.send(Msg::JobProgress {
                            id: id.clone(),
                            output: i,
                            frames: p.frames,
                            total: p.total,
                        });
                        if let Some(ptx) = &progress_tx {
                            let _ = ptx.send(ReplyEvent::Progress(json!({
                                "job": id, "output": i, "outputs": n, "path": out,
                                "frames": p.frames, "total": p.total,
                                "elapsed": p.elapsed.as_secs_f64(),
                            })));
                        }
                    };
                let result = crate::session::export_media(
                    &project,
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
        self.job_cancels.remove(&id);
        let mut gui = false;
        if let Some(j) = Arc::make_mut(&mut self.jobs)
            .iter_mut()
            .find(|j| j.id == id)
        {
            j.ended(&result);
            gui = j.origin == Origin::Gui.name();
        }
        if gui {
            match &result {
                Ok(_) => self.notice(format!("export {id} finished"), false),
                Err(e) if e.code == vidcut_core::ErrorCode::Cancelled => {
                    self.notice(format!("export {id} cancelled"), false)
                }
                Err(e) => self.notice(format!("export {id} failed: {}", e.message), true),
            }
        }
        if let Some((job, _)) = self.job_replies.remove(&id) {
            self.finish(job, result);
        }
        self.dirty = true;
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
        ) || (job.origin == Origin::Gui
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
            // Previews the GUI asks for (Ripple Delete Silence's stripes).
            || (job.origin == Origin::Gui && job.dry_run);
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
        if let (Err(e), Origin::Gui, None) = (&result, job.origin, &job.reply)
            && e.code != vidcut_core::ErrorCode::Conflict
        {
            self.notice(format!("{}: {}", job.method, e.message), true);
        }
        self.dirty = true;
        if let Some(r) = job.reply {
            self.answers.push((r, result));
        }
    }

    /// Runs `task` on its own thread; it reports back with `Msg::TaskDone`.
    fn spawn_task(&mut self, mut job: Job, task: Task) {
        let tx = self.tx.clone();
        let reply = job.reply.take();
        let progress = reply.as_ref().map(|r| (r.tx.clone(), r.cancel.clone()));
        let _ = std::thread::Builder::new()
            .name("vidcut-task".into())
            .spawn(move || {
                let result = task(progress);
                job.reply = reply;
                let _ = tx.send(Msg::TaskDone { job, result });
            });
    }

    fn dispatch(&mut self, envelope: Envelope, job: &Job) -> Step {
        let request = envelope.request.clone();
        // Live requests look at or move the players: they see the timeline
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
            r if r.is_projectless() => {
                Step::Task(Box::new(move |_| crate::session::run(None, envelope)))
            }
            Request::ProjectOpen { path, create } => Step::Now(
                self.open_project(&path, create)
                    .map(|()| self.state_response()),
            ),
            Request::ViewerSet { mode } => {
                let motion = self.set_viewer(mode);
                self.shown(PlayerId::of(mode), motion)
            }
            Request::ViewerLoad { media, at } => match self.load_source(&media, at.as_ref()) {
                Ok(motion) => self.shown(PlayerId::Source, motion),
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
                let id = PlayerId::of(self.viewer);
                self.pause_other(id);
                self.players[id.index()].play(speed);
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerPause { viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                self.players[PlayerId::of(self.viewer).index()].pause();
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerToggle { viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                let id = PlayerId::of(self.viewer);
                if self.players[id.index()].playing() {
                    self.players[id.index()].pause();
                } else {
                    self.pause_other(id);
                    self.players[id.index()].play(1.0);
                }
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerSeek { at, viewer, fast } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                let id = PlayerId::of(self.viewer);
                let target = match self.viewer_context() {
                    Ok(c) => c.resolve_from(&at, self.players[id.index()].position),
                    Err(e) => return Step::Now(Err(e)),
                };
                match target {
                    Ok(frame) => {
                        let motion = self.seek(id, frame, fast);
                        self.shown(id, Some(motion))
                    }
                    Err(e) => Step::Now(Err(e.into())),
                }
            }
            Request::PlayerStep { frames, viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                let id = PlayerId::of(self.viewer);
                let p = &mut self.players[id.index()];
                p.pause();
                let target = p.position + frames;
                let motion = self.seek(id, target, false);
                self.shown(id, Some(motion))
            }
            Request::PlayerState {} => Step::Now(Ok(self.state_response())),
            Request::PlayerMute { mute } => {
                let mute = mute.unwrap_or(!self.players[TIMELINE].muted());
                for p in &mut self.players {
                    p.set_muted(mute);
                }
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
                    let player = PlayerId::of(self.viewer);
                    if self.settled(player) {
                        let id = self.capture_viewer(player);
                        Step::Wait(Until::Capture {
                            id,
                            out,
                            player: Some(player),
                        })
                    } else {
                        Step::Wait(Until::Settled { player, out })
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
            // Decoding work runs on a worker against the saved project.
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
                    None => Step::Now(self.edit(envelope)),
                }
            }
            _ => Step::Now(self.edit(envelope)),
        }
    }

    /// Replies once `motion` of `player` is on screen.
    fn shown(&mut self, player: PlayerId, motion: Option<u64>) -> Step {
        match motion {
            Some(m) => Step::Wait(Until::Shown {
                player,
                motion: m,
                at: self.players[player.index()].position,
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
        if let Err(e) = envelope.check_rev(open.project().rev) {
            return Step::Now(Err(e));
        }
        let project = open.project().clone();
        let path = open.store.path().to_path_buf();
        let context = open
            .project()
            .time_context(Some(self.players[TIMELINE].position));
        let envelope = bind_playhead(envelope, &context);
        Step::Task(Box::new(move |progress| {
            let cancel_flag = progress
                .as_ref()
                .map(|(_, c)| c.clone())
                .unwrap_or_default();
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut report = |i: usize, n: usize, out: &Path, p: vidcut_av::export::Progress| {
                if let Some((tx, _)) = &progress
                    && (p.frames == p.total || last.elapsed() >= Duration::from_millis(200))
                {
                    last = Instant::now();
                    let _ = tx.send(ReplyEvent::Progress(json!({
                        "output": i, "outputs": n, "path": out,
                        "frames": p.frames, "total": p.total,
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
        }))
    }

    /// Project requests: edits and queries through the editor.
    fn edit(&mut self, mut envelope: Envelope) -> Result<Response> {
        let playhead = self.players[TIMELINE].position;
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
        let Some(open) = self.project.as_mut() else {
            return Err(no_project());
        };
        let request = envelope.request.clone();
        let (result, changed) = open.apply(envelope, &AvEnv, Some(playhead));
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
            if !r.dry_run && is_edit_function(&request) {
                self.playhead_after_edit(r);
            }
        }
        result
    }

    // ---------------------------------------------------------------- project

    fn open_project(&mut self, path: &Path, create: bool) -> Result<()> {
        let open = OpenProject::open(path, create, self.tx.clone())?;
        self.project = Some(open);
        self.selection = Selection::default();
        self.source_media = None;
        self.source_content = None;
        self.source_positions.clear();
        self.tape_position = 0;
        self.players[SOURCE].unload();
        self.players[TIMELINE].unload();
        self.remember_recent(path);
        self.register();
        self.project_changed(None);
        Ok(())
    }

    fn remember_recent(&self, path: &Path) {
        let file = paths::state_dir().join("recent.json");
        let canonical = path.canonicalize().unwrap_or(path.to_path_buf());
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
            return;
        };
        let p = open.project();
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

    /// Starts thumbnails and preview remuxes for new media.
    fn check_media(&mut self) {
        let Some(open) = &self.project else { return };
        let media: Vec<_> = open.project().media.clone();
        for m in media {
            if m.info.video.is_some() && self.thumbs_wanted.insert(m.id.clone()) {
                let (tx, id, media) = (self.tx.clone(), m.id.clone(), m.clone());
                self.workers.run(move || {
                    let _ = tx.send(Msg::Thumbs {
                        media: id,
                        result: filmstrip(&media),
                    });
                });
            }
            if !m.info.audio.is_empty() && self.waveforms_wanted.insert(m.id.clone()) {
                let (tx, id, path) = (self.tx.clone(), m.id.clone(), m.path.clone());
                let cache = paths::media_cache("waveforms", &m, "peaks");
                self.workers.run(move || {
                    let cached = std::fs::read(&cache)
                        .ok()
                        .and_then(|b| vidcut_av::waveform::Waveform::from_bytes(&b));
                    let result = match cached {
                        Some(w) => Ok(w),
                        None => vidcut_av::waveform::waveform(&path, 0, WAVEFORM_RATE)
                            .map_err(|e| e.to_string())
                            .inspect(|w| {
                                if let Some(dir) = cache.parent()
                                    && std::fs::create_dir_all(dir).is_ok()
                                {
                                    let _ = std::fs::write(&cache, w.to_bytes());
                                }
                            }),
                    };
                    let _ = tx.send(Msg::Waveform { media: id, result });
                });
            }
            if !self.network.contains_key(&m.path) {
                self.network.insert(m.path.clone(), false);
                let (tx, path) = (self.tx.clone(), m.path.clone());
                self.workers.run(move || {
                    let network = player::on_network(&path);
                    let _ = tx.send(Msg::Storage { path, network });
                });
            }
            if vidcut_av::remux::needs_index(&m.info.format) && !self.remux.contains_key(&m.path) {
                self.remux.insert(m.path.clone(), RemuxState::Pending);
                let tx = self.tx.clone();
                let out = paths::media_cache("preview", &m, "mkv");
                let source = m.path.clone();
                self.workers.run(move || {
                    let result = if out.exists() {
                        Ok(out)
                    } else {
                        std::fs::create_dir_all(out.parent().expect("in the cache"))
                            .map_err(|e| e.to_string())
                            .and_then(|()| {
                                let tmp = out.with_extension("part.mkv");
                                vidcut_av::remux::to_mkv(&source, &tmp)
                                    .map_err(|e| e.to_string())?;
                                std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
                                Ok(out)
                            })
                    };
                    let _ = tx.send(Msg::Remuxed { source, result });
                });
            }
        }
    }

    // ---------------------------------------------------------------- previews

    /// Filler media for the project's format. Generated here, synchronously,
    /// the first time a format needs them (then cached on disk): the preview
    /// is never in a half-loaded state that seeks would have to wait for.
    fn fillers(&mut self, p: &Project) -> Option<Fillers> {
        let key = (p.settings.width, p.settings.height, p.settings.fps);
        match self.fillers.get(&key) {
            Some(Some(f)) => return Some(f.clone()),
            Some(None) => return None,
            None => {}
        }
        let dir = paths::cache_dir().join("fillers");
        let result = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                vidcut_av::filler::ensure(&dir, key.0, key.1, key.2).map_err(|e| e.to_string())
            });
        match result {
            Ok(f) => {
                self.fillers.insert(key, Some(f.clone()));
                Some(f)
            }
            Err(e) => {
                self.fillers.insert(key, None);
                self.notice(format!("preview fillers: {e}"), true);
                None
            }
        }
    }

    /// The plan `player` plays: media mpv can't seek exactly swapped for
    /// their remuxes, and segment tails it would cut short replaced by
    /// patches. Starts the checks still missing, those nearest `position`
    /// first.
    fn preview_plan(
        &mut self,
        mut plan: RenderPlan,
        player: PlayerId,
        position: i64,
    ) -> RenderPlan {
        let swap = |remux: &HashMap<PathBuf, RemuxState>, path: &mut PathBuf| {
            if let Some(RemuxState::Ready(p)) = remux.get(path.as_path()) {
                *path = p.clone();
            }
        };
        for lane in &mut plan.audio {
            for a in lane {
                if let render::AudioPiece::Source { path, .. } = a {
                    swap(&self.remux, path);
                }
            }
        }
        let mut video = Vec::with_capacity(plan.video.len());
        let mut spots = Vec::new();
        let mut checks = Vec::new();
        let mut at = 0;
        for mut piece in std::mem::take(&mut plan.video) {
            at += piece.frames();
            let render::VideoPiece::Source {
                media,
                path,
                origin,
                source_frame,
                frames,
            } = &mut piece
            else {
                video.push(piece);
                continue;
            };
            swap(&self.remux, path);
            let key: TailKey = (
                path.clone(),
                *source_frame + *frames,
                plan.fps,
                plan.width,
                plan.height,
            );
            spots.push((at, key.clone()));
            let retry = matches!(self.tails.get(&key), Some(TailState::Failed))
                && self.tail_failures.get(&key).copied().unwrap_or(0) < TAIL_TRIES;
            if retry {
                self.tails.remove(&key);
            }
            match self.tails.get(&key) {
                Some(TailState::Ready {
                    path: patch,
                    frames: n,
                }) => {
                    let n = *n;
                    let keep = (*frames - n).max(0);
                    let media = media.clone();
                    let total = *frames;
                    if keep > 0 {
                        *frames = keep;
                        video.push(piece);
                    }
                    // The patch's frames are the last `n` of the cut, from 0.
                    video.push(render::VideoPiece::Source {
                        media,
                        path: patch.clone(),
                        origin: 0.0,
                        source_frame: (n - total).max(0),
                        frames: total.min(n),
                    });
                }
                Some(_) => video.push(piece),
                None => {
                    self.tails.insert(key.clone(), TailState::Checking);
                    checks.push((at, key, *origin));
                    video.push(piece);
                }
            }
        }
        checks.sort_by_key(|(end, ..)| (end - position).abs());
        for (_, key, origin) in checks {
            let (tx, fps) = (self.tx.clone(), plan.fps);
            let end = origin + fps.frames_to_secs(key.1) - fps.frames_to_secs(1) / 2.0;
            self.workers.run(move || {
                let lost = vidcut_av::tails::lost_before(&key.0, end).map_err(|e| e.to_string());
                let _ = tx.send(Msg::TailChecked { key, origin, lost });
            });
        }
        self.tail_spots[player.index()] = spots;
        plan.video = video;
        plan
    }

    /// Renders the patch for a tail that loses frames from `lost` (s) on.
    fn render_tail(&mut self, key: TailKey, origin: f64, lost: f64) {
        let (path, end, fps, w, h) = key.clone();
        // From the project frame whose sampling window holds `lost` (one more
        // to be safe; patch frames are exact anyway).
        let first = ((lost - origin) * fps.as_f64() - 0.5).floor() as i64;
        let frames = (end - first).clamp(1, MAX_TAIL.min(end));
        self.tails.insert(key.clone(), TailState::Rendering);
        let dir = paths::cache_dir().join("tails");
        let (size, mtime) = std::fs::metadata(&path)
            .map(|m| {
                use std::os::unix::fs::MetadataExt;
                (m.len(), m.mtime())
            })
            .unwrap_or_default();
        let name = format!(
            "{:016x}-{end}-{frames}-{}x{h}-{}.mkv",
            paths::fnv1a(format!("{}|{size}|{mtime}", path.display()).as_bytes()),
            w,
            fps.to_string().replace('/', "_")
        );
        let tx = self.tx.clone();
        self.workers.run(move || {
            let out = dir.join(name);
            let result = if out.exists() {
                Ok((out, frames))
            } else {
                let piece = render::VideoPiece::Source {
                    media: String::new(),
                    path,
                    origin,
                    source_frame: end - frames,
                    frames,
                };
                std::fs::create_dir_all(&dir)
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        vidcut_av::tails::render_patch(&piece, fps, w, h, &out)
                            .map_err(|e| e.to_string())
                    })
                    .map(|()| (out, frames))
            };
            let _ = tx.send(Msg::TailRendered { key, result });
        });
    }

    /// EDL for `plan` as `player` plays it from `position`, or `None` while
    /// fillers are being made.
    fn edl(
        &mut self,
        plan: RenderPlan,
        player: PlayerId,
        position: i64,
    ) -> Option<preview::Preview> {
        let p = self.project.as_ref()?.project().clone();
        let fillers = self.fillers(&p)?;
        let plan = self.preview_plan(plan, player, position);
        let network = |path: &PathBuf| self.network.get(path).copied().unwrap_or(false);
        let read_ahead = plan.video.iter().any(|v| match v {
            render::VideoPiece::Source { path, .. } => network(path),
            render::VideoPiece::Black { .. } => false,
        }) || plan.audio.iter().flatten().any(|a| match a {
            render::AudioPiece::Source { path, .. } => network(path),
            render::AudioPiece::Silence { .. } => false,
        });
        let deinterlace = plan.video.iter().any(|v| match v {
            render::VideoPiece::Source { media, .. } => p
                .media(media)
                .and_then(|m| m.info.video.as_ref())
                .is_some_and(|v| v.fields.is_some()),
            render::VideoPiece::Black { .. } => false,
        });
        Some(preview::Preview {
            read_ahead,
            deinterlace,
            ..preview::build(&plan, &fillers)
        })
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
        let p = self.project.as_ref()?.project().clone();
        let end = p.end();
        if end == 0 {
            self.players[TIMELINE].unload();
            return None;
        }
        let plan = match render::flatten(&p, 0, end) {
            Ok(plan) => plan,
            Err(e) => {
                self.notice(format!("timeline preview: {}", e.message), true);
                return None;
            }
        };
        let position = self.players[TIMELINE].position;
        let edl = self.edl(plan, PlayerId::Timeline, position)?;
        self.timeline_loaded_at = Some(Instant::now());
        self.players[TIMELINE].load(edl, end, p.settings.fps, position)
    }

    /// Makes the source player show what the current source mode wants
    /// (nothing happens if that is already loaded).
    fn refresh_source(&mut self) -> Option<u64> {
        let p = self.project.as_ref()?.project().clone();
        // Source Tape: the pool view's bin, in its order (p. 696). A removed
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
                    self.players[SOURCE].unload();
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
            self.players[SOURCE].unload();
            return None;
        };
        let position = if self.source_content.as_ref() == Some(&want) {
            // Same content (perhaps changed media): stay where we are.
            self.players[SOURCE].position
        } else {
            self.stash_source_position();
            match &want {
                SourceContent::Tape => self.tape_position,
                SourceContent::Clip(m) => self.source_positions.get(m).copied().unwrap_or(0),
            }
        };
        let frames = plan.frames;
        let edl = self.edl(plan, PlayerId::Source, position)?;
        self.source_content = Some(want);
        self.players[SOURCE].load(edl, frames, p.settings.fps, position)
    }

    /// Remembers the source playhead of the content being replaced.
    fn stash_source_position(&mut self) {
        let pos = self.players[SOURCE].position;
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
        let under = tape_at(&self.tape, self.players[SOURCE].position).map(|e| e.media.clone());
        if under != self.tape_media {
            self.tape_media = under.clone();
            if let Some(m) = under {
                self.selection.media = vec![m];
                self.dirty = true;
            }
        }
    }

    // ---------------------------------------------------------------- viewer

    fn set_viewer(&mut self, mode: ViewerMode) -> Option<u64> {
        if mode == self.viewer && (!mode.is_source() || mode == self.last_source) {
            return None;
        }
        let leaving = PlayerId::of(self.viewer);
        let entering = PlayerId::of(mode);
        if leaving != entering {
            self.players[leaving.index()].pause();
        }
        self.viewer = mode;
        if mode.is_source() {
            self.last_source = mode;
            return self.refresh_source();
        }
        None
    }

    fn pause_other(&mut self, id: PlayerId) {
        for p in &mut self.players {
            if p.id != id {
                p.pause();
            }
        }
    }

    /// Opens `media` as Source Clip (double-click in the pool, p. 680).
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
        if self.viewer.is_source() || self.viewer == ViewerMode::Timeline {
            self.players[TIMELINE].pause();
        }
        self.source_media = Some(media.to_string());
        self.viewer = ViewerMode::SourceClip;
        self.last_source = ViewerMode::SourceClip;
        let motion = self.refresh_source();
        Ok(match (motion, position) {
            (Some(m), _) => Some(m),
            // Same clip already loaded: just move to the requested frame.
            (None, Some(pos)) => Some(self.seek(PlayerId::Source, pos, false)),
            (None, None) => None,
        })
    }

    fn seek(&mut self, id: PlayerId, frame: i64, fast: bool) -> u64 {
        if id == PlayerId::Timeline && self.timeline_due.take().is_some() {
            // A reload is pending: do it now, loading at the new position.
            self.players[TIMELINE].position = frame.max(0);
            if let Some(motion) = self.refresh_timeline() {
                return motion;
            }
        }
        self.players[id.index()].seek(frame, fast)
    }

    /// The context for times typed at the viewer: timeline times, or source
    /// times of the shown clip or tape.
    fn viewer_context(&self) -> Result<TimeContext> {
        let open = self.project.as_ref().ok_or_else(no_project)?;
        let p = open.project();
        Ok(match self.viewer {
            ViewerMode::Timeline => p.time_context(Some(self.players[TIMELINE].position)),
            ViewerMode::SourceClip => {
                let position = self.players[SOURCE].position;
                match self.source_media.as_ref().and_then(|m| p.media(m)) {
                    Some(m) => media_context(p, m, Some(position)),
                    None => return Err(Error::usage("no source clip is open")),
                }
            }
            ViewerMode::SourceTape => {
                let position = self.players[SOURCE].position;
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
        let position = self.players[SOURCE].position;
        match (&self.source_content, self.viewer) {
            (Some(SourceContent::Clip(m)), ViewerMode::SourceClip) if m == media => Some(position),
            (Some(SourceContent::Tape), ViewerMode::SourceTape) => tape_at(&self.tape, position)
                .filter(|e| e.media == media)
                .map(|e| position - e.start),
            _ => None,
        }
    }

    /// Decision D15: after an edit function the timeline playhead moves to
    /// the frame after the new clip(s), ready for the next edit.
    fn playhead_after_edit(&mut self, r: &Response) {
        let Some(open) = &self.project else { return };
        let p = open.project();
        let end = r
            .changes
            .created
            .iter()
            .filter_map(|id| p.clip(id))
            .map(Clip::end)
            .max();
        if let Some(end) = end {
            self.seek(PlayerId::Timeline, end, false);
        }
    }

    fn set_selection(
        &mut self,
        clips: Option<OneOrMany<String>>,
        media: Option<OneOrMany<String>>,
    ) -> Result<()> {
        let open = self.project.as_ref().ok_or_else(no_project)?;
        let p = open.project();
        let playhead = Some(self.players[TIMELINE].position);
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
        let blank = player.is_some_and(|p| !self.players[p.index()].is_loaded());
        let result = result.map(|mut image| {
            if blank {
                image.rgba.as_chunks_mut::<4>().0.fill([0, 0, 0, 255]);
            }
            image
        });
        let state = self.state_response();
        self.spawn_task(
            w.job,
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

    /// A tail's check or patch failed: tried again in a moment up to
    /// [`TAIL_TRIES`] times, then left unpatched with a notice.
    fn tail_failed(&mut self, key: TailKey, what: &str) {
        self.tails.insert(key.clone(), TailState::Failed);
        let failures = self.tail_failures.entry(key).or_default();
        *failures += 1;
        if *failures < TAIL_TRIES {
            let at = Instant::now() + TAIL_RETRY;
            self.timeline_due = Some(self.timeline_due.map_or(at, |d| d.min(at)));
        } else {
            self.notice(
                format!("{what}; the preview may drop the last frames before that cut"),
                true,
            );
        }
    }

    /// Whether `player` shows its latest motion (seek or load), with no tail
    /// check or patch pending that could still change that frame.
    fn settled(&self, player: PlayerId) -> bool {
        let p = &self.players[player.index()];
        p.shown >= p.motion && !self.tail_near(player, p.position)
    }

    /// Asks the renderer for a picture of `player`; the answer comes as
    /// `Msg::Captured` with the returned id.
    fn capture_viewer(&mut self, player: PlayerId) -> u64 {
        let id = self.next_capture;
        self.next_capture += 1;
        let tx = self.tx.clone();
        self.renderer.capture_viewer(
            player,
            Box::new(move |result| {
                let _ = tx.send(Msg::Captured { id, result });
            }),
        );
        id
    }

    /// Whether a tail check or patch still running could change `player`'s
    /// frame at `position`: a patch replaces at most [`MAX_TAIL`] frames
    /// before the end of its segment.
    fn tail_near(&self, player: PlayerId, position: i64) -> bool {
        self.tail_spots[player.index()].iter().any(|(end, key)| {
            (end - MAX_TAIL..*end).contains(&position)
                && matches!(
                    self.tails.get(key),
                    Some(TailState::Checking | TailState::Rendering)
                )
        })
    }

    /// Tail checks and patches still running.
    fn tails_pending(&self) -> usize {
        self.tails
            .values()
            .filter(|t| matches!(t, TailState::Checking | TailState::Rendering))
            .count()
    }

    // ---------------------------------------------------------------- state

    fn state_response(&self) -> Response {
        let rev = self.project.as_ref().map_or(0, |p| p.project().rev);
        let mut r = answer(self.state_json());
        r.rev = rev;
        r
    }

    fn state_json(&self) -> Value {
        let p = self.project.as_ref().map(|o| o.project());
        let fps = p.map_or(Rational::new(30, 1).expect("valid"), |p| p.settings.fps);
        let tc = |f: i64, stc: i64| format_timecode(f, fps, stc);
        let player = |pl: &Player, stc: i64| {
            json!({
                "loaded": pl.is_loaded(),
                "frames": pl.frames,
                "position": pl.position,
                "position_tc": tc(pl.position, stc),
                "playing": pl.playing(),
                "speed": pl.snap().speed,
                "error": pl.error,
            })
        };
        let mut source = player(&self.players[SOURCE], 0);
        let position = self.players[SOURCE].position;
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
            "timeline": player(&self.players[TIMELINE], p.map_or(0, |p| p.settings.start_tc)),
            "selection": self.selection,
            "pool": self.pool,
            "muted": self.players[TIMELINE].muted(),
            "clipboard": self.clipboard.len(),
            "preview": self.preview_error.as_deref().map_or(json!(true), |e| json!(e)),
            "preview_pending": self.tails_pending(),
        })
    }
}

/// A background task; gets the client's progress channel and cancel flag.
type Task =
    Box<dyn FnOnce(Option<(Sender<ReplyEvent>, Arc<AtomicBool>)>) -> Result<Response> + Send>;

enum Step {
    Now(Result<Response>),
    Wait(Until),
    Task(Task),
    /// Measure media on a worker, then run the request here (docs/decisions/agents.md §3).
    Measure(Measure, Box<Envelope>),
}

/// Measuring that a request needs, done before it runs.
type Measure = Box<dyn FnOnce() -> Result<()> + Send>;

fn reply(reply: Option<Reply>, result: Result<Response>) {
    if let Some(r) = reply {
        let _ = r.tx.send(ReplyEvent::Done(result));
    }
}

fn answer(result: Value) -> Response {
    Response {
        rev: 0,
        changes: Default::default(),
        result,
        dry_run: false,
        via: String::new(),
    }
}

/// `jobs.wait`'s answer for an ended job: the job, or how it failed.
fn job_answer(j: &JobInfo) -> Result<Response> {
    match &j.error {
        Some(e) => Err(Error::new(e.code, format!("job {}: {}", j.id, e.message))),
        None => Ok(answer(j.to_json())),
    }
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

/// Removes the per-process preview directories (`$XDG_RUNTIME_DIR/vidcut/<pid>`)
/// of instances that are gone without cleaning up.
fn remove_stale_runtime_dirs() {
    let Ok(entries) = std::fs::read_dir(ipc::runtime_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let Some(pid) = e
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        // SAFETY: signal 0 only checks whether the process exists.
        let gone = unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}
