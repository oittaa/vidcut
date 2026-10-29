mod jobs;
mod missing;
mod player;
mod project;
pub mod snapshot;
pub mod stats;
mod workers;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use notify::Watcher;
use serde_json::{Value, json};
use vidcut_core::disk_cache::{self, Kind};
use vidcut_core::duck::Unmeasured;
use vidcut_core::editor::{Env, media_context, resolve_clip, resolve_cue};
use vidcut_core::model::{Clip, Format, Title};
use vidcut_core::ops::ClipSpans;
use vidcut_core::render::{self, RenderPlan, TapeEntry};
use vidcut_core::request::{
    Client, ClientKind, Detect, DynamicTrimMode, GuiPanel, OneOrMany, ScreenshotTarget,
    TimecodeDisplay, TimelineOptions, TrimSide, ViewerFrame, ViewerMode,
};
use vidcut_core::time::{Keyword, Rational, TimeContext, TimeSpec, format_timecode};
use vidcut_core::{Envelope, Error, Project, Request, Response, Result};

pub use jobs::{JobInfo, JobOutput, JobState};
pub use player::MakeOutput;
pub use snapshot::*;
pub use vidcut_av::thumbs::Image;

use crate::env::AvEnv;
use crate::ipc::{self, Instance};
use crate::paths;
use crate::threads::Work;
use player::{Player, skipped_to};
use project::OpenProject;
use workers::Workers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Gui { drag: bool },
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

pub type Capture = Box<dyn FnOnce(Result<Image>) + Send>;

pub type Presented = Box<dyn FnOnce(Option<i64>) + Send>;

#[derive(Debug, Clone, PartialEq)]
pub enum TrimEditor {
    Open {
        at: Option<i64>,
        track: String,
        by: Option<i64>,
        side: TrimSide,
        transition: bool,
    },
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GuiChange {
    SelectEdit {
        near: i64,
        track: String,
        side: TrimSide,
        add: bool,
    },
    EditSide(TrimSide),
    Deselect,
    Target(String),
    Timeline(TimelineOptions),
    Show(GuiPanel),
    Close(GuiPanel),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EditPoint {
    pub at: i64,
    pub track: String,
    pub side: TrimSide,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct TimelineState {
    pub zoom: f64,
    pub fixed_playhead: bool,
    pub snap: bool,
    pub trim_to_audio: bool,
    pub safe_edit: bool,
    pub names: bool,
    pub durations: bool,
    pub status: bool,
    pub filmstrips: bool,
    pub waveforms: bool,
    pub boring: bool,
    pub boring_secs: f64,
    pub jump_frames: i64,
    pub minimize_subtitles: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct GuiState {
    pub edit: Option<EditPoint>,
    pub edits: Vec<EditPoint>,
    pub target: String,
    pub timeline: TimelineState,
    pub windows: Vec<GuiPanel>,
}

pub type GuiDone = Box<dyn FnOnce(std::result::Result<GuiState, String>) + Send>;

pub const MAX_SPEED: f64 = 64.0;

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicTrim {
    pub on: Option<bool>,
    pub side: Option<TrimSide>,
    pub mode: Option<DynamicTrimMode>,
    pub run: Option<f64>,
    pub step: Option<i64>,
}

pub trait Renderer: Send + Sync {
    fn attach(&self, _engine: Option<Arc<vidcut_av::player::Player>>) {}
    fn present(&self, done: Presented);
    fn capture_viewer(&self, done: Capture);
    fn capture_window(&self, done: Capture);
    fn window(&self, change: WindowChange);
    fn quit(&self);
    fn gui(&self, _changes: Vec<GuiChange>, _done: GuiDone) {}
    fn gui_state(&self) -> Option<GuiState> {
        None
    }
    fn trim_editor(&self, _: TrimEditor) {}
    fn dynamic_trim(&self, _: DynamicTrim) {}
    fn two_up(&self) -> Option<stats::TwoUpStats> {
        None
    }
    fn viewer_stats(&self) -> Option<stats::ViewerStats> {
        None
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowChange {
    pub size: Option<[f32; 2]>,
    pub position: Option<[f32; 2]>,
    pub minimized: Option<bool>,
    pub maximized: Option<bool>,
    pub fullscreen: Option<bool>,
}

pub struct Config {
    pub project: Option<PathBuf>,
    pub create: bool,
    pub player: Option<PlayerConfig>,
    pub preview_error: Option<String>,
    pub renderer: Arc<dyn Renderer>,
    pub notify: Arc<dyn Fn() + Send + Sync>,
    pub socket: Option<PathBuf>,
}

pub struct PlayerConfig {
    pub audio: MakeOutput,
    pub budget: vidcut_av::player::Budget,
    pub gpu_pictures: vidcut_av::hw::GpuPictures,
}

pub(crate) enum ReplyEvent {
    Progress(Value),
    Done(Result<Response>),
}

pub(crate) struct Reply {
    tx: Sender<ReplyEvent>,
    cancel: Arc<AtomicBool>,
    notice: bool,
}

#[derive(Debug)]
pub struct Answer(Receiver<ReplyEvent>);

impl Answer {
    pub fn take(&self) -> Option<Result<Response>> {
        match self.0.try_recv() {
            Ok(ReplyEvent::Done(r)) => Some(r),
            Ok(ReplyEvent::Progress(_)) | Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(Error::no_instance("the vidcut GUI is shutting down")))
            }
        }
    }
}

pub(crate) struct Job {
    method: String,
    params: Value,
    origin: Origin,
    reply: Option<Reply>,
    rev: Option<u64>,
    dry_run: bool,
    said: Option<String>,
}

pub(crate) enum Msg {
    Missing {
        media: HashSet<String>,
        proxies: HashMap<String, PathBuf>,
    },
    Request {
        envelope: Envelope,
        origin: Origin,
        project: Option<PathBuf>,
        reply: Option<Reply>,
    },
    Picture,
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
    Made {
        job: Job,
        result: Result<(PathBuf, Vec<String>)>,
    },
    Measured {
        job: Job,
        envelope: Envelope,
        result: Result<()>,
        encode: Option<String>,
    },
    DuckHeard {
        measured: Vec<Unmeasured>,
        failed: Option<String>,
    },
    FileChanged,
    PreferencesChanged,
    OmarchyChanged,
    HistoryPreview(Option<PreviewAt>),
    Previewed {
        at: PreviewAt,
        rev: u64,
        result: Result<Arc<Project>>,
    },
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
    JobPlanned {
        id: String,
        outputs: Vec<(PathBuf, i64)>,
    },
    JobProgress {
        id: String,
        output: usize,
        progress: vidcut_av::export::Progress,
    },
    JobNote {
        id: String,
        note: vidcut_av::export::Note,
    },
    JobDone {
        id: String,
        result: Result<Response>,
    },
    Audition(Option<Audition>),
    Skip(Option<Vec<ClipSpans>>),
    PreviewTitle(Option<(String, Title)>),
    PreviewPictures(Vec<(String, vidcut_core::model::Picture)>),
    Subtitles(Option<vidcut_av::player::SubtitleView>),
    FreeTitle(Option<FreeTitle>),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreeTitle {
    title: Title,
    at: i64,
    fraction: f64,
}

pub struct Audition {
    pub timeline: Box<Project>,
    pub at: i64,
    pub speed: f64,
    pub skips: Option<Vec<(i64, i64)>>,
}

#[derive(Clone)]
pub struct Handle {
    tx: Sender<Msg>,
    shared: Arc<Mutex<Arc<Snapshot>>>,
}

impl Handle {
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn send(&self, envelope: Envelope) {
        self.send_from(envelope, false, None);
    }

    fn send_from(&self, envelope: Envelope, drag: bool, reply: Option<Reply>) {
        let _ = self.tx.send(Msg::Request {
            envelope,
            origin: Origin::Gui { drag },
            project: None,
            reply,
        });
    }

    pub fn request(&self, method: &str, params: Value) {
        self.request_from(method, params, false, None);
    }

    pub fn request_answered(&self, method: &str, params: Value) -> Answer {
        let (tx, rx) = channel();
        let reply = Reply {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
            notice: true,
        };
        self.request_from(method, params, false, Some(reply));
        Answer(rx)
    }

    pub fn audition(&self, audition: Option<Audition>) {
        let _ = self.tx.send(Msg::Audition(audition));
    }

    pub fn skip(&self, parts: Option<Vec<ClipSpans>>) {
        let _ = self.tx.send(Msg::Skip(parts));
    }

    pub fn preview_title(&self, title: Option<(String, Title)>) {
        let _ = self.tx.send(Msg::PreviewTitle(title));
    }

    pub fn preview_pictures(&self, pictures: Vec<(String, vidcut_core::model::Picture)>) {
        let _ = self.tx.send(Msg::PreviewPictures(pictures));
    }

    pub fn show_subtitles(&self, view: Option<vidcut_av::player::SubtitleView>) {
        let _ = self.tx.send(Msg::Subtitles(view));
    }

    pub fn preview_free_title(&self, title: Option<(Title, i64, f64)>) {
        let _ = self
            .tx
            .send(Msg::FreeTitle(title.map(|(title, at, fraction)| {
                FreeTitle {
                    title,
                    at,
                    fraction,
                }
            })));
    }

    pub fn request_drag(&self, method: &str, params: Value) {
        self.request_from(method, params, true, None);
    }

    fn request_from(&self, method: &str, params: Value, drag: bool, reply: Option<Reply>) {
        match Envelope::from_json(json!({ "method": method, "params": params })) {
            Ok(e) => self.send_from(e, drag, reply),
            Err(e) => {
                let _ = self.tx.send(Msg::TaskDone {
                    job: Job {
                        method: method.into(),
                        params,
                        origin: Origin::Gui { drag },
                        reply,
                        rev: None,
                        dry_run: false,
                        said: None,
                    },
                    result: Err(Error::internal(e)),
                });
            }
        }
    }

    pub fn call(
        &self,
        envelope: Envelope,
        origin: Origin,
        project: Option<PathBuf>,
        progress: &mut dyn FnMut(Option<Value>) -> bool,
    ) -> Result<Response> {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
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
                    notice: false,
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

    pub fn history_preview(&self, at: Option<PreviewAt>) {
        let _ = self.tx.send(Msg::HistoryPreview(at));
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
    }

    pub fn shutdown_and_wait(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.tx.send(Msg::Shutdown).is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn start(config: Config) -> (Handle, std::thread::JoinHandle<()>) {
    let (tx, rx) = channel();
    let pid = std::process::id();
    let (audio, budget, gpu_pictures) = match config.player {
        Some(p) => (Some(p.audio), p.budget, p.gpu_pictures),
        None => (None, vidcut_av::player::Budget::both(0), Vec::new()),
    };
    vidcut_av::player::set_budget(budget);
    let player = {
        let (tx, notify) = (tx.clone(), config.notify.clone());
        Player::new(
            audio,
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
    let (theme, theme_error) = chosen_theme();
    let theme = Arc::new(theme);
    let empty = Snapshot {
        seq: 0,
        pid,
        socket: config.socket.clone(),
        project: None,
        viewer: ViewerMode::Timeline,
        viewer_frame: ViewerFrame::Page,
        last_source: ViewerMode::SourceClip,
        source: PlayerSnap::default(),
        timeline: PlayerSnap::default(),
        source_media: None,
        tape: Arc::new(Vec::new()),
        selection: Selection::default(),
        pool: PoolView::default(),
        clipboard: 0,
        key_clipboard: 0,
        notices: Arc::new(VecDeque::new()),
        thumbs: Arc::new(HashMap::new()),
        missing: Arc::new(HashSet::new()),
        transcribed: Arc::new(HashSet::new()),
        fillers: Arc::default(),
        lost_proxies: Arc::new(HashMap::new()),
        proxy_dir: Arc::from(Path::new("")),
        waveforms: Arc::new(HashMap::new()),
        jobs: Arc::new(Vec::new()),
        audio_scrubbing: true,
        looping: false,
        fast_review: false,
        video_only: false,
        audio_only: false,
        edit_only_rev: 0,
        stats: false,
        timecode_display: TimecodeDisplay::Auto,
        timer1_origin: Timer1Origin::default(),
        guides: crate::preferences::load()
            .map(|p| p.viewer.guides)
            .unwrap_or_default(),
        show_super_scale: crate::preferences::load().is_ok_and(|p| p.viewer.super_scale),
        preview_error: config.preview_error.clone(),
        theme: theme.clone(),
        history_preview: None,
        flash: None,
    };
    let shared = Arc::new(Mutex::new(Arc::new(empty)));
    let preferences = crate::preferences::path();
    if let Some(dir) = preferences.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let themes = crate::themes::dir();
    let _ = std::fs::create_dir_all(&themes);
    let look_changed = tx.clone();
    crate::look::watch(std::sync::Arc::new(move || {
        let _ = look_changed.send(Msg::PreferencesChanged);
    }));
    let mut live = Live {
        _preferences: project::watch(&preferences, tx.clone(), || Msg::PreferencesChanged),
        _themes: project::watch_dir(&themes, tx.clone(), || Msg::PreferencesChanged),
        omarchy: None,
        omarchy_due: None,
        theme,
        theme_error: theme_error.clone(),
        tx: tx.clone(),
        rx,
        shared: shared.clone(),
        notify: config.notify,
        renderer: config.renderer,
        instance,
        project: None,
        project_snap: None,
        viewer: ViewerMode::Timeline,
        viewer_frame: ViewerFrame::Page,
        last_source: ViewerMode::SourceClip,
        source_media: None,
        source_content: None,
        source_positions: HashMap::new(),
        tape_position: 0,
        tape: Arc::new(Vec::new()),
        tape_media: None,
        player,
        skipping: None,
        free_title: None,
        free_pictures: Vec::new(),
        selection: Selection::default(),
        pool: PoolView::default(),
        clipboard: Vec::new(),
        key_clipboard: Vec::new(),
        log: VecDeque::new(),
        log_seq: 0,
        notices: Arc::new(VecDeque::new()),
        notice_seq: 0,
        thumbs: Arc::new(HashMap::new()),
        missing: Arc::new(HashSet::new()),
        transcribed: Arc::new(HashSet::new()),
        fillers: Arc::new(crate::preferences::filler_lists()),
        lost_proxies: Arc::new(HashMap::new()),
        proxy_dir: Arc::from(Path::new("")),
        missing_watch: missing::MissingWatch::start(tx.clone()),
        thumbs_wanted: HashMap::new(),
        waveforms: Arc::new(HashMap::new()),
        waveforms_wanted: HashMap::new(),
        workers: Workers::new(2),
        jobs: Arc::new(Vec::new()),
        job_cancels: HashMap::new(),
        turns: jobs::Turns::default(),
        gates: HashMap::new(),
        job_replies: HashMap::new(),
        next_job: 1,
        hearing: 0,
        audio_scrubbing: true,
        looping: false,
        fast_review: false,
        review_speed: None,
        video_only: false,
        audio_only: false,
        edit_only_rev: 0,
        stats: false,
        timecode_display: TimecodeDisplay::Auto,
        timer1_origin: Timer1Origin::default(),
        guides: crate::preferences::load()
            .map(|p| p.viewer.guides)
            .unwrap_or_default(),
        show_super_scale: crate::preferences::load().is_ok_and(|p| p.viewer.super_scale),
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
        history_preview: None,
        preview_wanted: None,
        previews: None,
        past: Default::default(),
        flash: None,
        flash_seq: 0,
        duck_measuring: HashSet::new(),
        duck_failed: HashSet::new(),
    };
    live.watch_omarchy();
    let project = config.project;
    let create = config.create;
    let thread = crate::threads::spawn("vidcut-session", Work::Interactive, move || {
        if let Some(e) = theme_error {
            live.notice(e, true);
        }
        if let Some(path) = project
            && let Err(e) = to_make(&path, create, false, &Format::default())
                .and_then(|new| live.open_project(&path, new))
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

struct Preview {
    store: vidcut_core::Store,
    project: Project,
    at: PreviewAt,
    rev: u64,
}

fn start_previews(
    inbox: Sender<Msg>,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> Option<Sender<Preview>> {
    let (tx, rx) = channel::<Preview>();
    crate::threads::spawn("vidcut-preview", Work::Interactive, move || {
        let mut past = vidcut_core::past::Past::default();
        while let Ok(mut job) = rx.recv() {
            std::thread::sleep(Duration::from_millis(30));
            while let Ok(newer) = rx.try_recv() {
                job = newer;
            }
            let result = match job.at {
                PreviewAt::Step(step) => job.store.project_at(&mut past, job.project, step),
                PreviewAt::Version(id, k) => job.store.version_at(&mut past, job.project, id, k),
            };
            let done = Msg::Previewed {
                at: job.at,
                rev: job.rev,
                result,
            };
            if inbox.send(done).is_err() {
                return;
            }
            notify();
        }
    })
    .ok()?;
    Some(tx)
}

#[derive(Debug, Clone, PartialEq)]
enum SourceContent {
    Clip(String),
    Tape,
}

enum Until {
    Shown {
        motion: u64,
        at: i64,
    },
    Job {
        id: String,
    },
    Settled {
        out: PathBuf,
    },
    Capture {
        id: u64,
        out: PathBuf,
        player: Option<PlayerId>,
    },
}

struct Wait {
    job: Job,
    until: Until,
    deadline: Option<Instant>,
}

const WAIT_LIMIT: Duration = Duration::from_secs(10);

const CLIENT_POLL: Duration = Duration::from_millis(50);

const JOBS_KEPT: usize = 50;

const LOG_CAP: usize = 500;
const NOTICE_CAP: usize = 20;

const WAVEFORM_RATE: u32 = 100;

const THUMBS: usize = 24;
const THUMB_SIZE: (u32, u32) = (192, 108);

fn media_version(m: &vidcut_core::model::Media) -> String {
    format!(
        "{}|{}|{}|{}",
        m.path.display(),
        m.size,
        m.mtime_ns,
        m.sequence.as_ref().map_or(0, |s| s.count)
    )
}

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
            let decoder = crate::preferences::gpu_decoding().own_decoder();
            let decoder = decoder.as_ref();
            let strip = match media.sequence.as_ref().map(|s| s.count) {
                Some(pictures) => vidcut_av::thumbs::filmstrip_pictures(
                    &input,
                    pictures,
                    THUMBS,
                    THUMB_SIZE.0,
                    THUMB_SIZE.1,
                    decoder,
                    media.color_space,
                ),
                None => vidcut_av::thumbs::filmstrip(
                    &input,
                    THUMBS,
                    THUMB_SIZE.0,
                    THUMB_SIZE.1,
                    decoder,
                    media.color_space,
                ),
            }
            .map_err(|e| e.to_string())?;
            if prefs.disk_cache && !strip.0.is_empty() {
                crate::thumbcache::store(media, THUMBS, THUMB_SIZE, (&strip.0, &strip.1));
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

fn chosen_name() -> String {
    crate::preferences::load()
        .map(|p| p.appearance.theme)
        .unwrap_or_else(|_| crate::themes::DEFAULT.into())
}

fn chosen_theme() -> (crate::themes::Theme, Option<String>) {
    let name = chosen_name();
    match crate::themes::load(&name) {
        Ok(t) => (t, None),
        Err(e) => (
            crate::themes::load(crate::themes::DEFAULT)
                .unwrap_or_else(|_| crate::themes::default_theme().clone()),
            Some(format!(
                "The theme {name} doesn't load, so the default one shows. {}",
                e.message
            )),
        ),
    }
}

const RELOAD_EVERY: Duration = Duration::from_millis(100);
const OMARCHY_SETTLES: Duration = Duration::from_millis(150);

const HEARS: [&str; 5] = ["subtitles", "words", "transcript", "word_times", "retime"];
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
const HEARING_VIDEO: usize = 1 << 30;

struct Live {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    _preferences: Option<notify::RecommendedWatcher>,
    _themes: Option<notify::RecommendedWatcher>,
    omarchy: Option<(notify::RecommendedWatcher, Option<PathBuf>)>,
    omarchy_due: Option<Instant>,
    theme: Arc<crate::themes::Theme>,
    theme_error: Option<String>,
    shared: Arc<Mutex<Arc<Snapshot>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    renderer: Arc<dyn Renderer>,
    instance: Option<Instance>,
    project: Option<OpenProject>,
    project_snap: Option<ProjectSnap>,
    viewer: ViewerMode,
    viewer_frame: ViewerFrame,
    last_source: ViewerMode,
    source_media: Option<String>,
    source_content: Option<SourceContent>,
    source_positions: HashMap<String, i64>,
    tape_position: i64,
    tape: Arc<Vec<TapeEntry>>,
    tape_media: Option<String>,
    player: Player,
    skipping: Option<Vec<ClipSpans>>,
    free_title: Option<FreeTitle>,
    free_pictures: Vec<(String, vidcut_core::model::Picture)>,
    selection: Selection,
    pool: PoolView,
    clipboard: Vec<vidcut_core::model::TrackClip>,
    key_clipboard: Vec<vidcut_core::request::CopiedKey>,
    log: VecDeque<LogEntry>,
    log_seq: u64,
    notices: Arc<VecDeque<Notice>>,
    notice_seq: u64,
    thumbs: Arc<HashMap<String, Arc<Filmstrip>>>,
    missing: Arc<HashSet<String>>,
    transcribed: Arc<HashSet<String>>,
    fillers: Arc<std::collections::BTreeMap<String, Vec<String>>>,
    lost_proxies: Arc<HashMap<String, PathBuf>>,
    proxy_dir: Arc<Path>,
    missing_watch: missing::MissingWatch,
    thumbs_wanted: HashMap<String, String>,
    waveforms: Arc<HashMap<String, Arc<vidcut_av::waveform::Waveform>>>,
    waveforms_wanted: HashMap<String, String>,
    workers: Workers,
    jobs: Arc<Vec<JobInfo>>,
    job_cancels: HashMap<String, Arc<AtomicBool>>,
    turns: jobs::Turns,
    gates: HashMap<String, Sender<()>>,
    job_replies: HashMap<String, (Job, bool)>,
    next_job: u64,
    hearing: usize,
    audio_scrubbing: bool,
    looping: bool,
    fast_review: bool,
    review_speed: Option<f64>,
    video_only: bool,
    audio_only: bool,
    edit_only_rev: u64,
    stats: bool,
    timecode_display: TimecodeDisplay,
    timer1_origin: Timer1Origin,
    guides: vidcut_core::preferences::Guides,
    show_super_scale: bool,
    audio_noticed: Option<String>,
    playing: bool,
    waits: Vec<Wait>,
    next_capture: u64,
    timeline_due: Option<Instant>,
    timeline_loaded_at: Option<Instant>,
    seq: u64,
    dirty: bool,
    answers: Vec<(Reply, Result<Response>)>,
    preview_error: Option<String>,
    history_preview: Option<Arc<HistoryPreview>>,
    preview_wanted: Option<(PreviewAt, u64)>,
    previews: Option<Sender<Preview>>,
    past: vidcut_core::past::Past,
    flash: Option<Arc<Flash>>,
    flash_seq: u64,
    duck_measuring: HashSet<Unmeasured>,
    duck_failed: HashSet<Unmeasured>,
}

const SOURCE: PlayerId = PlayerId::Source;
const TIMELINE: PlayerId = PlayerId::Timeline;

impl Live {
    fn run(&mut self) {
        loop {
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
        for c in self.job_cancels.values() {
            c.store(true, Ordering::Relaxed);
        }
        self.gates.clear();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.job_cancels.is_empty() {
            match self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Msg::JobDone { id, result }) => self.job_done(id, result),
                Ok(Msg::Measured {
                    job,
                    encode: Some(id),
                    ..
                }) => {
                    self.job_cancels.remove(&id);
                    reply(job.reply, Err(Error::no_instance("the vidcut GUI closed")));
                }
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
        vidcut_core::temp::remove_left();
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

    fn handle(&mut self, msg: Msg) -> bool {
        let mut changed = true;
        match msg {
            Msg::Request {
                envelope,
                origin,
                project,
                reply,
            } => self.request(envelope, origin, project, reply),
            Msg::Audition(a) => changed = self.audition(a),
            Msg::Skip(parts) => changed = self.set_skipping(parts),
            Msg::PreviewTitle(title) => {
                changed = false;
                if self.timeline_due.take().is_some() {
                    self.refresh_timeline();
                }
                self.player.preview_title(title);
            }
            Msg::PreviewPictures(pictures) => {
                changed = false;
                if self.free_pictures != pictures {
                    self.player.preview_pictures(pictures.clone());
                    self.free_pictures = pictures;
                    self.schedule_timeline();
                    (self.notify)();
                }
            }
            Msg::Subtitles(view) => {
                changed = false;
                self.player.set_subtitles(view);
            }
            Msg::FreeTitle(title) => {
                changed = false;
                if self.free_title != title {
                    self.free_title = title;
                    self.schedule_timeline();
                }
            }
            Msg::Picture => {
                self.player.picture_landed();
                self.present_due();
                changed = self.ready_changed();
            }
            Msg::Presented { motion, frame } => {
                self.player.presented(motion, frame);
                changed = false;
            }
            Msg::Captured { id, result } => self.captured(id, result),
            Msg::TaskDone { job, result } => self.finish(job, result),
            Msg::Made { job, result } => {
                let result = result.and_then(|(path, warnings)| {
                    self.open_project(&path, None)?;
                    let mut r = self.state_response();
                    r.result["warnings"] = json!(warnings);
                    Ok(r)
                });
                self.finish(job, result);
            }
            Msg::Measured {
                job,
                envelope,
                result,
                encode,
            } => {
                let (job, result) = match result {
                    Err(e) => (job, Err(e)),
                    Ok(()) => {
                        let rev = self.project.as_ref().map(|p| p.project().rev);
                        let job = Job { rev, ..job };
                        let result = self.edit(envelope, job.origin);
                        (job, result)
                    }
                };
                let ended = encode.is_some();
                if let Some(id) = encode {
                    self.job_ended(&id, &result);
                }
                self.finish(job, result);
                self.refresh_transcribed(ended);
            }
            Msg::DuckHeard { measured, failed } => {
                for m in &measured {
                    self.duck_measuring.remove(m);
                }
                if let Some(e) = failed {
                    self.duck_failed.extend(measured);
                    self.notice(format!("Ducking isn't heard: {e}"), true);
                }
                self.refresh_timeline();
            }
            Msg::PreferencesChanged => {
                disk_cache::set_policy(crate::preferences::cache_policy());
                changed = self
                    .player
                    .follow_decoding(crate::preferences::gpu_decoding());
                changed |= self.player.follow_audio(crate::preferences::audio_device());
                let fillers = crate::preferences::filler_lists();
                if *self.fillers != fillers {
                    self.fillers = Arc::new(fillers);
                    changed = true;
                }
                let guides = crate::preferences::load()
                    .map(|p| p.viewer.guides)
                    .unwrap_or_default();
                if self.guides != guides {
                    self.guides = guides;
                    changed = true;
                }
                let show_super_scale =
                    crate::preferences::load().is_ok_and(|p| p.viewer.super_scale);
                if self.show_super_scale != show_super_scale {
                    self.show_super_scale = show_super_scale;
                    self.schedule_timeline();
                    self.refresh_source();
                    changed = true;
                }
                self.follow_budget();
                self.follow_theme();
                if self.project.is_some() && self.proxy_folder() != *self.proxy_dir {
                    self.project_changed(None);
                }
            }
            Msg::OmarchyChanged => {
                changed = false;
                self.omarchy_due = Some(Instant::now() + OMARCHY_SETTLES);
            }
            Msg::HistoryPreview(at) => changed = self.preview_step(at),
            Msg::Previewed { at, rev, result } => changed = self.previewed(at, rev, result),
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
            Msg::Missing { media, proxies } => {
                let lost: Vec<String> = proxies
                    .iter()
                    .filter(|(id, path)| self.lost_proxies.get(*id) != Some(*path))
                    .map(|(id, _)| id.clone())
                    .collect();
                let changed = *self.lost_proxies != proxies;
                self.missing = Arc::new(media);
                self.lost_proxies = Arc::new(proxies);
                let playing = self
                    .project
                    .as_ref()
                    .map(|o| o.project().settings.use_proxies);
                if changed && playing == Some(true) {
                    self.schedule_timeline();
                    self.refresh_source();
                }
                if !lost.is_empty() {
                    self.make_proxies(Some(&lost));
                }
            }
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
                progress,
            } => {
                if let Some(j) = Arc::make_mut(&mut self.jobs)
                    .iter_mut()
                    .find(|j| j.id == id)
                {
                    j.progressed(output, progress);
                }
            }
            Msg::JobNote { id, note } => {
                if let Some(j) = Arc::make_mut(&mut self.jobs)
                    .iter_mut()
                    .find(|j| j.id == id)
                {
                    j.details.note(note);
                    self.dirty = true;
                }
            }
            Msg::JobDone { id, result } => self.job_done(id, result),
            Msg::Shutdown => return false,
        }
        self.dirty |= changed;
        true
    }

    fn present_due(&mut self) {
        let Some(motion) = self.player.present_due(Instant::now()) else {
            return;
        };
        vidcut_av::debug!(
            "session",
            "{:?}: present motion {motion}",
            PlayerId::of(self.viewer)
        );
        let tx = self.tx.clone();
        self.renderer.present(Box::new(move |frame| {
            let _ = tx.send(Msg::Presented { motion, frame });
        }));
    }

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
        if let Some(due) = self.omarchy_due {
            if now >= due {
                self.omarchy_due = None;
                self.follow_theme();
            } else {
                soonest(due);
            }
        }
        self.present_due();
        let p = &self.player;
        if p.shown < p.motion {
            soonest(now + Duration::from_millis(5));
        }
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
        let audio = self.player.audio_notice();
        if audio != self.audio_noticed {
            if let Some(n) = &audio {
                self.notice(n.clone(), true);
            }
            self.audio_noticed = audio;
        }
        self.sync_tape_selection();
        self.sync_loop();
        self.follow_review();
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
                    let p = &self.player;
                    let overtaken = p.target != at;
                    if overtaken && matches!(w.job.origin, Origin::Gui { .. }) {
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
                    .filter(|j| !j.unfinished())
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
            viewer_frame: self.viewer_frame,
            last_source: self.last_source,
            source: self.player.snap(SOURCE),
            timeline: self.player.snap(TIMELINE),
            source_media: self.source_media.clone(),
            tape: self.tape.clone(),
            selection: self.selection.clone(),
            pool: self.pool.clone(),
            clipboard: self.clipboard.len(),
            key_clipboard: self.key_clipboard.len(),
            notices: self.notices.clone(),
            thumbs: self.thumbs.clone(),
            missing: self.missing.clone(),
            transcribed: self.transcribed.clone(),
            fillers: self.fillers.clone(),
            lost_proxies: self.lost_proxies.clone(),
            proxy_dir: self.proxy_dir.clone(),
            waveforms: self.waveforms.clone(),
            jobs: self.jobs.clone(),
            audio_scrubbing: self.audio_scrubbing,
            looping: self.looping,
            fast_review: self.fast_review,
            video_only: self.video_only,
            audio_only: self.audio_only,
            edit_only_rev: self.edit_only_rev,
            stats: self.stats,
            timecode_display: self.timecode_display,
            timer1_origin: self.timer1_origin,
            guides: self.guides,
            show_super_scale: self.show_super_scale,
            preview_error: self.preview_error.clone(),
            theme: self.theme.clone(),
            history_preview: self.history_preview.clone(),
            flash: self.flash.clone(),
        };
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(snap);
        (self.notify)();
    }

    fn follow_theme(&mut self) {
        let (theme, error) = chosen_theme();
        if *self.theme != theme {
            self.theme = Arc::new(theme);
            self.dirty = true;
        }
        if error != self.theme_error {
            self.theme_error = error.clone();
            if let Some(e) = error {
                self.notice(e, true);
            }
        }
        self.watch_omarchy();
    }

    fn watch_omarchy(&mut self) {
        let wanted = crate::themes::colors_from(&chosen_name());
        if wanted != Some(crate::themes::ColorsFrom::Omarchy) {
            self.omarchy = None;
            return;
        }
        if self.omarchy.is_none() {
            self.omarchy = project::watcher(crate::omarchy::relevant, self.tx.clone(), || {
                Msg::OmarchyChanged
            })
            .map(|w| (w, None));
        }
        let Some((watcher, armed)) = &mut self.omarchy else {
            return;
        };
        if let Some(dir) = armed.take() {
            let _ = watcher.unwatch(&dir);
        }
        let (dir, mode) = crate::omarchy::watched();
        if watcher.watch(&dir, mode).is_ok() {
            *armed = Some(dir);
        }
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

    fn request(
        &mut self,
        envelope: Envelope,
        origin: Origin,
        expected: Option<PathBuf>,
        reply: Option<Reply>,
    ) {
        let mut envelope = envelope.normalized();
        if envelope.client.is_none() {
            let kind = match origin {
                Origin::Ipc => ClientKind::Ipc,
                Origin::Gui { .. } | Origin::Session => ClientKind::Gui,
            };
            envelope.client = Some(Client::new(kind, None));
        }
        let (method, params) = match serde_json::to_value(&envelope.request) {
            Ok(Value::Object(mut m)) => (
                m.remove("method")
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
                m.remove("params").unwrap_or(Value::Null),
            ),
            _ => (envelope.request.method(), Value::Null),
        };
        let before = self.project.as_ref().map(|o| o.project());
        let said = envelope
            .request
            .is_mutation()
            .then(|| vidcut_core::describe::describe(&method, &params, before));
        let job = Job {
            method,
            params,
            origin,
            reply,
            rev: before.map(|p| p.rev),
            dry_run: envelope.dry_run,
            said,
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
            Step::Wait(until) => self.waits.push(Wait {
                job: Job { rev: None, ..job },
                until,
                deadline: limit.map(|l| Instant::now() + l),
            }),
            Step::Task(work, task) => self.spawn_task(Job { rev: None, ..job }, work, task),
            Step::Make(make) => {
                let tx = self.tx.clone();
                let job = Job { rev: None, ..job };
                let _ = crate::threads::spawn("vidcut-make", Work::Interactive, move || {
                    let _ = tx.send(Msg::Made {
                        job,
                        result: make(),
                    });
                });
            }
            Step::Measure(measure, then) => {
                let tx = self.tx.clone();
                let cancel = job
                    .reply
                    .as_ref()
                    .map(|r| r.cancel.clone())
                    .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
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
            Step::Encode(encode, then, title, kind) => {
                self.start_encode(job, encode, *then, title, kind)
            }
        }
    }

    fn models_missing(&self, request: &Request) -> vidcut_core::editor::Models {
        let Some(open) = self.project.as_ref() else {
            return Default::default();
        };
        let playhead = Some(self.player.position(TIMELINE));
        let after =
            vidcut_core::editor::models_after(open.project(), request, &AvEnv::new(), playhead);
        let (retimes, upscalers) = after.unwrap_or_default();
        let dir = paths::models_dir();
        (
            retimes
                .into_iter()
                .filter(|r| !vidcut_av::interpolate::needed(&dir, *r).is_empty())
                .collect(),
            upscalers
                .into_iter()
                .filter(|u| !vidcut_av::super_scale::needed(&dir, *u).is_empty())
                .collect(),
        )
    }

    fn ai_missing(
        &self,
        media: &[String],
        on: Option<bool>,
        upscaler: Option<vidcut_core::model::MediaUpscaler>,
    ) -> Vec<vidcut_core::model::Upscaler> {
        let Some(open) = self.project.as_ref() else {
            return Vec::new();
        };
        let set = vidcut_core::ops::SuperScaleSet {
            on,
            sharpness: None,
            upscaler,
        };
        vidcut_core::ops::ai_upscalers(open.project(), media, &set)
            .into_iter()
            .filter(|u| !vidcut_av::super_scale::needed(&paths::models_dir(), *u).is_empty())
            .collect()
    }

    fn duck_measure(&self, envelope: &Envelope) -> Option<Measure> {
        use vidcut_core::model::DuckDetect;
        let p = self.project.as_ref()?.project();
        let Request::TrackSet {
            track,
            duck: Some(Some(set)),
            download,
            voice_isolation,
            ..
        } = &envelope.request
        else {
            return None;
        };
        let d = vidcut_core::duck::patched(p, track, set).ok()?;
        let isolate = voice_isolation.is_some_and(|v| v > 0.0)
            && *download
            && !vidcut_av::isolate::needed(&paths::models_dir()).is_empty();
        let detect = d.detect();
        let needed: Vec<(vidcut_core::model::Media, u32)> = match d.on {
            true => vidcut_core::duck::sounds(p, track, &d)
                .into_iter()
                .filter(|(m, _)| !self.missing.contains(&m.id))
                .filter(|(m, s)| crate::env::kept_measure(m, *s, detect).is_none())
                .map(|(m, s)| (m.clone(), s))
                .collect(),
            false => Vec::new(),
        };
        if needed.is_empty() && !isolate {
            return None;
        }
        let download = *download;
        Some(Box::new(move || {
            if isolate {
                vidcut_av::isolate::get_ready(&paths::models_dir(), true).map_err(|e| {
                    crate::export::av_error(&e, vidcut_core::ErrorCode::Media, String::new())
                })?;
            }
            let env = AvEnv::new();
            for (m, stream) in &needed {
                match detect {
                    DuckDetect::Speech => env.voice_activity(m, *stream, download).map(drop)?,
                    DuckDetect::Level => env.audio_levels(m, *stream).map(drop)?,
                }
            }
            Ok(())
        }))
    }

    fn loudness_measure(&self, envelope: &Envelope) -> Option<Measure> {
        let p = self.project.as_ref()?.project();
        let Request::ClipNormalize { clips, .. } = &envelope.request else {
            return None;
        };
        let playhead = Some(self.player.position(TIMELINE));
        let plans: Vec<_> = clips
            .to_vec()
            .iter()
            .filter_map(|r| resolve_clip(p, r, playhead).ok())
            .filter_map(|id| vidcut_core::render::sound_alone(p, &id).ok())
            .collect();
        if plans.is_empty() {
            return None;
        }
        Some(Box::new(move || {
            let env = AvEnv::new();
            for plan in &plans {
                env.loudness(plan)?;
            }
            Ok(())
        }))
    }

    fn measuring(&self, envelope: &Envelope) -> Option<Measure> {
        let p = self.project.as_ref()?.project();
        let playhead = Some(self.player.position(TIMELINE));
        let (clips, sound, speech, download) = match &envelope.request {
            Request::EditRippleDeleteSilence { parts: Some(_), .. } => return None,
            Request::EditRippleDeleteSilence {
                clips,
                detect,
                download,
                ..
            } => (clips.to_vec(), true, *detect == Detect::Speech, *download),
            Request::EditDetectSceneCuts { clips, .. } => (clips.to_vec(), false, false, false),
            Request::EditRippleDeleteWords {
                words, quiet: true, ..
            } => {
                let context = p.time_context(playhead);
                let spans: Vec<(i64, i64)> = words
                    .iter()
                    .filter_map(|w| {
                        Some((
                            context.resolve(&w.start).ok()?,
                            context.resolve(&w.end).ok()?,
                        ))
                    })
                    .collect();
                let near = p.settings.fps.as_f64().round() as i64;
                let solo = p.any_solo();
                let clips = p
                    .tracks
                    .iter()
                    .filter(|t| t.audible(solo))
                    .flat_map(|t| &t.clips)
                    .filter(|c| {
                        spans
                            .iter()
                            .any(|&(s, e)| c.start <= e + near && c.end() >= s - near)
                    })
                    .map(|c| c.id.clone())
                    .collect();
                (clips, true, false, false)
            }
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
                    Some(s) if speech => AvEnv::new().voice_activity(m, *s, download).map(drop)?,
                    Some(s) => AvEnv::new().audio_levels(m, *s).map(drop)?,
                    None => AvEnv::new().picture_stats(m).map(drop)?,
                }
            }
            Ok(())
        }))
    }

    fn start_export(&mut self, envelope: Envelope, job: Job) {
        let Some(open) = &self.project else {
            return self.finish(job, Err(no_project()));
        };
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
        let title = info.title.clone();
        self.add_job(info, cancel.clone());
        let gate = self.take_turn(&id, true, &title, progress_tx.as_ref());
        if job.reply.is_some() {
            self.job_replies.insert(id.clone(), (job, background));
        } else {
            self.finish(job, Ok(answer(json!({ "job": id }))));
        }
        let tx = self.tx.clone();
        let _ = crate::threads::spawn(format!("vidcut-{id}"), Work::Job, move || {
            let _noting = job_notes(&tx, &id);
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut report = |i: usize, n: usize, out: &Path, p: vidcut_av::export::Progress| {
                if p.frames != p.total && last.elapsed() < Duration::from_millis(100) {
                    return;
                }
                last = Instant::now();
                let _ = tx.send(Msg::JobProgress {
                    id: id.clone(),
                    output: i,
                    progress: p,
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
                    let _ = gate.recv();
                    if cancel.load(Ordering::Relaxed) {
                        return Err(cancelled());
                    }
                    Ok(())
                },
            );
            let _ = tx.send(Msg::JobDone { id, result });
        });
    }

    fn add_job(&mut self, info: JobInfo, cancel: Arc<AtomicBool>) {
        self.job_cancels.insert(info.id.clone(), cancel);
        let jobs = Arc::make_mut(&mut self.jobs);
        jobs.push(info);
        let finished = jobs.iter().filter(|j| !j.unfinished()).count();
        if finished > JOBS_KEPT {
            let mut drop = finished - JOBS_KEPT;
            jobs.retain(|j| {
                let gone = drop > 0 && !j.unfinished();
                drop -= usize::from(gone);
                !gone
            });
        }
        self.dirty = true;
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn follow_budget(&mut self) {
        vidcut_av::player::set_budget(crate::preferences::player_budget());
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    fn follow_budget(&mut self) {
        let mut budget = crate::preferences::player_budget();
        if self.hearing > 0 {
            budget.video = Some(budget.video.map_or(HEARING_VIDEO, |v| v.min(HEARING_VIDEO)));
        }
        vidcut_av::player::set_budget(budget);
    }

    fn start_encode(
        &mut self,
        job: Job,
        encode: Measure,
        then: Envelope,
        title: String,
        kind: &'static str,
    ) {
        if HEARS.contains(&kind) {
            self.hearing += 1;
            self.follow_budget();
        }
        let id = format!("j{}", self.next_job);
        self.next_job += 1;
        let cancel = job
            .reply
            .as_ref()
            .map(|r| r.cancel.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let progress = job.reply.as_ref().map(|r| r.tx.clone());
        let info = JobInfo::task(
            id.clone(),
            kind,
            job.origin.name(),
            job.params.clone(),
            title.clone(),
        );
        self.add_job(info, cancel.clone());
        let gate = self.take_turn(&id, false, &title, progress.as_ref());
        let tx = self.tx.clone();
        let _ = crate::threads::spawn(format!("vidcut-{id}"), Work::Job, move || {
            let _ = gate.recv();
            let stopped = cancel.load(Ordering::Relaxed);
            let _guard = vidcut_av::cancel::Guard::set(cancel);
            let _noting = job_notes(&tx, &id);
            let _report = {
                let (tx, id) = (tx.clone(), id.clone());
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut stage = None;
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
                        if p.frames != p.total
                            && p.stage == stage
                            && last.elapsed() < Duration::from_millis(100)
                        {
                            return;
                        }
                        last = Instant::now();
                        stage = p.stage;
                        let _ = tx.send(Msg::JobProgress {
                            id: id.clone(),
                            output,
                            progress: p,
                        });
                        if let Some(ptx) = &progress {
                            let _ = ptx.send(ReplyEvent::Progress(json!({
                                "job": id, "output": output, "path": out,
                                "frames": p.frames, "total": p.total, "stage": p.stage,
                                "elapsed": p.elapsed.as_secs_f64(),
                            })));
                        }
                    },
                ))
            };
            let result = if stopped { Err(cancelled()) } else { encode() };
            let _ = tx.send(Msg::Measured {
                job,
                envelope: then,
                result,
                encode: Some(id),
            });
        });
    }

    fn take_turn(
        &mut self,
        id: &str,
        export: bool,
        title: &str,
        progress: Option<&Sender<ReplyEvent>>,
    ) -> Receiver<()> {
        let (go, gate) = channel();
        if self.turns.ask(id, export) {
            let _ = go.send(());
            return gate;
        }
        self.gates.insert(id.to_string(), go);
        if let Some(j) = Arc::make_mut(&mut self.jobs)
            .iter_mut()
            .find(|j| j.id == id)
        {
            j.state = JobState::Queued;
        }
        if let Some(p) = progress {
            let _ = p.send(ReplyEvent::Progress(
                json!({ "job": id, "title": title, "queued": true }),
            ));
        }
        gate
    }

    fn start_turn(&mut self, id: &str) {
        if let Some(go) = self.gates.remove(id) {
            let _ = go.send(());
        }
        if let Some(j) = Arc::make_mut(&mut self.jobs)
            .iter_mut()
            .find(|j| j.id == id)
        {
            j.begin();
        }
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

    fn job_ended(&mut self, id: &str, result: &Result<Response>) {
        self.job_cancels.remove(id);
        self.gates.remove(id);
        if let Some(next) = self.turns.ended(id) {
            self.start_turn(&next);
        }
        self.dirty = true;
        if self.jobs.iter().any(|j| {
            j.id == id
                && HEARS.contains(&j.kind)
                && matches!(j.state, JobState::Queued | JobState::Running)
        }) {
            self.hearing = self.hearing.saturating_sub(1);
            self.follow_budget();
        }
        let Some(j) = Arc::make_mut(&mut self.jobs)
            .iter_mut()
            .find(|j| j.id == id)
        else {
            return;
        };
        j.ended(result);
        if j.kind == "proxy" {
            self.missing_watch.look_again();
        }
        if j.origin != (Origin::Gui { drag: false }).name() && j.origin != Origin::Session.name() {
            return;
        }
        let kind = j.kind;
        match result {
            Ok(r) => {
                self.notice(format!("{kind} {id} finished"), false);
                let told = r.result["warnings"].as_array().into_iter().flatten();
                for w in told
                    .filter_map(Value::as_str)
                    .chain(r.warnings.iter().map(String::as_str))
                {
                    self.notice(format!("{kind} {id}: {w}"), false);
                }
            }
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

    fn finish(&mut self, job: Job, result: Result<Response>) {
        let result = result.map(|mut r| {
            r.via = format!("gui:{}", std::process::id());
            r
        });
        let rev = self.project.as_ref().map_or(0, |p| p.project().rev);
        let quiet = matches!(
            job.method.as_str(),
            "player.state" | "selection.get" | "gui.log" | "gui.state"
        ) || (matches!(job.origin, Origin::Gui { .. })
            && matches!(
                job.method.as_str(),
                "export.plan"
                    | "presets.list"
                    | "presets.get"
                    | "looks.list"
                    | "encoders.list"
                    | "jobs.list"
                    | "preferences.get"
            ))
            || (job.method == "player.seek" && job.params["fast"] == json!(true))
            || (job.method == "mixer.preview" && matches!(job.origin, Origin::Gui { .. }))
            || (job.method == "pool.set"
                && matches!(job.origin, Origin::Gui { .. })
                && job.params.get("search").is_some())
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
                params: match job.method.as_str() {
                    "services.set_key" => json!({ "service": job.params["service"] }),
                    _ => job.params.clone(),
                },
                summary: self.summary(&job, &result),
                ok: result.is_ok(),
                error: result.as_ref().err().map(|e| e.message.clone()),
                rev,
                changed: !job.dry_run
                    && (result.as_ref().is_ok_and(|r| !r.changes.is_empty())
                        || (result.is_ok()
                            && job.method != "project.open"
                            && job.rev.is_some_and(|before| before != rev))),
            };
            self.log.push_back(entry);
            while self.log.len() > LOG_CAP {
                self.log.pop_front();
            }
        }
        if let (Err(e), Origin::Gui { .. }) = (&result, job.origin)
            && job.reply.as_ref().is_none_or(|r| r.notice)
            && e.code != vidcut_core::ErrorCode::Conflict
        {
            self.notice(format!("{}: {}", job.method, e.message), true);
        }
        self.dirty = true;
        self.tool_notice(&job, &result);
        self.history_notice(&job, &result);
        self.auto_proxies(&job, &result);
        if let Some(r) = job.reply {
            self.answers.push((r, result));
        }
    }

    fn summary(&self, job: &Job, result: &Result<Response>) -> String {
        let said = job.said.clone().unwrap_or_else(|| {
            let project = self.project.as_ref().map(|o| o.project());
            vidcut_core::describe::describe(&job.method, &job.params, project)
        });
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

    fn preview_step(&mut self, at: Option<PreviewAt>) -> bool {
        let (Some(open), Some(at)) = (&self.project, at) else {
            self.preview_wanted = None;
            return self.history_preview.take().is_some();
        };
        let rev = open.project().rev;
        if self
            .history_preview
            .as_ref()
            .is_some_and(|p| p.at == at && p.rev == rev)
            || self.preview_wanted == Some((at, rev))
        {
            return false;
        }
        self.preview_wanted = Some((at, rev));
        if !open.editor.on_its_file() {
            let result = match at {
                PreviewAt::Step(step) => self.past.project_at(&open.editor, step),
                PreviewAt::Version(id, k) => self.past.version_at(&open.editor, id, k),
            };
            return self.previewed(at, rev, result);
        }
        let job = Preview {
            store: open.store.clone(),
            project: open.project().clone(),
            at,
            rev,
        };
        if self.previews.is_none() {
            self.previews = start_previews(self.tx.clone(), self.notify.clone());
        }
        if let Some(tx) = &self.previews
            && tx.send(job).is_err()
        {
            self.previews = None;
        }
        false
    }

    fn previewed(&mut self, at: PreviewAt, rev: u64, result: Result<Arc<Project>>) -> bool {
        let now = self.project.as_ref().map(|o| o.project().rev);
        if self.preview_wanted != Some((at, rev)) || now != Some(rev) {
            return false;
        }
        self.preview_wanted = None;
        self.history_preview = result
            .ok()
            .map(|project| Arc::new(HistoryPreview { at, rev, project }));
        self.dirty = true;
        true
    }

    fn history_notice(&mut self, job: &Job, result: &Result<Response>) {
        let Ok(r) = result else {
            return;
        };
        if !matches!(job.origin, Origin::Gui { .. }) || job.dry_run {
            return;
        }
        let said = |key: &str| r.result[key].as_str().unwrap_or_default().to_string();
        match job.method.as_str() {
            "history.revert" => {
                return self.notice(format!("Reverted: {}", said("label")), false);
            }
            "history.mark" => {
                return self.notice(
                    format!(
                        "Marked step {}: \u{201c}{}\u{201d}",
                        r.result["step"],
                        said("name")
                    ),
                    false,
                );
            }
            "history.unmark" => {
                return self.notice(
                    format!("Took the name off step {}", r.result["step"]),
                    false,
                );
            }
            "history.restore" => {
                let text = match r.result["restored"].as_array().map_or(0, Vec::len) {
                    1 => "Restored a version: 1 step".to_string(),
                    n => format!("Restored a version: {n} steps"),
                };
                return self.notice(text, false);
            }
            _ => {}
        }
        let (verb, labels) = match (r.result["undone"].as_array(), r.result["redone"].as_array()) {
            (Some(l), _) => ("Undid", l),
            (_, Some(l)) => ("Redid", l),
            _ => return,
        };
        let text = match labels.as_slice() {
            [] => return,
            [one] => format!("{verb}: {}", one.as_str().unwrap_or_default()),
            many => format!("{verb} {} steps", many.len()),
        };
        self.notice(text, false);
        self.flash_seq += 1;
        self.flash = Some(Arc::new(Flash {
            seq: self.flash_seq,
            rev: r.rev,
            clips: serde_json::from_value(r.result["clips"].clone()).unwrap_or_default(),
            spans: serde_json::from_value(r.result["spans"].clone()).unwrap_or_default(),
        }));
    }

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
            "media.add_folder" => folder_notice(&r.result),
            "subtitles.import"
                if r.result["placed"] == "start"
                    && self
                        .project
                        .as_ref()
                        .is_some_and(|p| p.project().settings.start_tc > 0) =>
            {
                format!(
                    "{} subtitle(s) on {}, from the timeline's start: the file isn't timed in                      the timeline's timecode",
                    r.result["cues"],
                    r.result["track"].as_str().unwrap_or_default()
                )
            }
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

    fn auto_proxies(&mut self, job: &Job, result: &Result<Response>) {
        let Ok(response) = result else {
            return;
        };
        let added = match job.method.as_str() {
            _ if job.dry_run => return,
            "media.add" | "media.add_folder" | "timeline.import" | "apply" => {
                Some(&response.changes.created)
            }
            "project.set" if job.params["use_proxies"] == json!(true) => None,
            _ => return,
        };
        self.make_proxies(added.map(Vec::as_slice));
    }

    fn proxy_folder(&self) -> PathBuf {
        self.project
            .as_ref()
            .map(|open| AvEnv::for_project(open.store.path()).proxy_dir())
            .unwrap_or_default()
    }

    fn make_proxies(&self, only: Option<&[String]>) {
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
            .filter(|m| only.is_none_or(|a| a.contains(&m.id)))
            .filter(|m| {
                let made = m.fresh_proxy().is_some() && !self.lost_proxies.contains_key(&m.id);
                rules.wants(m) && !made && !busy.contains(&m.id)
            })
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

    fn proxies_encoding(&self, p: &Project) -> HashSet<String> {
        let mut ids = HashSet::new();
        for j in self
            .jobs
            .iter()
            .filter(|j| j.kind == "proxy" && j.unfinished())
        {
            match &j.params["media"] {
                Value::String(id) => {
                    ids.insert(id.clone());
                }
                Value::Array(list) => {
                    ids.extend(list.iter().filter_map(|v| v.as_str().map(String::from)));
                }
                _ => ids.extend(p.media.iter().map(|m| m.id.clone())),
            }
        }
        ids
    }

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
        let loads = matches!(
            request,
            Request::PlayerSeek { .. } | Request::PlayerStep { .. } | Request::ProjectOpen { .. }
        );
        if request.is_live() && !loads && self.timeline_due.take().is_some() {
            self.refresh_timeline();
        }
        match request {
            Request::ViewerGuides {
                enabled,
                framing,
                extents,
                action,
                title,
                center,
            } => {
                let patch = vidcut_core::preferences::GuidesPatch {
                    enabled,
                    framing,
                    extents,
                    action,
                    title,
                    center,
                };
                match crate::preferences::set_guides(patch, envelope.dry_run) {
                    Ok(v) => {
                        if !envelope.dry_run
                            && let Ok(guides) = serde_json::from_value::<
                                vidcut_core::preferences::Guides,
                            >(v["guides"].clone())
                        {
                            self.guides = guides;
                        }
                        Step::Now(Ok(answer(v)))
                    }
                    Err(e) => Step::Now(Err(e)),
                }
            }
            r if r.is_projectless() => Step::Task(
                Work::Interactive,
                Box::new(move |_| crate::session::run(None, envelope)),
            ),
            Request::ProjectOpen {
                path,
                create,
                force,
                save_as,
                fps,
                width,
                height,
                drop_frame,
                start_tc,
            } => {
                let format = crate::session::size(width, height).map(|size| Format {
                    fps,
                    size,
                    drop_frame,
                    start_tc,
                    ..Format::default()
                });
                if vidcut_core::editor::is_timeline_file(&path) {
                    let target = save_as.unwrap_or_else(|| path.with_extension("vidcut"));
                    let format = match format {
                        Ok(format) => format,
                        Err(e) => return Step::Now(Err(e)),
                    };
                    if envelope.dry_run {
                        return Step::Task(
                            Work::Interactive,
                            Box::new(move |_| {
                                let (project, path, warnings) = crate::session::create_from_otio(
                                    &target, &path, &format, true,
                                )?;
                                Ok(answer(json!({
                                    "project": path,
                                    "settings": project.settings,
                                    "warnings": warnings,
                                })))
                            }),
                        );
                    }
                    return Step::Make(Box::new(move || {
                        crate::session::create_from_otio(&target, &path, &format, false)
                            .map(|(_, path, warnings)| (path, warnings))
                    }));
                }
                if save_as.is_some() {
                    return Step::Now(Err(Error::usage(
                        "save_as is for opening an OpenTimelineIO file",
                    )));
                }
                Step::Now(
                    format
                        .and_then(|format| to_make(&path, create, force, &format))
                        .and_then(|new| self.open_project(&path, new))
                        .map(|()| self.state_response()),
                )
            }
            Request::ViewerSet {
                mode,
                display,
                reset_timer1,
                paste,
            } => self.viewer_set(mode, display, reset_timer1, paste),
            Request::ViewerLoad { media, at } => match self.load_source(&media, at.as_ref()) {
                Ok(motion) => self.shown(motion),
                Err(e) => Step::Now(Err(e)),
            },
            Request::ViewerFrame { mode } => {
                self.viewer_frame = mode;
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerPlay { speed, viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                let omitted = speed.is_none();
                let speed = speed.unwrap_or(1.0);
                if !speed.is_finite() || speed == 0.0 || speed.abs() > MAX_SPEED {
                    return Step::Now(Err(Error::usage(format!(
                        "speed must be non-zero, within ±{MAX_SPEED}"
                    ))));
                }
                if omitted && self.fast_review {
                    self.play_reviewed();
                } else {
                    self.review_speed = None;
                    self.play(speed);
                }
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerPause { viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                self.review_speed = None;
                self.player.pause();
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerToggle { viewer } => {
                if let Some(v) = viewer {
                    self.set_viewer(v);
                }
                if self.player.playing() {
                    self.review_speed = None;
                    self.player.pause();
                } else if self.fast_review {
                    self.play_reviewed();
                } else {
                    self.review_speed = None;
                    self.play(1.0);
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
            Request::PlayerLoop { on } => {
                self.looping = on.unwrap_or(!self.looping);
                self.sync_loop();
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerFastReview { on } => {
                self.fast_review = on.unwrap_or(!self.fast_review);
                let id = PlayerId::of(self.viewer);
                let snap = self.player.snap(id);
                if self.fast_review {
                    if snap.playing && (snap.speed - 1.0).abs() < 0.001 {
                        self.play_reviewed();
                    }
                } else if self.review_speed.take().is_some() && snap.playing {
                    self.play(1.0);
                }
                Step::Now(Ok(self.state_response()))
            }
            Request::PlayerStats { show } => {
                self.stats = show.unwrap_or(!self.stats);
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
                view,
                search,
            } => {
                let tape = bin.is_some() || sort.is_some() || descending.is_some();
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
                if let Some(v) = view {
                    self.pool.view = v;
                }
                if let Some(s) = search {
                    self.pool.search = s;
                }
                if tape {
                    self.refresh_source();
                }
                Step::Now(Ok(answer(json!(self.pool))))
            }
            Request::SelectionGet {} => {
                let state = self.renderer.gui_state();
                Step::Now(Ok(answer(selection_json(&self.selection, state.as_ref()))))
            }
            Request::SelectionSet {
                clips,
                cues,
                media,
                edit,
                track,
                side,
                add,
                clear,
            } => {
                let mut changes = Vec::new();
                if clear {
                    changes.push(GuiChange::Deselect);
                }
                match (edit, track) {
                    (Some(at), track) => {
                        let open = match self.project.as_ref() {
                            Some(o) => o,
                            None => return Step::Now(Err(no_project())),
                        };
                        let p = open.project();
                        let track = track.unwrap_or_else(|| vidcut_core::ops::MAIN.to_string());
                        if p.track(&track).is_none() {
                            return Step::Now(Err(Error::not_found(format!(
                                "there is no {track}"
                            ))));
                        }
                        let near = match p
                            .time_context(Some(self.player.position(TIMELINE)))
                            .resolve(&at)
                        {
                            Ok(f) => f,
                            Err(e) => return Step::Now(Err(e.into())),
                        };
                        changes.push(GuiChange::SelectEdit {
                            near,
                            track,
                            side: side.unwrap_or_default(),
                            add,
                        });
                    }
                    (None, Some(_)) => {
                        return Step::Now(Err(Error::usage(
                            "track goes with edit: it names the track of the edit to select",
                        )));
                    }
                    (None, None) if add => {
                        return Step::Now(Err(Error::usage(
                            "add goes with edit: it names the edit to add or take out",
                        )));
                    }
                    (None, None) => changes.extend(side.map(GuiChange::EditSide)),
                }
                let clips = match clips {
                    None if clear => Some(OneOrMany::Many(Vec::new())),
                    c => c,
                };
                let cues = match cues {
                    None if clear => Some(OneOrMany::Many(Vec::new())),
                    c => c,
                };
                if let Err(e) = self.set_selection(clips, cues, media) {
                    return Step::Now(Err(e));
                }
                if changes.is_empty() {
                    let state = self.renderer.gui_state();
                    return Step::Now(Ok(answer(selection_json(&self.selection, state.as_ref()))));
                }
                let selection = self.selection.clone();
                self.gui_change(changes, move |s| selection_json(&selection, Some(&s)))
            }
            Request::GuiScreenshot { out, target } => match target {
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
                position,
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
                if let Some([x, y]) = position
                    && !(x.abs() <= 65536.0 && y.abs() <= 65536.0)
                {
                    return Step::Now(Err(Error::usage("position must be within -65536..65536")));
                }
                self.renderer.window(WindowChange {
                    size,
                    position,
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
                self.gui_change(vec![GuiChange::Show(panel)], |s| json!(s))
            }
            Request::GuiClose { panel } => {
                self.gui_change(vec![GuiChange::Close(panel)], |s| json!(s))
            }
            Request::GuiState {} => self.gui_change(Vec::new(), |s| json!(s)),
            Request::GuiOnly {
                video_only,
                audio_only,
            } => match edit_only(self.video_only, self.audio_only, video_only, audio_only) {
                Ok((video, audio)) => {
                    self.video_only = video;
                    self.audio_only = audio;
                    self.edit_only_rev = self.edit_only_rev.saturating_add(1);
                    Step::Now(Ok(answer(json!({
                        "video_only": video,
                        "audio_only": audio,
                    }))))
                }
                Err(message) => Step::Now(Err(Error::usage(message))),
            },
            Request::GuiTarget { track } => {
                let known = self
                    .project
                    .as_ref()
                    .is_some_and(|o| o.project().track(&track).is_some());
                if !known {
                    return Step::Now(Err(Error::not_found(format!("there is no {track}"))));
                }
                self.gui_change(vec![GuiChange::Target(track)], |s| json!(s))
            }
            Request::GuiTimeline(o) => self.gui_change(vec![GuiChange::Timeline(o)], |s| json!(s)),
            Request::GuiTrimEditor {
                at,
                track,
                by,
                side,
                transition,
                close,
            } => {
                if close {
                    self.renderer.trim_editor(TrimEditor::Close);
                    return Step::Now(Ok(answer(Value::Null)));
                }
                let open = match self.project.as_ref() {
                    Some(o) => o,
                    None => return Step::Now(Err(no_project())),
                };
                let p = open.project();
                let track = track.unwrap_or_else(|| vidcut_core::ops::MAIN.to_string());
                match p.track(&track) {
                    Some(t) if !t.clips.is_empty() => {}
                    Some(_) => {
                        return Step::Now(Err(Error::usage(format!("{track} has no clips"))));
                    }
                    None => return Step::Now(Err(Error::usage(format!("there is no {track}")))),
                }
                let at = match at {
                    Some(t) => match p
                        .time_context(Some(self.player.position(TIMELINE)))
                        .resolve(&t)
                    {
                        Ok(f) => Some(f),
                        Err(e) => return Step::Now(Err(e.into())),
                    },
                    None => None,
                };
                self.renderer.trim_editor(TrimEditor::Open {
                    at,
                    track,
                    by,
                    side,
                    transition,
                });
                Step::Now(Ok(answer(Value::Null)))
            }
            Request::GuiDynamicTrim {
                on,
                side,
                mode,
                run,
                step,
            } => {
                if let Some(r) = run
                    && !(-MAX_SPEED..=MAX_SPEED).contains(&r)
                {
                    return Step::Now(Err(Error::usage(format!(
                        "Dynamic Trim runs at up to {MAX_SPEED} times normal speed, either way"
                    ))));
                }
                self.renderer.dynamic_trim(DynamicTrim {
                    on,
                    side,
                    mode,
                    run,
                    step,
                });
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
                if self.gates.remove(&job).is_some() {
                    self.turns.ended(&job);
                }
                Step::Now(self.job_json(&job).map(answer))
            }
            Request::JobsWait { job, .. } => match self.job_json(&job) {
                Ok(_) => Step::Wait(Until::Job { id: job }),
                Err(e) => Step::Now(Err(e)),
            },
            Request::ExportStart(_)
            | Request::ExportPlan(_)
            | Request::FrameRender { .. }
            | Request::StoryboardRender { .. }
            | Request::AnalyzeSilence { .. }
            | Request::AnalyzeScenes { .. }
            | Request::AnalyzeBlack { .. }
            | Request::AnalyzeLoudness { .. }
            | Request::AnalyzeWords { media: Some(_), .. }
            | Request::AnalyzeWords { clips: Some(_), .. } => self.project_task(envelope, job),
            Request::TrackSet {
                duck: Some(Some(_)),
                ..
            } if !envelope.dry_run => {
                self.duck_failed.clear();
                match self.duck_measure(&envelope) {
                    Some(measure) => Step::Measure(measure, Box::new(envelope)),
                    None => Step::Now(self.edit(envelope, job.origin)),
                }
            }
            Request::MediaSet {
                media,
                super_scale,
                upscaler,
                download: true,
                ..
            } if !envelope.dry_run
                && !self
                    .ai_missing(&media.to_vec(), super_scale, upscaler)
                    .is_empty() =>
            {
                let wanted = self.ai_missing(&media.to_vec(), super_scale, upscaler);
                let fetch = move || {
                    for u in wanted {
                        vidcut_av::super_scale::get_ready(&paths::models_dir(), u, true).map_err(
                            |e| {
                                crate::export::av_error(
                                    &e,
                                    vidcut_core::ErrorCode::Media,
                                    String::new(),
                                )
                            },
                        )?;
                    }
                    Ok(())
                };
                Step::Measure(Box::new(fetch), Box::new(envelope))
            }
            _ if fetches(&envelope.request)
                && !envelope.dry_run
                && self.models_missing(&envelope.request) != Default::default() =>
            {
                let (retimes, upscalers) = self.models_missing(&envelope.request);
                let fetch = move || {
                    let dir = paths::models_dir();
                    let failed = |e: vidcut_av::Error| {
                        crate::export::av_error(&e, vidcut_core::ErrorCode::Media, String::new())
                    };
                    for r in retimes {
                        vidcut_av::interpolate::get_ready(&dir, r, true).map_err(failed)?;
                    }
                    for u in upscalers {
                        vidcut_av::super_scale::get_ready(&dir, u, true).map_err(failed)?;
                    }
                    Ok(())
                };
                Step::Measure(Box::new(fetch), Box::new(envelope))
            }
            Request::ClipSet {
                voice_isolation: Some(amount),
                download: true,
                ..
            }
            | Request::TrackSet {
                voice_isolation: Some(amount),
                download: true,
                ..
            } if amount > 0.0
                && !envelope.dry_run
                && !vidcut_av::isolate::needed(&paths::models_dir()).is_empty() =>
            {
                let fetch = || {
                    vidcut_av::isolate::get_ready(&paths::models_dir(), true).map_err(|e| {
                        crate::export::av_error(&e, vidcut_core::ErrorCode::Media, String::new())
                    })
                };
                Step::Measure(Box::new(fetch), Box::new(envelope))
            }
            Request::ClipNormalize { .. } => match self.loudness_measure(&envelope) {
                Some(measure) => Step::Measure(measure, Box::new(envelope)),
                None => Step::Now(self.edit(envelope, job.origin)),
            },
            Request::EditRippleDeleteSilence { .. }
            | Request::EditDetectSceneCuts { .. }
            | Request::EditRippleDeleteWords { quiet: true, .. } => {
                match self.measuring(&envelope) {
                    Some(measure) => Step::Measure(measure, Box::new(envelope)),
                    None => Step::Now(self.edit(envelope, job.origin)),
                }
            }
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
                Step::Encode(Box::new(encode), Box::new(envelope), title, "proxy")
            }
            Request::SubtitlesTranscribe { .. }
            | Request::AnalyzeWords { .. }
            | Request::MediaTranscribe { .. } => {
                let Some(open) = &self.project else {
                    return Step::Now(Err(no_project()));
                };
                let (project, path) = (open.project().clone(), open.store.path().to_path_buf());
                let context = project.time_context(Some(self.player.position(TIMELINE)));
                let envelope = bind_playhead(envelope, &context);
                let copy = envelope.clone();
                let hear = move || {
                    let env = AvEnv::for_project(&path);
                    vidcut_core::Editor::new(project, Default::default())
                        .apply(copy, &env, None)
                        .map(drop)
                };
                let (title, kind) = match &envelope.request {
                    Request::AnalyzeWords { .. } => ("Words in the speech", "words"),
                    Request::MediaTranscribe { .. } => ("Transcribe Audio", "transcript"),
                    _ => ("Create Subtitles from Audio", "subtitles"),
                };
                Step::Encode(Box::new(hear), Box::new(envelope), title.into(), kind)
            }
            Request::SubtitlesRetime { .. } | Request::SubtitlesTimeWords { .. } => {
                let Some(open) = &self.project else {
                    return Step::Now(Err(no_project()));
                };
                let (project, path) = (open.project().clone(), open.store.path().to_path_buf());
                let context = project.time_context(Some(self.player.position(TIMELINE)));
                let envelope = bind_playhead(envelope, &context);
                let copy = envelope.clone();
                let hear = move || {
                    let env = AvEnv::for_project(&path);
                    vidcut_core::Editor::new(project, Default::default())
                        .apply(copy, &env, None)
                        .map(drop)
                };
                let (title, kind) = match &envelope.request {
                    Request::SubtitlesTimeWords { .. } => {
                        ("Synchronize Word Timings", "word_times")
                    }
                    _ => ("Re-time Subtitles to Speech", "retime"),
                };
                Step::Encode(Box::new(hear), Box::new(envelope), title.into(), kind)
            }
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
            Request::MediaTranscriptsRemove { .. } => {
                let result = self.edit(envelope, job.origin);
                self.refresh_transcribed(true);
                Step::Now(result)
            }
            _ => Step::Now(self.edit(envelope, job.origin)),
        }
    }

    fn shown(&mut self, motion: Option<u64>) -> Step {
        match motion {
            Some(m) => Step::Wait(Until::Shown {
                motion: m,
                at: self.player.target,
            }),
            None => Step::Now(Ok(self.state_response())),
        }
    }

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

    fn edit(&mut self, mut envelope: Envelope, origin: Origin) -> Result<Response> {
        let playhead = self.player.position(TIMELINE);
        match &mut envelope.request {
            Request::EditPaste { clips, .. } if clips.is_none() => {
                if self.clipboard.is_empty() {
                    return Err(Error::usage("the clipboard is empty"));
                }
                *clips = Some(self.clipboard.clone());
            }
            Request::KeyframesPaste { keyframes, .. } if keyframes.is_none() => {
                if self.key_clipboard.is_empty() {
                    return Err(Error::usage("the clipboard has no keyframes"));
                }
                *keyframes = Some(self.key_clipboard.clone());
            }
            Request::MediaMarksSet {
                media,
                mark_in,
                out,
                ..
            } => {
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
            .map(|open| AvEnv::for_project(open.store.path()).heard_only())
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
            self.pool.bin = format!("{to}{rest}");
        }
        if let Request::MediaAddFolder { .. } = &request
            && let Ok(r) = &result
            && !r.dry_run
            && let Some(bin) = r.result["bin"].as_str()
            && self.pool.bin != bin
        {
            self.pool.bin = bin.to_string();
            if !changed {
                self.refresh_source();
            }
        }
        if changed {
            self.project_changed(result.as_ref().ok());
        }
        if let Ok(r) = &result {
            match &request {
                Request::EditCopy { .. } | Request::EditCut { .. } => {
                    if let Ok(clips) = serde_json::from_value(r.result["clips"].clone()) {
                        self.clipboard = clips;
                        self.key_clipboard.clear();
                    }
                }
                Request::KeyframesCopy { .. } | Request::KeyframesCut { .. } => {
                    if let Ok(keys) = serde_json::from_value(r.result["keyframes"].clone()) {
                        self.key_clipboard = keys;
                        self.clipboard.clear();
                    }
                }
                _ => {}
            }
            if !r.dry_run && is_edit_function(&request) && origin != (Origin::Gui { drag: true }) {
                self.playhead_after_edit(r);
            }
        }
        result
    }

    fn open_project(&mut self, path: &Path, create: Option<project::New>) -> Result<()> {
        let open = OpenProject::open(path, create, self.tx.clone())?;
        let proxies = AvEnv::for_project(open.store.path()).proxy_dir();
        for m in &open.project().media {
            if let Some(proxy) = &m.proxy {
                disk_cache::used(&proxy.path_in(&proxies));
            }
        }
        self.project = Some(open);
        self.selection = Selection::default();
        self.source_media = None;
        self.source_content = None;
        self.source_positions.clear();
        self.tape_position = 0;
        self.player.unload(SOURCE);
        self.player.unload(TIMELINE);
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
            let _ = vidcut_core::temp::write(&file, json!(recent).to_string().as_bytes());
        }
    }

    fn project_changed(&mut self, response: Option<&Response>) {
        if let Some(trouble) = self.project.as_mut().and_then(OpenProject::trouble) {
            self.notice(trouble, true);
        }
        let Some(open) = &self.project else {
            self.project_snap = None;
            self.missing_watch.watch(Vec::new(), Vec::new());
            return;
        };
        let folder = AvEnv::for_project(open.store.path()).proxy_dir();
        let p = open.project();
        self.missing_watch.watch(
            p.media
                .iter()
                .map(|m| (m.id.clone(), m.path.clone()))
                .collect(),
            p.media
                .iter()
                .filter_map(|m| Some((m.id.clone(), m.proxy.as_ref()?.path_in(&folder))))
                .collect(),
        );
        self.proxy_dir = Arc::from(folder);
        let project = Arc::new(p.clone());
        let path = open.store.path().to_path_buf();
        let history = match self.project.as_mut().map(OpenProject::steps) {
            Some(Ok(steps)) => Arc::new(steps),
            Some(Err(e)) => {
                self.notice(e.message, true);
                self.project_snap
                    .as_ref()
                    .map(|s| s.history.clone())
                    .unwrap_or_default()
            }
            None => Default::default(),
        };
        self.history_preview = None;
        self.preview_wanted = None;
        self.project_snap = Some(ProjectSnap {
            path,
            colors: vidcut_core::render::timeline_colors(&project),
            project: project.clone(),
            history,
        });
        let p = &*project;
        let media: HashSet<String> = p.media.iter().map(|m| m.id.clone()).collect();
        self.selection.media.retain(|m| media.contains(m));
        self.selection.clips.retain(|c| p.clip(c).is_some());
        self.selection.cues.retain(|c| p.cue(c).is_some());
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

    fn refresh_transcribed(&mut self, job_ended: bool) {
        let Some(open) = &self.project else { return };
        let now = crate::env::transcribed(&open.project().media);
        if job_ended || *self.transcribed != now {
            self.transcribed = Arc::new(now);
            self.dirty = true;
        }
    }

    fn check_media(&mut self) {
        self.refresh_transcribed(false);
        let Some(open) = &self.project else { return };
        let media: Vec<_> = open.project().media.clone();
        let channels = open.project().settings.channels;
        for m in media {
            let version = media_version(&m);
            let pictures = format!("{version}|{}", m.color_space.key());
            if m.info.video.is_some() && self.thumbs_wanted.get(&m.id) != Some(&pictures) {
                self.thumbs_wanted.insert(m.id.clone(), pictures.clone());
                if self.thumbs.contains_key(&m.id) {
                    let mut map = (*self.thumbs).clone();
                    map.remove(&m.id);
                    self.thumbs = Arc::new(map);
                }
                let (tx, id, media) = (self.tx.clone(), m.id.clone(), m.clone());
                let wanted = pictures;
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
                let origin = paths::media_origin(&m);
                let cache = Kind::Waveforms.file(&origin, &format!("peaks-{channels}ch"));
                self.workers.run(move || {
                    let cached = disk_cache::read(&cache)
                        .and_then(|b| vidcut_av::waveform::Waveform::from_bytes(origin.check(&b)?));
                    let result = match cached {
                        Some(w) => Ok(w),
                        None => vidcut_av::waveform::waveform(&path, 0, WAVEFORM_RATE, channels)
                            .map_err(|e| e.to_string())
                            .inspect(|w| {
                                let _ = disk_cache::write(&cache, &origin.stamp(&w.to_bytes()));
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

    fn play(&mut self, speed: f64) {
        if !(self.viewer == ViewerMode::Timeline && self.play_skipping(speed)) {
            self.player.play(speed);
        }
    }

    fn play_reviewed(&mut self) {
        let rate = self.review_rate();
        self.review_speed = Some(rate);
        self.play(rate);
    }

    fn follow_review(&mut self) {
        let Some(applied) = self.review_speed else {
            return;
        };
        if !self.player.playing() {
            return;
        }
        let rate = self.review_rate();
        if (rate - applied).abs() > 0.001 {
            self.review_speed = Some(rate);
            self.play(rate);
        }
    }

    fn review_rate(&self) -> f64 {
        fast_review_rate(self.reviewed_frames(), self.project_fps())
    }

    fn project_fps(&self) -> f64 {
        self.project
            .as_ref()
            .map_or(30.0, |o| o.project().settings.fps.as_f64())
    }

    fn reviewed_frames(&self) -> i64 {
        let Some(open) = &self.project else {
            return 0;
        };
        let p = open.project();
        match self.viewer {
            ViewerMode::Timeline => {
                let at = self.player.position(TIMELINE);
                p.tracks
                    .iter()
                    .rev()
                    .filter(|t| t.kind == vidcut_core::model::TrackKind::Video && t.video_enabled)
                    .find_map(|t| {
                        t.clip_at(at)
                            .map(|i| (t.clips[i].end() - t.clips[i].start).max(0))
                    })
                    .unwrap_or(0)
            }
            ViewerMode::SourceClip => self
                .source_media
                .as_ref()
                .and_then(|id| p.media(id).map(|m| p.media_frames(m)))
                .unwrap_or(0),
            ViewerMode::SourceTape => {
                let at = self.player.position(SOURCE);
                tape_at(&self.tape, at)
                    .map(|e| e.frames.max(0))
                    .unwrap_or(0)
            }
        }
    }

    fn play_skipping(&mut self, speed: f64) -> bool {
        let (Some(parts), Some(open)) = (&self.skipping, &self.project) else {
            return false;
        };
        let p = open.project();
        let mut q = p.clone();
        if vidcut_core::ops::remove_parts(&mut q, parts).is_err() {
            return false;
        }
        let skips = vidcut_core::ops::removed_time(parts);
        let at = skipped_to(&skips, self.player.position(TIMELINE));
        self.audition(Some(Audition {
            timeline: Box::new(q),
            at,
            speed,
            skips: Some(skips),
        }))
    }

    fn set_skipping(&mut self, parts: Option<Vec<ClipSpans>>) -> bool {
        self.skipping = parts;
        let skipping = self.player.skipping();
        let starts = self.skipping.is_some()
            && self.viewer == ViewerMode::Timeline
            && !self.player.auditioning();
        if self.player.playing() && (skipping || starts) {
            let speed = self.player.snap(TIMELINE).speed;
            self.play(speed);
            return true;
        }
        skipping && self.player.end_audition().is_some()
    }

    fn audition(&mut self, audition: Option<Audition>) -> bool {
        let Some(a) = audition else {
            return self.player.end_audition().is_some();
        };
        let q = &a.timeline;
        let Ok((mut plan, _)) = render::flatten_lanes(q, 0, q.end()) else {
            return false;
        };
        self.hear_ducking(&mut plan, q);
        let mut plan = self.preview_plan(plan);
        if a.skips.is_none() {
            plan.video = vec![render::VideoPiece::Black {
                frames: plan.frames,
            }];
        }
        self.player.audition(plan, a.at, a.speed, a.skips).is_some()
    }

    fn hear_ducking(&mut self, plan: &mut RenderPlan, p: &Project) {
        if plan.ducking.is_empty() {
            return;
        }
        let mut kept = |m: &vidcut_core::model::Media,
                        stream: u32,
                        detect: vidcut_core::model::DuckDetect|
         -> Result<Option<vidcut_core::duck::Measured>> {
            Ok(crate::env::kept_measure(m, stream, detect))
        };
        let Ok(missing) = vidcut_core::duck::hear(plan, p, &mut kept) else {
            return;
        };
        let missing: Vec<Unmeasured> = missing
            .into_iter()
            .filter(|m| !self.duck_measuring.contains(m) && !self.duck_failed.contains(m))
            .filter(|m| !self.missing.contains(&m.media))
            .collect();
        let wanted: Vec<_> = missing
            .iter()
            .filter_map(|u| Some((p.media(&u.media)?.clone(), u.stream, u.detect)))
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.duck_measuring.extend(missing.iter().cloned());
        let tx = self.tx.clone();
        let _ = crate::threads::spawn("vidcut-measure", Work::Job, move || {
            let mut failed = None;
            for (m, stream, detect) in &wanted {
                let env = AvEnv::new();
                let result = match detect {
                    vidcut_core::model::DuckDetect::Speech => {
                        env.voice_activity(m, *stream, false).map(drop)
                    }
                    vidcut_core::model::DuckDetect::Level => env.audio_levels(m, *stream).map(drop),
                };
                if let Err(e) = result {
                    failed.get_or_insert(e.message);
                }
            }
            let _ = tx.send(Msg::DuckHeard {
                measured: missing,
                failed,
            });
        });
    }

    fn preview_plan(&self, mut plan: RenderPlan) -> RenderPlan {
        if !self.show_super_scale {
            for piece in &mut plan.video {
                for layer in piece.layers_mut() {
                    if let render::VideoPiece::Source { super_scale, .. } = layer {
                        *super_scale = Default::default();
                    }
                }
            }
        }
        if let Some(open) = &self.project {
            vidcut_core::preview::apply_proxies(
                &mut plan,
                open.project(),
                &self.proxy_dir,
                Path::is_file,
            );
        }
        plan
    }

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

    fn refresh_timeline_at(&mut self, at: Option<i64>) -> Option<u64> {
        let mut p = self.project.as_ref()?.project().clone();
        for (id, picture) in &self.free_pictures {
            if let Some((t, c)) = p.find_clip(id) {
                p.tracks[t].clips[c].picture = (!picture.is_usual()).then(|| picture.clone());
            }
        }
        let mut scrolled: Option<(String, (i64, i64), i64)> = None;
        if let Some(FreeTitle {
            title,
            at,
            fraction,
        }) = &self.free_title
        {
            let len = vidcut_core::ops::titles::default_length(&p);
            if let Ok((id, _)) = vidcut_core::ops::titles::add(&mut p, None, *at, len, title)
                && let Some(c) = p
                    .clip(&id)
                    .filter(|c| c.title.as_ref().is_some_and(Title::moves))
            {
                let off = (fraction * len as f64).round() as i64 - (at - c.start);
                scrolled = Some((id.clone(), (c.start, c.end()), off));
            }
        }
        let end = p.end();
        if end == 0 {
            self.player.unload(TIMELINE);
            return None;
        }
        let (mut plan, lanes) = match render::flatten_lanes(&p, 0, end) {
            Ok(planned) => planned,
            Err(e) => {
                self.notice(format!("timeline preview: {}", e.message), true);
                return None;
            }
        };
        if let Some((id, (a, b), off)) = &scrolled {
            let mut from = 0;
            for piece in &mut plan.video {
                let frames = piece.frames();
                if from < *b && from + frames > *a {
                    scroll_by(piece, id, *off);
                }
                from += frames;
            }
        }
        self.hear_ducking(&mut plan, &p);
        let plan = self.preview_plan(plan);
        self.timeline_loaded_at = Some(Instant::now());
        self.player.load(TIMELINE, plan, lanes, at)
    }

    fn refresh_source(&mut self) -> Option<u64> {
        let p = self.project.as_ref()?.project().clone();
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

    fn viewer_set(
        &mut self,
        mode: Option<ViewerMode>,
        display: Option<TimecodeDisplay>,
        reset_timer1: bool,
        paste: Option<String>,
    ) -> Step {
        if mode.is_none() && display.is_none() && !reset_timer1 && paste.is_none() {
            return Step::Now(Err(Error::usage(
                "viewer.set needs mode, display, reset_timer1, or paste",
            )));
        }
        let viewer = mode.unwrap_or(self.viewer);
        let shown = display.unwrap_or(self.timecode_display);
        let id = PlayerId::of(viewer);
        let playhead = self.player.position(id);
        let zero = if reset_timer1 {
            playhead
        } else {
            self.timer1_origin.zero(id)
        };
        let target = if let Some(text) = &paste {
            let view = timecode_view(
                self.project.as_ref().map(|open| open.project()),
                viewer,
                playhead,
                self.tape.as_slice(),
                shown,
                zero,
                self.source_media.as_deref(),
            );
            match seek_frame(text, &view) {
                Ok(frame) => Some(frame),
                Err(e) => return Step::Now(Err(e)),
            }
        } else {
            None
        };
        let motion = mode.and_then(|mode| self.set_viewer(mode));
        if reset_timer1 {
            let id = PlayerId::of(self.viewer);
            self.timer1_origin.reset(id, self.player.position(id));
        }
        if let Some(display) = display {
            self.timecode_display = display;
        }
        let Some(mut frame) = target else {
            return self.shown(motion);
        };
        if reset_timer1 && shown == TimecodeDisplay::Timer1 {
            let id = PlayerId::of(self.viewer);
            frame = frame - zero + self.timer1_origin.zero(id);
        }
        let id = PlayerId::of(self.viewer);
        let motion = self.seek(id, frame, true).or(motion);
        self.shown(motion)
    }

    fn set_viewer(&mut self, mode: ViewerMode) -> Option<u64> {
        if mode == self.viewer && (!mode.is_source() || mode == self.last_source) {
            return None;
        }
        self.viewer = mode;
        if mode.is_source() {
            self.last_source = mode;
            let loaded = self.refresh_source();
            return self.player.show(SOURCE).or(loaded);
        }
        self.player.show(TIMELINE)
    }

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
            (None, Some(pos)) => self.seek(SOURCE, pos, true),
            (None, None) => None,
        })
    }

    fn seek(&mut self, id: PlayerId, frame: i64, sound: bool) -> Option<u64> {
        if id == TIMELINE
            && self.timeline_due.take().is_some()
            && let Some(motion) = self.refresh_timeline_at(Some(frame.max(0)))
        {
            return Some(motion);
        }
        self.player.seek(id, frame, sound)
    }

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

    fn sync_loop(&mut self) {
        let marks = self.looping.then(|| self.loop_marks()).flatten();
        self.player.set_loop(self.looping, marks);
    }

    fn loop_marks(&self) -> Option<(i64, i64)> {
        let p = self.project.as_ref()?.project();
        let tm = &p.tape_marks;
        let span = (self.viewer == ViewerMode::SourceTape
            && tm.order.iter().eq(self.tape.iter().map(|e| &e.media)))
        .then(|| render::crossing_span(&self.tape, tm.mark_in, tm.mark_out))
        .flatten();
        let (a, b) = match span {
            Some(span) => span,
            None => {
                let c = self.viewer_context().ok()?;
                (c.mark_in?, c.mark_out?)
            }
        };
        (b > a).then_some((a, b))
    }

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
        cues: Option<OneOrMany<String>>,
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
        let cues = cues
            .map(|c| {
                c.to_vec()
                    .iter()
                    .map(|r| resolve_cue(p, r, playhead))
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
        if let Some(mut c) = cues {
            c.dedup();
            self.selection.cues = c;
        }
        if let Some(m) = media {
            self.selection.media = m.to_vec();
        }
        Ok(())
    }

    fn gui_change(
        &self,
        changes: Vec<GuiChange>,
        reply: impl FnOnce(GuiState) -> Value + Send + 'static,
    ) -> Step {
        let (tx, rx) = channel();
        self.renderer.gui(
            changes,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        Step::Task(
            Work::Interactive,
            Box::new(move |_| match rx.recv_timeout(WAIT_LIMIT) {
                Ok(Ok(state)) => Ok(answer(reply(state))),
                Ok(Err(why)) => Err(Error::usage(why)),
                Err(RecvTimeoutError::Timeout) => {
                    Err(Error::internal("timed out waiting for the GUI"))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    Err(Error::no_instance("this vidcut has no GUI"))
                }
            }),
        )
    }

    fn captured(&mut self, id: u64, result: Result<Image>) {
        let Some(i) = self
            .waits
            .iter()
            .position(|w| matches!(w.until, Until::Capture { id: c, .. } if c == id))
        else {
            return;
        };
        let w = self.waits.swap_remove(i);
        let Until::Capture { out, player, .. } = w.until else {
            return;
        };
        let blank = player.is_some_and(|p| !self.player.is_loaded(p));
        let result = result.map(|mut image| {
            if blank {
                image.rgba.as_chunks_mut::<4>().0.fill([0, 0, 0, 255]);
            }
            image
        });
        let state = self.state_response();
        self.spawn_task(
            w.job,
            Work::Interactive,
            Box::new(move |_| {
                let image = result?;
                let png =
                    vidcut_av::export::still::png_rgba(image.width, image.height, &image.rgba)
                        .map_err(|e| Error::internal(e.to_string()))?;
                vidcut_core::temp::write(&out, &png)
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

    fn settled(&self) -> bool {
        let p = &self.player;
        p.shown >= p.motion || p.failure().is_some()
    }

    fn capture_viewer(&mut self) -> u64 {
        let id = self.next_capture;
        self.next_capture += 1;
        let tx = self.tx.clone();
        self.renderer.capture_viewer(Box::new(move |result| {
            let _ = tx.send(Msg::Captured { id, result });
        }));
        id
    }

    fn state_response(&self) -> Response {
        let rev = self.project.as_ref().map_or(0, |p| p.project().rev);
        let mut r = answer(self.state_json());
        r.rev = rev;
        r
    }

    fn state_json(&self) -> Value {
        let (late, skipped) = self.player.misses();
        let p = self.project.as_ref().map(|o| o.project());
        let id = PlayerId::of(self.viewer);
        let timecode = format_timecode_field(&timecode_view(
            p,
            self.viewer,
            self.player.position(id),
            self.tape.as_slice(),
            self.timecode_display,
            self.timer1_origin.zero(id),
            self.source_media.as_deref(),
        ))
        .unwrap_or_else(|| "-".into());
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
            "viewer_frame": self.viewer_frame,
            "last_source": self.last_source,
            "source_media": self.source_media,
            "source": source,
            "timeline": player(TIMELINE, p.map_or(0, |p| p.settings.start_tc)),
            "late": late,
            "skipped": skipped,
            "selection": self.selection,
            "pool": self.pool,
            "muted": self.player.muted(),
            "audio_device": self.player.audio_device(),
            "audio_error": self.player.audio_error,
            "audio_scrubbing": self.audio_scrubbing,
            "loop": self.looping,
            "fast_review": self.fast_review,
            "video_only": self.video_only,
            "audio_only": self.audio_only,
            "timecode_display": self.timecode_display,
            "timer1_origin": self.timer1_origin,
            "timecode": timecode,
            "stats": self.player.stats(self.renderer.viewer_stats()),
            "stats_shown": self.stats,
            "guides": self.guides,
            "two_up": self.renderer.two_up(),
            "clipboard": self.clipboard.len(),
            "keyframe_clipboard": self.key_clipboard.len(),
            "preview": self.preview_error.as_deref().map_or(json!(true), |e| json!(e)),
        })
    }
}

type Task =
    Box<dyn FnOnce(Option<(Sender<ReplyEvent>, Arc<AtomicBool>)>) -> Result<Response> + Send>;

enum Step {
    Now(Result<Response>),
    Wait(Until),
    Task(Work, Task),
    Make(Make),
    Measure(Measure, Box<Envelope>),
    Encode(Measure, Box<Envelope>, String, &'static str),
}

type Measure = Box<dyn FnOnce() -> Result<()> + Send>;

type Make = Box<dyn FnOnce() -> Result<(PathBuf, Vec<String>)> + Send>;

fn reply(reply: Option<Reply>, result: Result<Response>) {
    if let Some(r) = reply {
        let _ = r.tx.send(ReplyEvent::Done(result));
    }
}

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

fn job_answer(j: &JobInfo) -> Result<Response> {
    match &j.error {
        Some(e) => Err(Error::new(e.code, format!("job {}: {}", j.id, e.message))),
        None => Ok(answer(j.to_json())),
    }
}

fn to_make(
    path: &Path,
    create: bool,
    force: bool,
    format: &Format,
) -> Result<Option<project::New>> {
    if !format.is_empty() && !create {
        return Err(Error::usage("a frame rate or size needs create"));
    }
    if force && !create {
        return Err(Error::usage(
            "force replaces a file with a new project, so it needs create",
        ));
    }
    if !create {
        return Ok(None);
    }
    if path.exists() && !force {
        if !format.is_empty() {
            return Err(Error::invalid(format!(
                "{} already exists: open it, create another file, or replace it with force",
                path.display()
            )));
        }
        return Ok(None);
    }
    Ok(Some(project::New {
        settings: format.settings()?,
        replace: force,
    }))
}

fn selection_json(selection: &Selection, gui: Option<&GuiState>) -> Value {
    let mut v = json!(selection);
    v["edit"] = json!(gui.and_then(|s| s.edit.as_ref()));
    v["edits"] = json!(gui.map_or(&[][..], |s| &s.edits));
    v
}

fn job_notes(tx: &Sender<Msg>, id: &str) -> vidcut_av::progress::Noting {
    let (tx, id) = (tx.clone(), id.to_string());
    vidcut_av::progress::Noting::set(Arc::new(move |note| {
        let _ = tx.send(Msg::JobNote {
            id: id.clone(),
            note,
        });
    }))
}

fn scroll_by(piece: &mut render::VideoPiece, clip: &str, off: i64) {
    match piece {
        render::VideoPiece::Titled { under, titles } => {
            for s in titles.iter_mut().filter(|s| s.clip == clip) {
                s.run.at += off;
            }
            scroll_by(under, clip, off);
        }
        render::VideoPiece::Mix { under, over } => {
            scroll_by(under, clip, off);
            for o in over {
                scroll_by(&mut o.piece, clip, off);
            }
        }
        _ => {}
    }
}

fn fetches(r: &Request) -> bool {
    match r {
        Request::ClipSet {
            retime: Some(_),
            download: true,
            ..
        }
        | Request::EditChangeSpeed { download: true, .. }
        | Request::SpeedSet { download: true, .. }
        | Request::ProjectSet(vidcut_core::request::ProjectSet { download: true, .. }) => true,
        Request::Apply { commands, .. } => commands.iter().any(fetches),
        _ => false,
    }
}

fn no_project() -> Error {
    Error::usage("no project is open in this vidcut GUI (project.open opens one)")
}

fn cancelled() -> Error {
    Error::new(vidcut_core::ErrorCode::Cancelled, "cancelled")
}

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
    if let Request::SubtitlesTranscribe {
        range: Some(range), ..
    }
    | Request::AnalyzeWords {
        range: Some(range), ..
    } = &mut envelope.request
        && let Ok((a, b)) = context.range(range)
    {
        *range = vidcut_core::time::RangeSpec {
            start: TimeSpec::Frames(a),
            end: vidcut_core::time::RangeEnd::At(TimeSpec::Frames(b)),
        };
    }
    envelope
}

fn folder_notice(result: &Value) -> String {
    let names = |files: &[&Value], why: bool| {
        let mut names: Vec<String> = files
            .iter()
            .take(5)
            .map(|s| {
                let path = Path::new(s["path"].as_str().unwrap_or_default());
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                match (why, s["why"].as_str()) {
                    (true, Some(why)) => format!("{name} ({why})"),
                    _ => name.into_owned(),
                }
            })
            .collect();
        if files.len() > names.len() {
            names.push(format!("and {} more", files.len() - names.len()));
        }
        names.join(", ")
    };
    let clips = result["media"].as_array().map_or(0, Vec::len);
    let bin = result["bin"].as_str().unwrap_or_default();
    let mut text = match clips {
        1 => format!("Import Media Folder: 1 clip in bin {bin}"),
        n => format!("Import Media Folder: {n} clips in bin {bin}"),
    };
    let skipped: Vec<&Value> = result["skipped"].as_array().into_iter().flatten().collect();
    let (others, unread): (Vec<&Value>, Vec<&Value>) = skipped
        .iter()
        .partition(|s| s["why"] == vidcut_core::folder::NOT_MEDIA);
    match others.len() {
        0 => {}
        1 => text.push_str(&format!(
            "; skipped a file that isn't media: {}",
            names(&others, false)
        )),
        n => text.push_str(&format!(
            "; skipped {n} files that aren't media: {}",
            names(&others, false)
        )),
    }
    if !unread.is_empty() {
        text.push_str(&format!("; couldn't open {}", names(&unread, true)));
    }
    for s in &skipped {
        let path = s["path"].as_str().unwrap_or_default();
        let why = s["why"].as_str().unwrap_or_default();
        text.push_str(&format!(
            "
{path}: {why}"
        ));
    }
    text
}

fn edit_only(
    video: bool,
    audio: bool,
    set_video: Option<bool>,
    set_audio: Option<bool>,
) -> std::result::Result<(bool, bool), &'static str> {
    if set_video == Some(true) && set_audio == Some(true) {
        return Err("video only and audio only cannot both be on");
    }
    let mut video = set_video.unwrap_or(video);
    let mut audio = set_audio.unwrap_or(audio);
    if set_video == Some(true) {
        audio = false;
    }
    if set_audio == Some(true) {
        video = false;
    }
    if video && audio {
        return Err("video only and audio only cannot both be on");
    }
    Ok((video, audio))
}

pub(crate) fn fast_review_rate(frames: i64, fps: f64) -> f64 {
    if frames <= 0 || !fps.is_finite() || fps <= 0.0 {
        return 1.0;
    }
    let rate = frames as f64 / fps / 10.0;
    if !rate.is_finite() || rate <= 1.0 {
        1.0
    } else if rate >= 8.0 {
        8.0
    } else {
        rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_only_turns_audio_only_off() {
        assert_eq!(
            edit_only(false, true, Some(true), None).unwrap(),
            (true, false)
        );
    }

    #[test]
    fn audio_only_turns_video_only_off() {
        assert_eq!(
            edit_only(true, false, None, Some(true)).unwrap(),
            (false, true)
        );
    }

    #[test]
    fn both_on_in_one_request_is_refused() {
        assert_eq!(
            edit_only(false, false, Some(true), Some(true)),
            Err("video only and audio only cannot both be on")
        );
    }

    #[test]
    fn a_ten_second_clip_plays_at_normal_speed() {
        assert_eq!(fast_review_rate(240, 24.0), 1.0);
    }

    #[test]
    fn a_twenty_second_clip_plays_at_twice_normal_speed() {
        assert_eq!(fast_review_rate(480, 24.0), 2.0);
    }

    #[test]
    fn a_long_clip_plays_at_most_at_eight_times() {
        assert_eq!(fast_review_rate(2400, 24.0), 8.0);
    }

    #[test]
    fn an_empty_clip_plays_at_normal_speed() {
        assert_eq!(fast_review_rate(0, 24.0), 1.0);
    }

    #[test]
    fn the_folder_notice_names_the_files_skipped() {
        let result = json!({
            "bin": "Shoot",
            "media": ["m1", "m2"],
            "skipped": [
                { "path": "/shoot/broken.mp4", "why": "Invalid data found when processing input" },
                { "path": "/shoot/notes.txt", "why": "not a media file" },
                { "path": "/shoot/Docs/readme.txt", "why": "not a media file" },
            ],
        });
        assert_eq!(
            folder_notice(&result),
            "Import Media Folder: 2 clips in bin Shoot; skipped 2 files that aren't media: \
             notes.txt, readme.txt; couldn't open broken.mp4 (Invalid data found when processing input)\n\
             /shoot/broken.mp4: Invalid data found when processing input\n\
             /shoot/notes.txt: not a media file\n\
             /shoot/Docs/readme.txt: not a media file"
        );
    }
}
