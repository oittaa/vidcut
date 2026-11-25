use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_av::export::SpeechNote;
use vidcut_core::export::{
    AAC_BITRATE, AAC_BITRATES, AAC_RATES, AudioCodec, AudioSettings, Container, EncoderChoice,
    ExportSettings, HdrMetadata, HdrMode, VideoCodec, VideoSettings, apply_overrides, validate,
};
use vidcut_core::request::{
    ExportParams, FindSpeech, ImportMode, LevelsFrom, SubtitleFormat, SubtitlesAs, UniqueAs,
};
use vidcut_core::{Envelope, Error, Response};

use super::export_page::{AddChoice, QueueStep};
use super::folders::Kind;
use super::theme;
use super::workspace::{Workspace, dim, escape_cancels};
use crate::live::{self, JobInfo, JobState, Origin};

#[derive(Debug)]
pub(super) struct Ask {
    slot: Arc<Mutex<Option<vidcut_core::Result<Response>>>>,
    stop: Arc<AtomicBool>,
}

impl Ask {
    pub(super) fn send(
        live: &live::Handle,
        ctx: &egui::Context,
        method: &str,
        params: Value,
    ) -> Self {
        Self::send_json(live, ctx, json!({ "method": method, "params": params }))
    }

    pub(super) fn send_dry(
        live: &live::Handle,
        ctx: &egui::Context,
        method: &str,
        params: Value,
    ) -> Self {
        let envelope = json!({ "method": method, "params": params, "dry_run": true });
        Self::send_json(live, ctx, envelope)
    }

    fn send_json(live: &live::Handle, ctx: &egui::Context, envelope: Value) -> Self {
        let envelope = Envelope::from_json(envelope);
        let slot = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, halt, live, ctx) = (slot.clone(), stop.clone(), live.clone(), ctx.clone());
        let _ = crate::threads::spawn("vidcut-ask", crate::threads::Work::Interactive, move || {
            let mut going = |_| !halt.load(Ordering::Relaxed);
            let result = envelope
                .map_err(Error::internal)
                .and_then(|e| live.call(e, Origin::Gui { drag: false }, None, &mut going));
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            ctx.request_repaint();
        });
        Self { slot, stop }
    }

    pub(super) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub(super) fn take(&self) -> Option<vidcut_core::Result<Response>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

pub trait Dialogs: Send + Sync {
    fn save_file(
        &self,
        title: &str,
        folder: &Path,
        name: &str,
        extensions: &[&str],
    ) -> Option<PathBuf>;
    fn pick_file(&self, title: &str, folder: &Path, extensions: &[&str]) -> Option<PathBuf>;
    fn pick_folder(&self, title: &str, folder: &Path) -> Option<PathBuf>;
    fn pick_files(
        &self,
        _title: &str,
        _folder: &Path,
        _filters: &[(&str, &[&str])],
    ) -> Vec<PathBuf> {
        Vec::new()
    }
}

pub struct RfdDialogs;

impl Dialogs for RfdDialogs {
    fn save_file(
        &self,
        title: &str,
        folder: &Path,
        name: &str,
        extensions: &[&str],
    ) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title(title)
            .set_directory(folder)
            .set_file_name(name)
            .add_filter(extensions.join(", "), extensions)
            .save_file()
    }

    fn pick_file(&self, title: &str, folder: &Path, extensions: &[&str]) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title(title)
            .set_directory(folder)
            .add_filter(extensions.join(", "), extensions)
            .pick_file()
    }

    fn pick_folder(&self, title: &str, folder: &Path) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title(title)
            .set_directory(folder)
            .pick_folder()
    }

    fn pick_files(&self, title: &str, folder: &Path, filters: &[(&str, &[&str])]) -> Vec<PathBuf> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .set_directory(folder);
        for (name, extensions) in filters {
            dialog = dialog.add_filter(*name, extensions);
        }
        dialog
            .add_filter("all files", &["*"])
            .pick_files()
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub(super) struct Picked(Arc<Mutex<Option<Option<PathBuf>>>>);

impl Picked {
    pub(super) fn ask(
        ctx: &egui::Context,
        f: impl FnOnce() -> Option<PathBuf> + Send + 'static,
    ) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let (out, ctx) = (slot.clone(), ctx.clone());
        let _ = crate::threads::spawn(
            "vidcut-file-dialog",
            crate::threads::Work::Interactive,
            move || {
                let path = f();
                *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                ctx.request_repaint();
            },
        );
        Self(slot)
    }

    pub(super) fn take(&self) -> Option<Option<PathBuf>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct PresetInfo {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub quick_export: bool,
    pub family: Option<String>,
    #[serde(default)]
    pub user: bool,
    pub settings: ExportSettings,
    #[serde(default)]
    pub unavailable: Option<String>,
}

#[derive(Default)]
pub struct ExportUi {
    pub(super) presets: Vec<PresetInfo>,
    presets_ask: Option<Ask>,
    pub(super) encodable: Value,
    encodable_ask: Option<Ask>,
    quick: Option<Quick>,
    pub(super) render: Option<Box<Render>>,
    render_wanted: bool,
    pub(super) viewer_before: Option<vidcut_core::request::ViewerMode>,
    pub(super) selected: Vec<String>,
    pub(super) editing: Option<String>,
    pub(super) edit_wanted: Option<String>,
    pub(super) leave_edit: Option<super::export_page::LeaveEdit>,
    pub(super) job_details: bool,
    pub(super) queue: Option<super::export_page::QueueStep>,
    pub(super) renaming: Option<super::export_page::Renaming>,
    starting: Option<Starting>,
    pub show_jobs: bool,
    jobs_seen: std::collections::HashMap<String, JobState>,
    jobs_glow_until: f64,
    jobs_failed_unseen: bool,
    otio: Option<Otio>,
    flatten: bool,
    import_name: String,
    warnings: Option<(String, Vec<String>)>,
    confirm_quit: bool,
    pub quit_confirmed: bool,
    pub(super) jobs_window: super::window::Tool,
}

const JOBS: super::window::Separate = super::window::Separate {
    title: "Jobs",
    file: "window-jobs.json",
    size: [460.0, 560.0],
};

impl ExportUi {
    pub(super) fn quick_open(&self) -> bool {
        self.quick.is_some()
    }

    pub(super) fn close_quick(&mut self) {
        self.quick = None;
    }

    pub(super) fn busy(&self) -> bool {
        self.starting.is_some()
    }

    pub(super) fn switch_project(&mut self) {
        self.render = None;
        self.render_wanted = false;
        self.selected.clear();
        self.editing = None;
        self.edit_wanted = None;
        self.leave_edit = None;
        self.queue = None;
        self.renaming = None;
    }

    pub(super) fn asking(&self, jobs: &[JobInfo]) -> bool {
        matches!(self.otio, Some(Otio::Choose(..) | Otio::Missing { .. }))
            || (self.confirm_quit && jobs.iter().any(JobInfo::unfinished))
            || matches!(
                self.queue,
                Some(QueueStep::ReRender { .. } | QueueStep::Replace { .. })
            )
            || self.leave_edit.is_some()
            || self.starting.as_ref().is_some_and(|s| {
                !s.offline.is_empty() || s.weights || (!s.existing.is_empty() && s.rename.is_none())
            })
    }
}

struct Quick {
    selected: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum From {
    Quick,
    Page,
}

struct Starting {
    params: Value,
    from: From,
    update: Option<Update>,
    picking: Option<Picked>,
    ask: Option<Ask>,
    weights: bool,
    offline: Vec<String>,
    check: Option<Ask>,
    existing: Vec<String>,
    rename: Option<Picked>,
}

pub(super) struct Update {
    pub(super) job: String,
    pub(super) next: Option<String>,
}

impl Starting {
    fn method(&self) -> &'static str {
        match (self.from, &self.update) {
            (From::Quick, _) => "export.start",
            (From::Page, None) => "render.add",
            (From::Page, Some(_)) => "render.update",
        }
    }
}

enum Otio {
    PickImport(Picked),
    Choose(PathBuf, String),
    Importing {
        ask: Ask,
        path: PathBuf,
        mode: ImportMode,
    },
    Missing {
        path: PathBuf,
        mode: ImportMode,
        message: String,
    },
    PickSearch {
        picked: Picked,
        path: PathBuf,
        mode: ImportMode,
    },
    PickExport(Picked, &'static str),
    Exporting(Ask),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Tab {
    Video,
    Audio,
    File,
}

pub(super) struct Render {
    pub(super) preset: String,
    base: ExportSettings,
    pub(super) settings: ExportSettings,
    stashed_video: Option<VideoSettings>,
    pub(super) range: String,
    pub(super) per_clip: bool,
    pub(super) template: String,
    pub(super) source_names: bool,
    suffix: String,
    frame_count: bool,
    unique: Option<UniqueAs>,
    subfolder: String,
    separate_folders: bool,
    source_levels: u32,
    levels_from: LevelsFrom,
    pub(super) name: String,
    pub(super) folder: String,
    pub(super) overwrite: bool,
    pub(super) picking: Option<Picked>,
    plan: Option<Ask>,
    planned: Option<(Value, u64)>,
    pub(super) plan_result: Option<std::result::Result<Value, String>>,
    pub(super) error: Option<String>,
    pub(super) save: Option<SavePreset>,
    subtitles: Option<SubtitlesAs>,
    subtitle_tracks: Vec<String>,
    subtitle_format: SubtitleFormat,
    pub(super) tab: Tab,
}

pub(super) struct SavePreset {
    name: String,
    title: String,
    quick_export: bool,
    overwrite: bool,
    ask: Option<Ask>,
    error: Option<String>,
}

fn video_codecs(c: Container) -> &'static [VideoCodec] {
    use VideoCodec as V;
    match c {
        Container::Mp4 => &[V::H264, V::Hevc, V::Av1, V::Vp9],
        Container::Mov => &[V::H264, V::Hevc, V::Prores, V::Dnxhr],
        Container::Mkv => &[V::H264, V::Hevc, V::Av1, V::Vp9, V::Prores, V::Dnxhr],
        Container::Webm => &[V::Vp9, V::Av1],
        Container::Tiff => &[V::Tiff],
        Container::Exr => &[V::Exr],
        _ => &[],
    }
}

fn audio_codecs(c: Container) -> &'static [AudioCodec] {
    use AudioCodec as A;
    match c {
        Container::Mp4 => &[A::Aac, A::Opus, A::Mp3, A::Flac],
        Container::Mov => &[A::Aac, A::PcmS24, A::PcmS16],
        Container::Mkv => &[A::Aac, A::Opus, A::Flac, A::PcmS24, A::PcmS16, A::Mp3],
        Container::Webm | Container::Ogg => &[A::Opus],
        Container::M4a => &[A::Aac],
        Container::Mp3 => &[A::Mp3],
        Container::Flac => &[A::Flac],
        Container::Wav => &[A::PcmS24, A::PcmS16],
        Container::Tiff | Container::Exr => &[],
    }
}

#[cfg(windows)]
const GPU_ENCODERS: &[EncoderChoice] =
    &[EncoderChoice::Nvenc, EncoderChoice::Amf, EncoderChoice::Qsv];
#[cfg(target_os = "macos")]
const GPU_ENCODERS: &[EncoderChoice] = &[EncoderChoice::VideoToolbox];
#[cfg(not(any(windows, target_os = "macos")))]
const GPU_ENCODERS: &[EncoderChoice] = &[
    EncoderChoice::Nvenc,
    EncoderChoice::Vaapi,
    EncoderChoice::Qsv,
];

const CONTAINERS: [Container; 11] = [
    Container::Mp4,
    Container::Mov,
    Container::Mkv,
    Container::Webm,
    Container::Tiff,
    Container::Exr,
    Container::Wav,
    Container::Flac,
    Container::Mp3,
    Container::M4a,
    Container::Ogg,
];

fn profiles(c: VideoCodec) -> &'static [&'static str] {
    match c {
        VideoCodec::Prores => &["proxy", "lt", "standard", "hq", "4444", "xq"],
        VideoCodec::Dnxhr => &["lb", "sq", "hq", "hqx", "444"],
        VideoCodec::H264 => &["baseline", "main", "high"],
        VideoCodec::Hevc => &["main", "main10"],
        VideoCodec::Av1 | VideoCodec::Vp9 => &[],
        VideoCodec::Tiff => &vidcut_core::export::TIFF_PROFILES,
        VideoCodec::Exr => &vidcut_core::export::EXR_PROFILES,
    }
}

fn fixed_format(c: VideoCodec) -> bool {
    matches!(
        c,
        VideoCodec::Prores | VideoCodec::Dnxhr | VideoCodec::Tiff | VideoCodec::Exr
    )
}

pub(super) fn name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn lossy(a: AudioCodec) -> bool {
    matches!(a, AudioCodec::Aac | AudioCodec::Opus | AudioCodec::Mp3)
}

fn usual_bitrate(a: AudioCodec) -> u64 {
    match a {
        AudioCodec::Aac => AAC_BITRATE,
        _ => 256_000,
    }
}

fn fit_codecs(s: &mut ExportSettings) {
    if s.container.audio_only() {
        s.video = None;
    }
    if let Some(v) = &mut s.video {
        let ok = video_codecs(s.container);
        if !ok.contains(&v.codec) {
            v.codec = ok[0];
            v.profile = None;
            v.filename_digits = None;
            v.first_frame = None;
            v.compression_level = None;
            if fixed_format(v.codec) {
                v.bit_depth = None;
            }
        }
        if v.image_bits().is_some() {
            if !matches!(v.hdr_metadata, HdrMetadata::Auto | HdrMetadata::None) {
                v.hdr_metadata = HdrMetadata::Auto;
            }
            v.mastering_display = None;
            v.max_cll = None;
        }
    }
    if s.container.image_sequence() {
        s.audio = None;
    }
    if let Some(a) = &mut s.audio {
        let ok = audio_codecs(s.container);
        if !ok.contains(&a.codec) {
            a.codec = ok[0];
            a.bitrate = lossy(a.codec).then_some(a.bitrate.unwrap_or(usual_bitrate(a.codec)));
        }
    }
}

pub(super) fn overrides(base: &ExportSettings, settings: &ExportSettings) -> Vec<String> {
    fn raw(v: &Value) -> String {
        match v {
            Value::String(s) if serde_json::from_str::<Value>(s).is_err() => s.clone(),
            other => other.to_string(),
        }
    }
    fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
                keys.sort();
                keys.dedup();
                for k in keys {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    diff(
                        &p,
                        x.get(k).unwrap_or(&Value::Null),
                        y.get(k).unwrap_or(&Value::Null),
                        out,
                    );
                }
            }
            _ if a == b => {}
            _ => out.push(format!("{path}={}", raw(b))),
        }
    }
    let mut out = Vec::new();
    diff(
        "",
        &serde_json::to_value(base).unwrap_or_default(),
        &serde_json::to_value(settings).unwrap_or_default(),
        &mut out,
    );
    out
}

pub(super) fn command(project: &Path, p: &Value) -> String {
    let mut p = p.clone();
    if let Some(m) = p.as_object_mut() {
        m.remove("background");
    }
    crate::cli::command_line(project, "export.start", &p).unwrap_or_default()
}

pub(super) fn near_center(ctx: &egui::Context, w: f32, h: f32) -> egui::Pos2 {
    let r = ctx.content_rect();
    egui::pos2(
        (r.center().x - w / 2.0).max(r.left() + 8.0),
        (r.center().y - h / 2.0).max(r.top() + 40.0),
    )
}

pub(super) fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn job_paths(j: &JobInfo) -> Option<&Path> {
    if j.params["per_clip"] == true {
        j.params["output"].as_str().map(Path::new)
    } else {
        j.outputs.first().map(|o| o.path.as_path())
    }
}

impl SavePreset {
    pub(super) fn new() -> Self {
        Self {
            name: String::new(),
            title: String::new(),
            quick_export: false,
            overwrite: false,
            ask: None,
            error: None,
        }
    }
}

impl Render {
    fn new(preset: &PresetInfo, name: String, folder: String) -> Self {
        Self {
            preset: preset.name.clone(),
            base: preset.settings.clone(),
            settings: preset.settings.clone(),
            stashed_video: None,
            range: String::new(),
            per_clip: false,
            template: CLIP_NAME.into(),
            source_names: false,
            suffix: String::new(),
            frame_count: true,
            unique: None,
            subfolder: String::new(),
            separate_folders: false,
            source_levels: 0,
            levels_from: LevelsFrom::Tail,
            name,
            folder,
            overwrite: false,
            picking: None,
            plan: None,
            planned: None,
            plan_result: None,
            error: None,
            save: None,
            subtitles: None,
            subtitle_tracks: Vec::new(),
            subtitle_format: SubtitleFormat::Srt,
            tab: Tab::Video,
        }
    }

    pub(super) fn use_preset(&mut self, preset: &PresetInfo) {
        self.preset = preset.name.clone();
        self.base = preset.settings.clone();
        self.settings = preset.settings.clone();
        self.stashed_video = None;
    }

    pub(super) fn changed(&self) -> bool {
        self.settings != self.base
    }

    pub(super) fn load_job(
        &mut self,
        preset: &PresetInfo,
        e: &ExportParams,
        shown: Option<String>,
    ) -> std::result::Result<(), String> {
        let mut settings = e
            .settings
            .clone()
            .unwrap_or_else(|| preset.settings.clone());
        apply_overrides(&mut settings, &e.set).map_err(|x| x.message)?;
        let ext = e.output.extension().and_then(|x| x.to_str());
        if !e.per_clip
            && let Some(c) = ext.and_then(Container::from_extension)
        {
            settings.container = c;
        }
        if let Some(encoder) = e.encoder
            && let Some(v) = settings.video.as_mut()
        {
            v.encoder = encoder;
        }
        if e.audio_only || settings.container.audio_only() {
            settings.video = None;
        }
        self.preset = preset.name.clone();
        self.base = preset.settings.clone();
        self.settings = settings;
        self.stashed_video = None;
        self.per_clip = e.per_clip;
        self.template = e.name.clone().unwrap_or_else(|| CLIP_NAME.into());
        self.source_names = e.source_names;
        self.suffix = e.suffix.clone().unwrap_or_default();
        self.frame_count = !e.no_frame_count;
        self.unique = e.unique;
        self.subfolder = e.subfolder.clone().unwrap_or_default();
        self.separate_folders = e.separate_folders;
        self.source_levels = e.source_levels;
        self.levels_from = e.levels_from;
        match e.per_clip {
            true => self.folder = e.output.display().to_string(),
            false => {
                if let Some(stem) = e.output.file_stem() {
                    self.name = stem.to_string_lossy().into_owned();
                }
                if let Some(folder) = e.output.parent() {
                    self.folder = folder.display().to_string();
                }
            }
        }
        self.overwrite = e.overwrite;
        self.subtitles = match (&e.subtitles, e.subtitles_as) {
            (_, Some(how)) => Some(how),
            (Some(_), None) => Some(SubtitlesAs::File),
            (None, None) => None,
        };
        self.subtitle_tracks = match (&e.subtitles, e.subtitles_as) {
            (Some(tracks), _) => tracks.clone(),
            (None, Some(_)) => shown.into_iter().collect(),
            (None, None) => Vec::new(),
        };
        self.subtitle_format = e.subtitle_format.unwrap_or(SubtitleFormat::Srt);
        self.error = None;
        self.save = None;
        Ok(())
    }

    pub(super) fn output(&self) -> PathBuf {
        let folder = PathBuf::from(self.folder.trim());
        let name = self.name.trim();
        match (self.per_clip, name.is_empty()) {
            (true, _) => folder,
            (false, true) => PathBuf::new(),
            (false, false) => {
                folder.join(format!("{name}.{}", self.settings.container.extension()))
            }
        }
    }

    fn clip_names(&self, p: &mut Value) {
        p["per_clip"] = json!(true);
        if self.source_names {
            p["source_names"] = json!(true);
        } else {
            if self.template != CLIP_NAME {
                p["name"] = json!(self.template);
            }
            if !self.suffix.is_empty() {
                p["suffix"] = json!(self.suffix);
            }
            if !self.frame_count {
                p["no_frame_count"] = json!(true);
            }
        }
        if let Some(how) = self.unique {
            p["unique"] = json!(how);
        }
        if !self.subfolder.trim().is_empty() {
            p["subfolder"] = json!(self.subfolder.trim());
        }
        if self.separate_folders {
            p["separate_folders"] = json!(true);
        }
        if self.source_levels > 0 {
            p["source_levels"] = json!(self.source_levels);
            if self.levels_from != LevelsFrom::Tail {
                p["levels_from"] = json!(self.levels_from);
            }
        }
    }

    pub(super) fn params(&self) -> Value {
        let path = self.output();
        let output = path.to_string_lossy();
        let output = output.as_ref();
        let mut p = json!({ "output": output, "preset": self.preset });
        let (mut base, mut settings) = (self.base.clone(), self.settings.clone());
        if settings.video.is_none() && base.video.is_some() {
            p["audio_only"] = json!(true);
            base.video = None;
            settings.video = None;
        }
        let ext = Path::new(output)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(Container::from_extension);
        if !self.per_clip && ext == Some(settings.container) {
            base.container = settings.container;
        }
        if !self.per_clip
            && !settings.container.image_sequence()
            && let Some(v) = settings.video.as_mut()
        {
            v.filename_digits = None;
        }
        let set = overrides(&base, &settings);
        if !set.is_empty() {
            p["set"] = json!(set);
        }
        p["range"] = json!(self.range);
        if self.per_clip {
            self.clip_names(&mut p);
        }
        if self.overwrite {
            p["overwrite"] = json!(true);
        }
        if let Some(how) = self.subtitles
            && !self.subtitle_tracks.is_empty()
        {
            p["subtitles"] = json!(self.subtitle_tracks);
            if how != SubtitlesAs::File {
                p["subtitles_as"] = json!(how);
            } else if self.subtitle_format != SubtitleFormat::Srt {
                p["subtitle_format"] = json!(self.subtitle_format);
            }
        }
        p
    }
}

fn size_of(p: &PresetInfo) -> String {
    p.settings
        .video
        .as_ref()
        .and_then(|v| v.height.map(|h| format!("{h}p")))
        .unwrap_or_else(|| "timeline size".into())
}

impl Workspace {
    pub(super) fn open_quick_export(&mut self, ctx: &egui::Context) {
        self.refresh_presets(ctx);
        self.export.quick = Some(Quick {
            selected: None,
            error: None,
        });
    }

    pub(super) fn want_render_settings(&mut self, ctx: &egui::Context) {
        if self.export.presets.is_empty() && self.export.presets_ask.is_none() {
            self.refresh_presets(ctx);
        }
        if self.export.render.is_none() {
            self.export.render_wanted = true;
        }
    }

    pub(super) fn refresh_presets(&mut self, ctx: &egui::Context) {
        self.export.presets_ask = Some(Ask::send(&self.live, ctx, "presets.list", json!({})));
        self.export.encodable_ask = Some(Ask::send(&self.live, ctx, "encoders.list", json!({})));
    }

    pub(super) fn export_folder(&self) -> PathBuf {
        self.folders
            .get(Kind::Exports)
            .or_else(|| self.project_folder())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub(super) fn project_stem(&self) -> String {
        self.snap
            .project
            .as_ref()
            .and_then(|p| p.path.file_stem())
            .map_or("export".into(), |s| s.to_string_lossy().into_owned())
    }

    fn animated_track(&self) -> Option<(String, String)> {
        let p = self.project()?;
        let t = p.subtitles.iter().find(|t| t.shown)?;
        let words = t.style.words.is_some() && t.cues.iter().any(|c| c.has_times());
        words.then(|| (t.id.clone(), t.display_name()))
    }

    pub(super) fn export_windows(&mut self, ctx: &egui::Context) {
        if let Some(ask) = &self.export.presets_ask
            && let Some(r) = ask.take()
        {
            self.export.presets_ask = None;
            match r.and_then(|r| {
                serde_json::from_value::<Vec<PresetInfo>>(r.result)
                    .map_err(|e| Error::internal(e.to_string()))
            }) {
                Ok(list) => self.export.presets = list,
                Err(e) => self.say(format!("presets: {}", e.message)),
            }
        }
        if let Some(ask) = &self.export.encodable_ask
            && let Some(r) = ask.take()
        {
            self.export.encodable_ask = None;
            match r {
                Ok(r) => self.export.encodable = r.result,
                Err(e) => self.say(format!("encoders: {}", e.message)),
            }
        }
        if self.export.render_wanted && !self.export.presets.is_empty() {
            self.export.render_wanted = false;
            let preset = self
                .export
                .presets
                .iter()
                .find(|p| p.name == "h265-master")
                .unwrap_or(&self.export.presets[0])
                .clone();
            let folder = self.export_folder().display().to_string();
            let mut render = Render::new(&preset, self.project_stem(), folder);
            if let Some((track, _)) = self.animated_track() {
                render.subtitles = Some(SubtitlesAs::Burn);
                render.subtitle_tracks = vec![track];
            }
            self.export.render = Some(Box::new(render));
        }
        self.quick_window(ctx);
        self.starting(ctx);
        self.jobs_window(ctx);
        self.otio_windows(ctx);
        self.quit_window(ctx);
    }

    fn quick_window(&mut self, ctx: &egui::Context) {
        let Some(mut q) = self.export.quick.take() else {
            return;
        };
        let animated = self.animated_track();
        let all = self.export.presets.clone();
        let mut presets: Vec<PresetInfo> = Vec::new();
        for p in all.iter().filter(|p| p.quick_export) {
            if p.family.is_none() || presets.iter().all(|k| k.family != p.family) {
                presets.push(p.clone());
            }
        }
        if q.selected.is_none() {
            q.selected = presets
                .iter()
                .find(|p| p.unavailable.is_none())
                .map(|p| p.name.clone());
        }
        let range = match self.project().map(|p| (p.marks.clone(), p.end())) {
            Some((m, _)) if m.mark_in.is_some() && m.mark_out.is_some() => {
                let (a, b) = (m.mark_in.unwrap_or(0), m.mark_out.unwrap_or(0));
                format!(
                    "In–Out: {} – {} ({})",
                    self.tc(a),
                    self.tc(b),
                    self.src_tc(b - a)
                )
            }
            Some((_, end)) => format!("Whole timeline ({})", self.src_tc(end)),
            None => String::new(),
        };
        let mut open = true;
        let mut export = false;
        let mut cancel = false;
        super::dialog::window(ctx, "Quick Export", &mut open, |ui| {
            if presets.is_empty() {
                super::dialog::note(ui, "Loading presets…");
                return;
            }
            ui.horizontal_wrapped(|ui| {
                for p in &presets {
                    let shown = all
                        .iter()
                        .find(|m| {
                            p.family.is_some()
                                && m.family == p.family
                                && Some(&m.name) == q.selected.as_ref()
                        })
                        .unwrap_or(p);
                    let selected = q.selected.as_deref() == Some(shown.name.as_str());
                    let title = p.family.as_deref().unwrap_or(&p.title);
                    let text = match shown.unavailable {
                        Some(_) => format!("{title}\nNot available here"),
                        None => format!(
                            "{title}\n{} · {}",
                            shown.settings.container.extension().to_uppercase(),
                            size_of(shown)
                        ),
                    };
                    let r = ui
                        .add_enabled_ui(shown.unavailable.is_none(), |ui| {
                            ui.add_sized([150.0, 56.0], egui::Button::selectable(selected, text))
                        })
                        .inner;
                    if r.clicked() {
                        if !selected {
                            q.selected = Some(p.name.clone());
                        }
                        q.error = None;
                    }
                }
            });
            let picked = all.iter().find(|p| Some(&p.name) == q.selected.as_ref());
            if let Some(p) = picked {
                super::dialog::note(ui, p.description.clone());
            }
            super::dialog::form(ui, "quick-export-form", |ui| {
                if let Some(family) = picked.and_then(|p| p.family.as_ref()) {
                    super::dialog::row(ui, "Resolution", |ui| {
                        egui::ComboBox::from_id_salt("quick-export-resolution")
                            .selected_text(picked.map(size_of).unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for m in all.iter().filter(|m| m.family.as_ref() == Some(family)) {
                                    let on = Some(&m.name) == q.selected.as_ref();
                                    if ui.selectable_label(on, size_of(m)).clicked() && !on {
                                        q.selected = Some(m.name.clone());
                                        q.error = None;
                                    }
                                }
                            });
                    });
                }
                super::dialog::row(ui, "Range", |ui| ui.label(&range));
                if let Some((_, name)) = &animated {
                    super::dialog::row(ui, "Subtitles", |ui| {
                        ui.label(format!("{name} burned in, words as spoken"))
                    });
                }
            });
            if let Some(e) = &q.error {
                super::dialog::warn(ui, e.clone());
            }
            let busy = self.export.starting.is_some();
            let usable = picked.is_some_and(|p| p.unavailable.is_none());
            super::dialog::footer(ui, |ui| {
                export = super::dialog::main_button(ui, "Export…", !busy && usable);
                cancel = ui.button("Cancel").clicked();
                match picked {
                    None => super::dialog::why(ui, "Pick a preset"),
                    Some(p) if p.unavailable.is_some() => {
                        super::dialog::why(ui, format!("{} isn't available here", p.title))
                    }
                    _ => {}
                }
            });
        });
        if cancel {
            open = false;
        }
        if export && let Some(name) = q.selected.clone() {
            let ext = all
                .iter()
                .find(|p| p.name == name)
                .map_or("mp4", |p| p.settings.container.extension());
            let file = format!("{}.{ext}", self.project_stem());
            let (dialogs, folder) = (self.dialogs.clone(), self.export_folder());
            let picking = Picked::ask(ctx, move || {
                dialogs.save_file("Quick Export", &folder, &file, &[ext])
            });
            q.error = None;
            let mut params = json!({ "preset": name, "overwrite": true, "background": true });
            if animated.is_some() {
                params["subtitles_as"] = json!(SubtitlesAs::Burn);
            }
            self.export.starting = Some(Starting {
                params,
                from: From::Quick,
                update: None,
                picking: Some(picking),
                ask: None,
                weights: false,
                offline: Vec::new(),
                check: None,
                existing: Vec::new(),
                rename: None,
            });
        }
        if open {
            self.export.quick = Some(q);
        }
    }

    fn starting(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.export.starting.take() else {
            return;
        };
        if let Some(p) = &s.picking
            && let Some(choice) = p.take()
        {
            s.picking = None;
            match choice {
                None => return,
                Some(path) => {
                    self.folders.remember(Kind::Exports, &path);
                    s.params["output"] = json!(path);
                    self.send_start(ctx, &mut s);
                }
            }
        }
        if !s.offline.is_empty() {
            match ask_offline(ctx, &s.offline) {
                Some(true) => {
                    s.offline.clear();
                    self.send_checked(ctx, &mut s);
                }
                Some(false) => return,
                None => {}
            }
        }
        if let Some(check) = &s.check
            && let Some(r) = check.take()
        {
            s.check = None;
            match r {
                Ok(r) => {
                    let files = r.result["existing"].as_array().into_iter().flatten();
                    s.existing = files.filter_map(Value::as_str).map(String::from).collect();
                    if s.existing.is_empty() {
                        self.send_now(ctx, &mut s);
                    }
                }
                Err(e) => {
                    if let Some(d) = &mut self.export.render {
                        d.error = Some(e.message);
                    }
                    return;
                }
            }
        }
        if !s.existing.is_empty() {
            let rename = !s.params["per_clip"].as_bool().unwrap_or(false);
            match super::export_page::ask_add_replace(ctx, &s.existing, rename) {
                Some(AddChoice::Replace) => {
                    s.existing.clear();
                    s.params["overwrite"] = json!(true);
                    self.send_now(ctx, &mut s);
                }
                Some(AddChoice::Rename) => {
                    s.existing.clear();
                    let output = PathBuf::from(s.params["output"].as_str().unwrap_or_default());
                    let folder = output.parent().map(Path::to_path_buf).unwrap_or_default();
                    let file = output.file_name().map(|f| f.to_string_lossy().to_string());
                    let ext = output.extension().map(|e| e.to_string_lossy().to_string());
                    let (file, ext) = (file.unwrap_or_default(), ext.unwrap_or_default());
                    let dialogs = self.dialogs.clone();
                    s.rename = Some(Picked::ask(ctx, move || {
                        dialogs.save_file("Rename", &folder, &file, &[ext.as_str()])
                    }));
                }
                Some(AddChoice::Cancel) => return,
                None => {}
            }
        }
        if let Some(p) = &s.rename
            && let Some(choice) = p.take()
        {
            s.rename = None;
            let Some(path) = choice else {
                return;
            };
            s.params["output"] = json!(path);
            s.params["overwrite"] = json!(path.exists());
            if let Some(d) = &mut self.export.render {
                if let Some(stem) = path.file_stem() {
                    d.name = stem.to_string_lossy().to_string();
                }
                if let Some(folder) = path.parent() {
                    d.folder = folder.to_string_lossy().to_string();
                }
            }
            self.send_now(ctx, &mut s);
        }
        if s.weights {
            let (retimes, upscalers) = self.ai_models();
            let needs = super::upscale::needed(&retimes, &upscalers);
            let answer = if needs.is_empty() {
                Some(false)
            } else {
                super::speech::ask_download(ctx, "export-download", &needs, "Download")
            };
            if let Some(download) = answer {
                s.weights = false;
                if download {
                    s.params["download"] = json!(true);
                }
                s.ask = Some(Ask::send(&self.live, ctx, s.method(), s.params.clone()));
            }
        }
        if let Some(ask) = &s.ask
            && let Some(r) = ask.take()
        {
            match r {
                Ok(r) => match s.from {
                    From::Quick => self.export.quick = None,
                    From::Page => {
                        if let Some(id) = r.result["id"].as_str() {
                            self.export.selected = vec![id.to_string()];
                        }
                        self.export.editing = None;
                        if let Some(job) = s.update.and_then(|u| u.next)
                            && let Err(e) = self.edit_job(&job)
                            && let Some(d) = &mut self.export.render
                        {
                            d.error = Some(e);
                        }
                    }
                },
                Err(e) => match s.from {
                    From::Quick => {
                        if let Some(q) = &mut self.export.quick {
                            q.error = Some(e.message);
                        }
                    }
                    From::Page => {
                        if let Some(d) = &mut self.export.render {
                            d.error = Some(e.message);
                        }
                    }
                },
            }
            return;
        }
        self.export.starting = Some(s);
    }

    fn ai_models(&self) -> vidcut_core::editor::Models {
        self.project()
            .map(vidcut_core::editor::models_used)
            .unwrap_or_default()
    }

    fn send_start(&self, ctx: &egui::Context, s: &mut Starting) {
        s.offline = self.project().map_or_else(Vec::new, |p| {
            let clips = p.tracks.iter().flat_map(|t| &t.clips);
            p.offline_in(clips).iter().map(|m| m.name.clone()).collect()
        });
        if s.offline.is_empty() {
            self.send_checked(ctx, s);
        }
    }

    fn send_checked(&self, ctx: &egui::Context, s: &mut Starting) {
        if s.from == From::Page && s.params["overwrite"] != json!(true) {
            s.check = Some(Ask::send_dry(&self.live, ctx, s.method(), s.params.clone()));
            return;
        }
        self.send_now(ctx, s);
    }

    fn send_now(&self, ctx: &egui::Context, s: &mut Starting) {
        let (retimes, upscalers) = self.ai_models();
        if !super::upscale::needed(&retimes, &upscalers).is_empty() {
            s.weights = true;
        } else {
            s.ask = Some(Ask::send(&self.live, ctx, s.method(), s.params.clone()));
        }
    }

    pub(super) fn start_render(
        &mut self,
        ctx: &egui::Context,
        d: &mut Render,
        update: Option<Update>,
    ) {
        let mut params = d.params();
        if let Some(u) = &update {
            params["job"] = json!(u.job);
        }
        d.error = None;
        self.folders.remember(Kind::Exports, &d.output());
        let mut s = Starting {
            ask: None,
            params,
            from: From::Page,
            update,
            picking: None,
            weights: false,
            offline: Vec::new(),
            check: None,
            existing: Vec::new(),
            rename: None,
        };
        self.send_start(ctx, &mut s);
        self.export.starting = Some(s);
    }

    pub(super) fn refresh_plan(&mut self, ctx: &egui::Context, d: &mut Render) {
        if let Some(ask) = &d.plan
            && let Some(r) = ask.take()
        {
            d.plan = None;
            d.plan_result = Some(r.map(|r| r.result).map_err(|e| e.message));
        }
        if d.plan.is_some() {
            return;
        }
        let params = d.params();
        let rev = self.project().map_or(0, |p| p.rev);
        if d.planned.as_ref() == Some(&(params.clone(), rev)) {
            return;
        }
        let local = if d.folder.trim().is_empty() {
            Err("Choose a location".to_string())
        } else if !d.per_clip && d.name.trim().is_empty() {
            Err("Name the file".to_string())
        } else {
            validate(&d.settings).map_err(|e| e.message)
        };
        d.planned = Some((params.clone(), rev));
        match local {
            Err(e) => d.plan_result = Some(Err(e)),
            Ok(()) => d.plan = Some(Ask::send(&self.live, ctx, "export.plan", params)),
        }
    }

    pub(super) fn plan_ready(&self, d: &Render) -> bool {
        matches!(d.plan_result, Some(Ok(_)))
            && d.planned == Some((d.params(), self.project().map_or(0, |p| p.rev)))
            && self.export.starting.is_none()
    }

    pub(super) fn save_preset_form(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        d: &mut Render,
    ) {
        let Some(mut s) = d.save.take() else {
            return;
        };
        let mut keep = true;
        if let Some(ask) = &s.ask
            && let Some(r) = ask.take()
        {
            s.ask = None;
            match r {
                Ok(r) => {
                    if let Ok(p) = serde_json::from_value::<PresetInfo>(r.result["preset"].clone())
                    {
                        d.preset = p.name;
                        d.base = d.settings.clone();
                    }
                    self.refresh_presets(ctx);
                    keep = false;
                }
                Err(e) => s.error = Some(e.message),
            }
        }
        ui.group(|ui| {
            ui.label(RichText::new("Save As New Preset").strong());
            super::dialog::form(ui, "save-preset-form", |ui| {
                super::dialog::row(ui, "Name", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut s.name)
                            .hint_text("my-web-preset")
                            .desired_width(160.0),
                    )
                });
                super::dialog::row(ui, "Title", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut s.title)
                            .hint_text("My Web Preset")
                            .desired_width(160.0),
                    )
                });
            });
            ui.checkbox(&mut s.quick_export, "Add to Quick Export");
            ui.checkbox(&mut s.overwrite, "Replace a preset of that name");
            if let Some(e) = &s.error {
                ui.label(RichText::new(e).color(theme::colors().error));
            }
            ui.horizontal(|ui| {
                let ok = !s.name.trim().is_empty() && s.ask.is_none();
                if ui
                    .add_enabled(ok, egui::Button::new("Save Preset"))
                    .clicked()
                {
                    let settings = d.settings.clone();
                    let title = s.title.trim();
                    s.error = None;
                    s.ask = Some(Ask::send(
                        &self.live,
                        ctx,
                        "presets.save",
                        json!({
                            "name": s.name.trim(),
                            "title": if title.is_empty() { s.name.trim() } else { title },
                            "quick_export": s.quick_export,
                            "settings": settings,
                            "overwrite": s.overwrite,
                        }),
                    ));
                }
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
                if s.name.trim().is_empty() {
                    super::dialog::why(ui, "Name it first");
                }
            });
        });
        if keep {
            d.save = Some(s);
        }
    }

    pub(super) fn watch_jobs(&mut self, now: f64) {
        let mut changed = false;
        let mut failed = false;
        for j in self.snap.jobs.iter() {
            let was = self.export.jobs_seen.insert(j.id.clone(), j.state);
            if was != Some(j.state) && (was.is_none() || !j.unfinished()) {
                changed = true;
                failed |= j.state == JobState::Failed;
            }
        }
        let jobs = self.snap.jobs.clone();
        self.export
            .jobs_seen
            .retain(|id, _| jobs.iter().any(|j| &j.id == id));
        if changed {
            self.export.jobs_glow_until = now + JOBS_GLOW;
        }
        if failed && !self.export.show_jobs {
            self.export.jobs_failed_unseen = true;
        }
    }

    pub(super) fn jobs_button(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let now = ui.input(|i| i.time);
        let running = self.snap.jobs.iter().filter(|j| j.unfinished()).count();
        let label = match running {
            0 => "Jobs".to_string(),
            n => format!("Jobs ({n})"),
        };
        let label = match self.export.jobs_failed_unseen {
            true => RichText::new(label).color(theme::colors().error),
            false => RichText::new(label),
        };
        let button = ui.toggle_value(&mut self.export.show_jobs, label);
        let left = self.export.jobs_glow_until - now;
        if left > 0.0 {
            ui.painter().rect_stroke(
                button.rect.expand(2.0),
                egui::CornerRadius::same(4),
                egui::Stroke::new(
                    2.0,
                    theme::colors()
                        .accent
                        .gamma_multiply((left / JOBS_GLOW) as f32),
                ),
                egui::StrokeKind::Outside,
            );
            super::timeline::repaint_in(ui.ctx(), std::time::Duration::from_millis(33));
        }
        button
    }

    fn jobs_window(&mut self, ctx: &egui::Context) {
        if !self.export.show_jobs {
            self.export.jobs_window.close();
            return;
        }
        self.export.jobs_failed_unseen = false;
        let jobs = self.snap.jobs.clone();
        let rate = self.project().map(|p| p.fps().as_f64());
        let project = self.snap.project.as_ref().map(|p| p.path.clone());
        let mut open = true;
        let owner = self.main_window;
        let mut window = std::mem::take(&mut self.export.jobs_window);
        super::dialog::separate_window(ctx, &mut window, owner, &JOBS, &mut open, |ui| {
            if jobs.is_empty() {
                ui.label(dim("No jobs yet. Jobs from the CLI show here too."));
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for j in jobs.iter().rev() {
                    ui.group(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&j.title).strong());
                            match j.origin {
                                "ipc" => ui.label(dim("script")),
                                "session" => ui.label(dim("automatic")),
                                _ => ui.label(""),
                            };
                        });
                        job_chips(ui, j);
                        for w in job_warnings(j) {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("⚠ {w}"))
                                        .color(ui.visuals().warn_fg_color),
                                )
                                .wrap(),
                            );
                        }
                        for n in job_lines(j, "notes") {
                            ui.add(egui::Label::new(dim(n)).wrap());
                        }
                        job_view(ui, j, |method, params| self.send(method, params));
                        egui::CollapsingHeader::new("Details")
                            .id_salt(("job-details", &j.id))
                            .show(ui, |ui| job_details(ui, j, rate, project.as_deref()));
                    });
                }
            });
        });
        self.export.jobs_window = window;
        self.export.show_jobs = open;
    }

    pub(super) fn import_timeline(&mut self, ctx: &egui::Context, flatten: bool) {
        self.export.flatten = flatten;
        let (dialogs, folder) = (self.dialogs.clone(), self.export_folder());
        self.export.otio = Some(Otio::PickImport(Picked::ask(ctx, move || {
            dialogs.pick_file("Import Timeline", &folder, &["otio", "otioz"])
        })));
    }

    pub(super) fn export_timeline(&mut self, ctx: &egui::Context, bundle: bool) {
        let (dialogs, folder) = (self.dialogs.clone(), self.export_folder());
        let ext = if bundle { "otioz" } else { "otio" };
        let stem = if let Some(c) = self.project().and_then(|p| p.open_compound()) {
            c.name.replace(['/', '\\'], "-")
        } else {
            match self.project().and_then(|p| p.timeline()) {
                Some(t) => t.name.replace(['/', '\\'], "-"),
                None => self.project_stem(),
            }
        };
        let file = format!("{stem}.{ext}");
        let title = if bundle {
            "Export Timeline with Media"
        } else {
            "Export Timeline"
        };
        self.export.otio = Some(Otio::PickExport(
            Picked::ask(ctx, move || {
                dialogs.save_file(title, &folder, &file, &[ext])
            }),
            ext,
        ));
    }

    pub(super) fn show_import_warnings(&mut self, name: String, warnings: Vec<String>) {
        self.export.warnings = Some((name, warnings));
    }

    fn otio_windows(&mut self, ctx: &egui::Context) {
        if let Some((name, warnings)) = self.export.warnings.clone() {
            let mut open = true;
            let mut close = false;
            super::dialog::window(ctx, "Imported With Warnings", &mut open, |ui| {
                ui.label(format!("{name} was imported, except:"));
                for w in &warnings {
                    super::dialog::warn(ui, w.clone());
                }
                super::dialog::footer(ui, |ui| {
                    close = super::dialog::main_button(ui, "Close", true);
                });
            });
            if !open || close {
                self.export.warnings = None;
            }
        }
        let Some(state) = self.export.otio.take() else {
            return;
        };
        let import = |ws: &mut Self, path: PathBuf, mode: ImportMode, search: Option<PathBuf>| {
            if vidcut_core::editor::is_otioz(&path) {
                ws.say(format!(
                    "Unpacking the media of {}…",
                    path.file_name()
                        .map_or(String::new(), |n| n.to_string_lossy().into_owned())
                ));
            }
            let mut params = json!({ "path": path, "mode": mode });
            if let Some(dir) = search {
                params["search"] = json!([dir]);
            }
            if ws.export.flatten {
                params["flatten"] = json!(true);
            }
            if mode == ImportMode::New {
                params["name"] = json!(ws.export.import_name.trim());
                params["bin"] = json!(ws.snap.pool.bin);
            }
            Otio::Importing {
                ask: Ask::send(&ws.live, ctx, "timeline.import", params),
                path,
                mode,
            }
        };
        self.export.otio = match state {
            Otio::PickImport(p) => match p.take() {
                None => Some(Otio::PickImport(p)),
                Some(None) => None,
                Some(Some(path)) => {
                    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
                    Some(Otio::Choose(path, stem.unwrap_or_default()))
                }
            },
            Otio::Choose(path, mut name) => {
                let empty = self
                    .project()
                    .is_none_or(|p| p.tracks.iter().all(|t| t.clips.is_empty()));
                let flatten = self.export.flatten;
                let mut choice = None;
                let modal =
                    egui::Modal::new(egui::Id::unique("otio-import-choice")).show(ctx, |ui| {
                        ui.label(format!(
                            "Import {}:",
                            path.file_name()
                                .map_or(String::new(), |n| n.to_string_lossy().into_owned())
                        ));
                        ui.horizontal(|ui| {
                            ui.label("Timeline Name");
                            ui.text_edit_singleline(&mut name);
                        });
                        ui.horizontal(|ui| {
                            let named = !name.trim().is_empty();
                            if ui
                                .add_enabled(named, egui::Button::new("New Timeline"))
                                .clicked()
                            {
                                choice = Some(Some(ImportMode::New));
                            }
                            if ui.button("Append to the Timeline").clicked() {
                                choice = Some(Some(ImportMode::Append));
                            }
                            if !empty && ui.button("Replace the Timeline").clicked() {
                                choice = Some(Some(ImportMode::Replace));
                            }
                            if !flatten && ui.button("As a Compound Clip").clicked() {
                                choice = Some(Some(ImportMode::Compound));
                            }
                            if ui.button("Media Only").clicked() {
                                choice = Some(Some(ImportMode::Media));
                            }
                            if ui.button("Cancel").clicked() {
                                choice = Some(None);
                            }
                        });
                    });
                if choice.is_none() && escape_cancels(ctx, &modal) {
                    choice = Some(None);
                }
                match choice {
                    None => Some(Otio::Choose(path, name)),
                    Some(None) => None,
                    Some(Some(mode)) => {
                        self.export.import_name = name;
                        Some(import(self, path, mode, None))
                    }
                }
            }
            Otio::Missing {
                path,
                mode,
                message,
            } => {
                let mut choice = None;
                let modal =
                    egui::Modal::new(egui::Id::unique("otio-missing-media")).show(ctx, |ui| {
                        ui.set_max_width(520.0);
                        ui.label(&message);
                        ui.label(dim("Choose a folder to look in (subfolders too)."));
                        ui.horizontal(|ui| {
                            if ui.button("Choose Folder…").clicked() {
                                choice = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                choice = Some(false);
                            }
                        });
                    });
                if choice.is_none() && escape_cancels(ctx, &modal) {
                    choice = Some(false);
                }
                match choice {
                    None => Some(Otio::Missing {
                        path,
                        mode,
                        message,
                    }),
                    Some(false) => None,
                    Some(true) => {
                        let (dialogs, folder) = (self.dialogs.clone(), self.export_folder());
                        Some(Otio::PickSearch {
                            picked: Picked::ask(ctx, move || {
                                dialogs.pick_folder("Where Is the Media?", &folder)
                            }),
                            path,
                            mode,
                        })
                    }
                }
            }
            Otio::PickSearch { picked, path, mode } => match picked.take() {
                None => Some(Otio::PickSearch { picked, path, mode }),
                Some(None) => None,
                Some(Some(dir)) => Some(import(self, path, mode, Some(dir))),
            },
            Otio::Importing { ask, path, mode } => match ask.take() {
                None => Some(Otio::Importing { ask, path, mode }),
                Some(Ok(r)) => {
                    let count = |key: &str| r.result[key].as_array().map_or(0, Vec::len);
                    let warnings: Vec<String> = r.result["warnings"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|w| w.as_str().map(String::from))
                        .collect();
                    let name = r.result["name"]
                        .as_str()
                        .unwrap_or("the timeline")
                        .to_string();
                    self.say(match mode {
                        ImportMode::Compound => format!("Imported {name} as a compound clip"),
                        ImportMode::Media => {
                            format!("Imported {} media file(s) from {name}", count("media"))
                        }
                        _ => format!("Imported {} clip(s) from {name}", count("clips")),
                    });
                    if !warnings.is_empty() {
                        self.export.warnings = Some((name, warnings));
                    }
                    None
                }
                Some(Err(e)) if e.code == vidcut_core::ErrorCode::Media => Some(Otio::Missing {
                    path,
                    mode,
                    message: e.message,
                }),
                Some(Err(e)) => {
                    self.say(format!("Import failed: {}", e.message));
                    None
                }
            },
            Otio::PickExport(p, ext) => match p.take() {
                None => Some(Otio::PickExport(p, ext)),
                Some(None) => None,
                Some(Some(mut path)) => {
                    if path.extension().is_none() {
                        path.set_extension(ext);
                    }
                    self.folders.remember(Kind::Exports, &path);
                    let background = !crate::export::is_otio(&path);
                    Some(Otio::Exporting(Ask::send(
                        &self.live,
                        ctx,
                        "export.start",
                        json!({ "output": path, "overwrite": true, "background": background }),
                    )))
                }
            },
            Otio::Exporting(ask) => match ask.take() {
                None => Some(Otio::Exporting(ask)),
                Some(Ok(r)) if r.result["id"].is_string() => {
                    let id = r.result["id"].as_str().unwrap_or_default().to_string();
                    self.say(format!(
                        "Writing {} ({id})",
                        r.result["title"].as_str().unwrap_or("the bundle")
                    ));
                    None
                }
                Some(Ok(r)) => {
                    let o = &r.result["outputs"][0];
                    self.say(format!(
                        "Wrote {} ({} clips)",
                        o["path"].as_str().unwrap_or_default(),
                        o["clips"]
                    ));
                    None
                }
                Some(Err(e)) => {
                    self.say(format!("Export failed: {}", e.message));
                    None
                }
            },
        };
    }

    fn quit_window(&mut self, ctx: &egui::Context) {
        let running = self.snap.jobs.iter().filter(|j| j.unfinished()).count();
        if ctx.input(|i| i.viewport().close_requested())
            && running > 0
            && !self.export.quit_confirmed
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.export.confirm_quit = true;
        }
        if !self.export.confirm_quit {
            return;
        }
        if running == 0 {
            self.export.confirm_quit = false;
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("quit-while-exporting")).show(ctx, |ui| {
            ui.label(format!(
                "{running} job(s) still running. Quitting cancels them."
            ));
            ui.horizontal(|ui| {
                if ui.button("Keep Running").clicked() {
                    self.export.confirm_quit = false;
                }
                if ui.button("Cancel Jobs and Quit").clicked() {
                    self.export.confirm_quit = false;
                    self.export.quit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
        if self.export.confirm_quit && escape_cancels(ctx, &modal) {
            self.export.confirm_quit = false;
        }
    }
}

fn ask_offline(ctx: &egui::Context, names: &[String]) -> Option<bool> {
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique("export-offline")).show(ctx, |ui| {
        ui.set_width(super::dialog::WIDTH);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.heading("Media Offline");
        ui.add(egui::Label::new(names.join(", ")).wrap());
        super::dialog::footer(ui, |ui| {
            if super::dialog::main_button(ui, "Export Anyway", true) {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    if escape_cancels(ctx, &modal) {
        answer = Some(false);
    }
    answer
}

const SWEEP_LEG: f64 = 1.0;

fn sweep(time: f64) -> (f32, bool) {
    let phase = (time / SWEEP_LEG).rem_euclid(2.0);
    let forward = phase < 1.0;
    let t = if forward { phase } else { 2.0 - phase };
    let t = t * t * (3.0 - 2.0 * t);
    (t as f32, forward)
}

fn job_meter(j: &JobInfo) -> (Option<f32>, String) {
    if j.measuring {
        return (
            Some(j.progress.clamp(0.0, 1.0) as f32),
            format!(
                "measuring light for the HDR metadata · {:.0}%",
                j.progress * 100.0
            ),
        );
    }
    match j.fraction() {
        None => (
            None,
            format!("{} elapsed", clock(j.clock.elapsed().as_secs_f64())),
        ),
        Some(share) => {
            let text = match j.eta {
                Some(eta) => format!(
                    "{:.0}% · {} elapsed · about {} left",
                    share * 100.0,
                    clock(j.elapsed),
                    clock(eta)
                ),
                None => "starting…".into(),
            };
            (Some(share as f32), text)
        }
    }
}

fn fade(color: egui::Color32, alpha: f32) -> egui::Color32 {
    let [r, g, b, a] = color.to_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, (f32::from(a) * alpha) as u8)
}

fn sweep_span(width: f32, height: f32) -> f32 {
    let lo = height * 2.0;
    let hi = width;
    if lo <= hi {
        (width * 0.36).clamp(lo, hi)
    } else {
        hi
    }
}

fn paint_sweep(painter: &egui::Painter, bar: egui::Rect, color: egui::Color32, time: f64) {
    let span = sweep_span(bar.width(), bar.height());
    let (t, forward) = sweep(time);
    let x = bar.left() + (bar.width() - span) * t;
    let body = egui::Rect::from_min_size(egui::pos2(x, bar.top()), egui::vec2(span, bar.height()));
    let radius = bar.height() * 0.5;
    let steps = 24;
    for i in 0..steps {
        let toward_head = i as f32 / (steps - 1) as f32;
        let cover = 1.0 - toward_head;
        let w = bar.height() + (span - bar.height()) * cover;
        let left = if forward {
            body.right() - w
        } else {
            body.left()
        };
        let rect =
            egui::Rect::from_min_size(egui::pos2(left, body.top()), egui::vec2(w, body.height()));
        painter.rect_filled(rect, radius, fade(color, 0.22 + 0.78 * toward_head));
    }
}

fn job_bar(ui: &mut egui::Ui, share: Option<f32>, text: &str) {
    if let Some(share) = share {
        ui.add(
            egui::ProgressBar::new(share)
                .text(text)
                .desired_width(360.0),
        );
        return;
    }
    let height = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(360.0, height), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, ui.is_enabled(), text)
    });
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let track = ui.visuals().extreme_bg_color;
    let fill = ui.visuals().selection.bg_fill;
    let text_color = ui.visuals().text_color();
    let painter = ui.painter();
    painter.rect_filled(rect, height * 0.5, track);
    paint_sweep(painter, rect, fill, ui.input(|i| i.time));
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let pos = rect.left_center() - egui::vec2(0.0, galley.size().y * 0.5)
        + egui::vec2(ui.spacing().item_spacing.x, 0.0);
    painter.with_clip_rect(rect).galley(pos, galley, text_color);
}

pub(super) fn job_view(ui: &mut egui::Ui, j: &JobInfo, send: impl Fn(&str, Value)) {
    match j.state {
        JobState::Queued => {
            ui.label(dim("Waiting for the jobs before it"));
            if ui.button("Cancel").clicked() {
                send("jobs.cancel", json!({ "job": j.id }));
            }
        }
        JobState::Running => {
            let (share, text) = job_meter(j);
            job_bar(ui, share, &text);
            use vidcut_av::export::Stage;
            let (done, total) = (j.frames, j.total);
            ui.label(dim(match j.stage {
                Some(Stage::Downloading) => format!(
                    "downloading {} · {:.0} / {:.0} MB",
                    j.details.downloading.as_deref().unwrap_or("files"),
                    done as f64 / 1e6,
                    total as f64 / 1e6
                ),
                Some(Stage::Listening) => {
                    format!("listening · {} / {} s of sound", done / 1000, total / 1000)
                }
                Some(Stage::Loading) => "loading the model".into(),
                Some(Stage::Speakers) => {
                    format!("finding the speakers in {} s of sound", total / 1000)
                }
                Some(Stage::Uploading) => format!(
                    "uploading · {:.0} / {:.0} MB",
                    done as f64 / 1e6,
                    total as f64 / 1e6
                ),
                Some(Stage::Waiting) => "waiting for the reply".into(),
                Some(Stage::Transcribing) => {
                    format!(
                        "transcribing · {} / {} s of speech",
                        done / 1000,
                        total / 1000
                    )
                }
                None if matches!(j.kind, "subtitles" | "words" | "transcript") => {
                    "preparing…".into()
                }
                None => format!(
                    "{done} / {total} frames{}{}",
                    if j.outputs.len() > 1 {
                        format!(" · file {} of {}", j.output + 1, j.outputs.len())
                    } else {
                        String::new()
                    },
                    if j.passes > 1 {
                        format!(" · pass {} of {}", j.pass, j.passes)
                    } else {
                        String::new()
                    }
                ),
            }));
            if ui.button("Cancel").clicked() {
                send("jobs.cancel", json!({ "job": j.id }));
            }
        }
        JobState::Done => {
            ui.label(
                RichText::new(format!("Done in {}", clock(j.elapsed))).color(theme::colors().ok),
            );
            ui.horizontal(|ui| {
                if let Some(path) = job_paths(j) {
                    ui.label(dim(path.display().to_string()));
                    if ui.small_button("Open").clicked() {
                        super::file_manager::open(path);
                    }
                    if ui.small_button("Show in Folder").clicked() {
                        super::file_manager::show(path);
                    }
                }
            });
        }
        JobState::Cancelled => {
            ui.label(dim(if j.kind == "proxy" {
                "Cancelled; no proxy was recorded"
            } else {
                "Cancelled; nothing was written"
            }));
            run_again(ui, j, &send);
        }
        JobState::Failed => {
            let e = j.error.as_ref().map_or("", |e| e.message.as_str());
            ui.label(RichText::new(format!("Failed: {e}")).color(theme::colors().error));
            run_again(ui, j, &send);
        }
    }
}

fn run_again(ui: &mut egui::Ui, j: &JobInfo, send: &impl Fn(&str, Value)) {
    if let Some(method) = job_method(j)
        && ui.small_button("Run Again").clicked()
    {
        send(method, j.params.clone());
    }
}

const JOBS_GLOW: f64 = 1.5;

fn job_chips(ui: &mut egui::Ui, j: &JobInfo) {
    let d = &j.details;
    let mut chips = Vec::new();
    if let Some(v) = &d.video {
        chips.push(format!("{} · {}-bit", v.encoder, v.bits));
        if v.passes > 1 {
            chips.push(format!("{} passes", v.passes));
        }
        if v.gpu_frames_in {
            chips.push("frames on the GPU".into());
        }
    }
    if let Some(decode) = decoded_on(&d.sources) {
        chips.push(decode.into());
    }
    if !d.tried.is_empty() {
        chips.push(match d.tried.len() {
            1 => "1 encoder failed".into(),
            n => format!("{n} encoders failed"),
        });
    }
    if let Some(n) = speech_of(j) {
        chips.push(n.model.clone());
        if let Some(l) = &n.language {
            chips.push(super::speech::language_label(l));
        }
        if let Some(by) = n.speakers_by.as_deref().filter(|b| *b != "auto") {
            chips.push(format!("speakers by {}", speakers_name(by)));
        }
        if let Some(device) = &n.device {
            chips.push(format!("on {device}"));
        }
    }
    if chips.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let row = ui.max_rect().width();
        for c in chips {
            chip(ui, c, row);
        }
    });
}

fn chip(ui: &mut egui::Ui, text: String, row: f32) {
    let margin = egui::vec2(6.0, 1.0);
    let galley = egui::WidgetText::from(RichText::new(text).small()).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        row - 2.0 * margin.x,
        egui::TextStyle::Small,
    );
    let (rect, _) = ui.allocate_exact_size(galley.size() + 2.0 * margin, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(4),
        ui.visuals().faint_bg_color,
    );
    painter.galley(rect.min + margin, galley, ui.visuals().text_color());
}

fn job_details(ui: &mut egui::Ui, j: &JobInfo, rate: Option<f64>, project: Option<&Path>) {
    ui.horizontal(|ui| {
        if ui.small_button("Copy Details").clicked() {
            ui.ctx().copy_text(details_text(j, rate));
        }
        if let Some(method) = job_method(j)
            && ui.small_button("Copy as Command").clicked()
        {
            ui.ctx().copy_text(super::workspace::vidcut_command(
                project,
                method,
                &command_params(&j.params),
            ));
        }
    });
    let rows = detail_rows(j, rate);
    if rows.is_empty() {
        if j.state == JobState::Queued {
            ui.label(dim("Known once the job starts"));
        }
        return;
    }
    egui::Grid::new(("job-details-grid", &j.id))
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for (name, text) in rows {
                ui.label(dim(name));
                match name {
                    "" => ui.label(dim(text)),
                    _ => ui.label(text),
                };
                ui.end_row();
            }
        });
}

fn job_warnings(j: &JobInfo) -> Vec<String> {
    job_lines(j, "warnings")
}

fn job_lines(j: &JobInfo, field: &str) -> Vec<String> {
    j.result
        .as_ref()
        .and_then(|r| r[field].as_array())
        .into_iter()
        .flatten()
        .filter_map(|w| w.as_str().map(String::from))
        .collect()
}

fn job_method(j: &JobInfo) -> Option<&'static str> {
    match j.kind {
        "export" => Some("export.start"),
        "proxy" => Some("proxy.generate"),
        "subtitles" => Some("subtitles.transcribe"),
        "words" => Some("analyze.words"),
        "transcript" => Some("media.transcribe"),
        "retime" => Some("subtitles.retime"),
        "word_times" => Some("subtitles.time_words"),
        "sync" => Some("multicam.create"),
        "find" => Some("motion.find"),
        "track" => Some("motion.track"),
        "stabilize" => Some("picture.stabilize"),
        _ => None,
    }
}

fn command_params(params: &Value) -> Value {
    let mut params = params.clone();
    if let Some(p) = params.as_object_mut() {
        p.remove("background");
    }
    params
}

fn details_text(j: &JobInfo, rate: Option<f64>) -> String {
    let state = match j.state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Done => "done",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    };
    let mut lines = vec![
        format!("{} ({}, {state})", j.title, j.id),
        format!(
            "vidcut {}, FFmpeg {}",
            crate::system::BUILD,
            vidcut_av::ffmpeg_version()
        ),
    ];
    if let Some(e) = &j.error {
        lines.push(format!("Error: {}", e.message));
    }
    for (name, text) in detail_rows(j, rate) {
        lines.push(match name {
            "" => format!("  {text}"),
            _ => format!("{name}: {text}"),
        });
    }
    lines.join("\n")
}

fn speech_of(j: &JobInfo) -> Option<SpeechNote> {
    if let Some(n) = &j.details.speech {
        return Some(n.clone());
    }
    let r = j.result.as_ref()?;
    let r = match r["model"].is_string() {
        true => r,
        false => r["media"].get(0)?,
    };
    Some(SpeechNote {
        model: r["model"].as_str()?.to_string(),
        find: serde_json::from_value(r["find"].clone()).unwrap_or_default(),
        device: r["device"].as_str().map(String::from),
        language: r["language"].as_str().map(String::from),
        speakers_by: r["speakers_by"].as_str().map(String::from),
    })
}

fn speakers_name(by: &str) -> &str {
    match by {
        "moss" => "MOSS",
        "sortformer" => "Sortformer",
        "xai" => "xAI",
        "google" => "Google",
        by => by,
    }
}

fn speech_rows(n: &SpeechNote) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Model", n.model.clone())];
    if let Some(device) = &n.device {
        rows.push(("Device", device.clone()));
    }
    let language = n
        .language
        .as_deref()
        .map_or("Auto".into(), super::speech::language_label);
    rows.push(("Language", language));
    match n.speakers_by.as_deref() {
        Some("auto") => rows.push(("Speakers", "Automatic".into())),
        Some(by) => rows.push(("Speakers", format!("by {}", speakers_name(by)))),
        None => {}
    }
    let find = match n.find {
        FindSpeech::Voice => "Voice",
        FindSpeech::Sound => "Sound Level",
        FindSpeech::None => "None",
    };
    rows.push(("Find Speech By", find.into()));
    rows
}

fn detail_rows(j: &JobInfo, rate: Option<f64>) -> Vec<(&'static str, String)> {
    let d = &j.details;
    let mut rows = Vec::new();
    for o in &j.outputs {
        rows.push(("Writes", o.path.display().to_string()));
    }
    if let Some(n) = speech_of(j) {
        rows.extend(speech_rows(&n));
    }
    if let Some(v) = &d.video {
        let mut text = format!("{} · {}", v.encoder, v.pixel);
        if let Some(device) = &v.device {
            text += &format!(" on {device}");
        }
        if v.passes > 1 {
            text += &format!(" · {} passes", v.passes);
        }
        text += match v.gpu_frames_in {
            true => " · takes frames on the GPU",
            false => " · takes frames from memory",
        };
        rows.push(("Video", text));
    }
    if let Some(r) = &d.rounded {
        rows.push(("Size", r.message.clone()));
    }
    for t in &d.tried {
        rows.push(("Didn't open", t.text()));
    }
    if let Some(a) = &d.audio {
        let mut text = format!("{} · {} Hz · {} ch", a.encoder, a.rate, a.channels);
        if let Some(b) = a.bitrate {
            text += &format!(" · {} kb/s", b / 1000);
        }
        rows.push(("Audio", text));
        if let Some(l) = &a.loudness {
            let mut text = format!("gain {:+.1} dB", l.gain_db);
            if let Some(m) = l.measured {
                text = format!("measured {m:.1} LUFS, {text}");
            }
            if let Some(to) = l.loudness {
                text += &format!(", now {to:.1} LUFS");
            }
            rows.push(("Loudness", text));
        }
    }
    if let Some(c) = &d.color {
        rows.push(("Color", format!("{}, {}-bit", hdr_name(c.hdr), c.bit_depth)));
        rows.extend(c.why.iter().map(|why| ("", why.clone())));
    }
    if let Some(m) = &d.metadata {
        rows.push(("HDR Metadata", metadata_name(m.mode).into()));
        rows.extend(m.why.iter().map(|why| ("", why.clone())));
    }
    if let Some(decode) = decoded_on(&d.sources) {
        let files = match d.sources.len() {
            1 => "1 file".to_string(),
            n => format!("{n} files"),
        };
        rows.push(("Decoding", format!("{files}: {decode}")));
        for s in &d.sources {
            let name = s.path.file_name().map_or_else(
                || s.path.display().to_string(),
                |n| n.to_string_lossy().into(),
            );
            rows.push((
                "",
                match &s.went {
                    Some(went) => format!("{name}: {}, {went}", s.decoding),
                    None => format!("{name}: {}", s.decoding),
                },
            ));
        }
    }
    for (i, u) in d.upscaled.iter().enumerate() {
        let name = u.path.file_name().map_or_else(
            || u.path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        rows.push((
            if i == 0 { "Upscale" } else { "" },
            format!("{name}: {}", u.how),
        ));
    }
    if d.gpu_frames > 0 || d.gpu_decoded > 0 {
        rows.push((
            "On the GPU",
            format!("{} frames decoded, {} made", d.gpu_decoded, d.gpu_frames),
        ));
    }
    if d.copied.down > 0 || d.copied.up > 0 {
        rows.push((
            "Copied",
            format!(
                "{} frames down ({}), {} up ({})",
                d.copied.down,
                mib(d.copied.down_bytes),
                d.copied.up,
                mib(d.copied.up_bytes)
            ),
        ));
    }
    if let Some(fps) = d.fps {
        rows.push((
            "Speed",
            match rate {
                Some(r) if r > 0.0 => format!("{fps:.1} fps · {:.2}× real time", fps / r),
                _ => format!("{fps:.1} fps"),
            },
        ));
    }
    rows
}

fn decoded_on(sources: &[vidcut_av::export::SourceNote]) -> Option<&'static str> {
    let on_gpu = sources.iter().filter(|s| s.decoding.gpu.is_some()).count();
    match (on_gpu, sources.len()) {
        (_, 0) => None,
        (0, _) => Some("CPU decode"),
        (g, n) if g == n => Some("GPU decode"),
        _ => Some("GPU and CPU decode"),
    }
}

fn hdr_name(hdr: HdrMode) -> &'static str {
    match hdr {
        HdrMode::Auto => "automatic",
        HdrMode::Sdr => "SDR",
        HdrMode::Pq => "HDR PQ",
        HdrMode::Hlg => "HDR HLG",
    }
}

fn metadata_name(metadata: HdrMetadata) -> &'static str {
    match metadata {
        HdrMetadata::Auto => "automatic",
        HdrMetadata::Static => "mastering display and light levels",
        HdrMetadata::Hdr10plus => "static and HDR10+",
        HdrMetadata::DolbyVision => "static and Dolby Vision",
        HdrMetadata::All => "static, HDR10+ and Dolby Vision",
        HdrMetadata::None => "none",
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
}

fn bypass_line(outputs: &[Value]) -> Option<String> {
    let notes: Vec<&Value> = outputs
        .iter()
        .map(|o| &o["bypass"])
        .filter(|b| !b.is_null())
        .collect();
    if notes.is_empty() {
        return None;
    }
    let up_to: i64 = notes.iter().filter_map(|b| b["up_to"].as_i64()).sum();
    if up_to > 0 {
        return Some(format!("copies up to {up_to} frames"));
    }
    let refused = |b: &&Value| {
        let r = &b["refused"][0];
        let name = Path::new(r["path"].as_str()?)
            .file_name()?
            .to_string_lossy();
        Some(format!("{name}: {}", r["why"].as_str()?))
    };
    let why = notes
        .iter()
        .find_map(|b| b["why_not"].as_str().map(String::from))
        .or_else(|| notes.iter().find_map(refused));
    Some(match why {
        Some(why) => format!("copies nothing: {why}"),
        None => "copies nothing".into(),
    })
}

fn plan_view(ui: &mut egui::Ui, d: &Render) -> Option<Vec<String>> {
    let mut action = None;
    ui.label(dim("Plan"));
    match &d.plan_result {
        None => {
            ui.label(dim("planning…"));
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::colors().error));
        }
        Some(Ok(plan)) => {
            match &plan["video_encoder"] {
                Value::Null => {}
                v if v.get("error").is_some() => {
                    ui.label(
                        RichText::new(format!(
                            "video encoder unavailable: {}",
                            v["error"].as_str().unwrap_or_default()
                        ))
                        .color(theme::colors().error),
                    );
                }
                v => {
                    ui.label(format!(
                        "video: {} ({}){}",
                        v["name"].as_str().unwrap_or_default(),
                        v["pixel"].as_str().unwrap_or_default(),
                        v["device"]
                            .as_str()
                            .map(|d| format!(" on {d}"))
                            .unwrap_or_default()
                    ));
                }
            }
            let outputs = plan["outputs"].as_array().cloned().unwrap_or_default();
            if let Some(line) = outputs
                .iter()
                .find_map(|o| o["rounded"]["message"].as_str())
            {
                ui.label(dim(line));
            }
            if let Some(line) = color_line(&outputs) {
                ui.label(line.0);
            }
            if let Some(line) = metadata_line(&outputs) {
                ui.label(dim(line.0));
            }
            let frames: i64 = outputs.iter().filter_map(|o| o["frames"].as_i64()).sum();
            ui.label(format!("{} file(s), {frames} frames", outputs.len()));
            if let Some(line) = bypass_line(&outputs) {
                ui.label(dim(line));
            }
            for o in outputs.iter().take(4) {
                ui.label(dim(format!(
                    "{}  ({} frames)",
                    o["path"].as_str().unwrap_or_default(),
                    o["frames"]
                )));
            }
            if outputs.len() > 4 {
                ui.label(dim(format!("… and {} more", outputs.len() - 4)));
            }
            for note in plan["notes"].as_array().into_iter().flatten() {
                if note["code"] == "subtitles_cut_off" {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        note["message"].as_str().unwrap_or_default(),
                    );
                    continue;
                }
                if note["code"] == "proxy_media" {
                    ui.label(dim(note["message"].as_str().unwrap_or_default()));
                    continue;
                }
                if note["code"] != "hdr_tone_mapped" {
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(dim(note["message"].as_str().unwrap_or_default()));
                    let set: Vec<String> = note["set"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect();
                    if !set.is_empty() && ui.small_button("Export HDR").clicked() {
                        action = Some(set);
                    }
                });
            }
        }
    }
    action
}

fn color_line(outputs: &[Value]) -> Option<(String, String)> {
    let mut kinds: Vec<String> = Vec::new();
    let mut why = Vec::new();
    for c in outputs
        .iter()
        .map(|o| &o["color"])
        .filter(|c| c.is_object())
    {
        let range = match c["hdr"].as_str() {
            Some("pq") => "HDR (PQ)",
            Some("hlg") => "HDR (HLG)",
            _ => "SDR",
        };
        let kind = format!("{range}, {}-bit", c["bit_depth"]);
        if !kinds.contains(&kind) {
            kinds.push(kind);
            why.extend(
                c["why"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| w.as_str().map(String::from)),
            );
        }
    }
    (!kinds.is_empty()).then(|| (kinds.join(" / "), why.join("\n")))
}

fn metadata_line(outputs: &[Value]) -> Option<(String, String)> {
    let mut kinds: Vec<String> = Vec::new();
    let mut why = Vec::new();
    for m in outputs
        .iter()
        .map(|o| &o["metadata"])
        .filter(|m| crate::export::metadata_shown(m))
    {
        let words = crate::export::metadata_words(m);
        if !kinds.contains(&words) {
            kinds.push(words);
            why.extend(
                m["why"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| w.as_str().map(String::from)),
            );
        }
    }
    (!kinds.is_empty()).then(|| (kinds.join(" / "), why.join("\n")))
}

fn keep_hdr(d: &mut Render, set: &[String]) {
    let mut settings = d.settings.clone();
    if apply_overrides(&mut settings, set).is_ok() {
        d.settings = settings;
        fit_codecs(&mut d.settings);
    }
}

fn passes_row(ui: &mut egui::Ui, v: &mut VideoSettings, planned: &Value, auto: String) {
    use vidcut_core::export::{FirstPass, Multipass};
    let label = |p: Option<u8>| match p {
        None => auto.clone(),
        Some(n) => n.to_string(),
    };
    ui.label(dim("passes"));
    egui::ComboBox::from_id_salt("export-passes")
        .selected_text(label(v.passes))
        .show_ui(ui, |ui| {
            for p in [None, Some(1), Some(2)] {
                ui.selectable_value(&mut v.passes, p, label(p));
            }
        });
    match planned["passes"]["kind"].as_str() {
        Some("two_runs") => {
            ui.label(dim("first pass"));
            egui::ComboBox::from_id_salt("export-first-pass")
                .selected_text(name(&v.first_pass))
                .show_ui(ui, |ui| {
                    for f in [FirstPass::Fast, FirstPass::Full] {
                        ui.selectable_value(&mut v.first_pass, f, name(&f));
                    }
                });
        }
        Some("two_inside") => {
            ui.label(dim("first look"));
            egui::ComboBox::from_id_salt("export-multipass")
                .selected_text(name(&v.multipass))
                .show_ui(ui, |ui| {
                    for m in [Multipass::Quarter, Multipass::Full] {
                        ui.selectable_value(&mut v.multipass, m, name(&m));
                    }
                });
        }
        _ => {}
    }
    if planned["name"]
        .as_str()
        .is_some_and(|n| n.ends_with("_nvenc"))
    {
        ui.label(dim("lookahead"));
        let mut n = v
            .lookahead
            .unwrap_or(vidcut_core::export::DEFAULT_LOOKAHEAD);
        let max = vidcut_core::export::MAX_LOOKAHEAD;
        if ui
            .add(
                egui::DragValue::new(&mut n)
                    .range(0..=max)
                    .suffix(" frames"),
            )
            .changed()
        {
            v.lookahead = (n != vidcut_core::export::DEFAULT_LOOKAHEAD).then_some(n);
        }
    }
}

fn auto_label(picked: &Value, setting: &str, show: impl Fn(&Value) -> Option<String>) -> String {
    let mut picks: Vec<String> = Vec::new();
    for p in picked[setting]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(show)
    {
        if !picks.contains(&p) {
            picks.push(p);
        }
    }
    match picks.is_empty() {
        true => "auto".to_string(),
        false => format!("auto ({})", picks.join(" / ")),
    }
}

fn why_video(can: &Value, codec: VideoCodec, encoder: EncoderChoice) -> Option<String> {
    can["video"][name(&codec)][name(&encoder)]
        .as_str()
        .map(String::from)
}

fn why_audio(can: &Value, codec: AudioCodec) -> Option<String> {
    can["audio"][name(&codec)].as_str().map(String::from)
}

fn choice<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    text: String,
    why: Option<String>,
) -> egui::Response {
    let text = match &why {
        Some(why) => format!("{text} · {why}"),
        None => text,
    };
    ui.add_enabled_ui(why.is_none(), |ui| {
        ui.selectable_value(current, value, text)
    })
    .inner
}

fn subtitles_form(ui: &mut egui::Ui, d: &mut Render, tracks: &[(String, String, bool)]) {
    let mut on = d.subtitles.is_some();
    if ui.checkbox(&mut on, "Export subtitles").changed() {
        d.subtitles = on.then_some(SubtitlesAs::File);
        if on && d.subtitle_tracks.is_empty() {
            let shown = tracks.iter().find(|t| t.2).or(tracks.first());
            d.subtitle_tracks = shown.map(|t| t.0.clone()).into_iter().collect();
        }
    }
    let Some(mut how) = d.subtitles else {
        return;
    };
    egui::Grid::new("export-subtitles")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("As");
            ui.horizontal(|ui| {
                choice(
                    ui,
                    &mut how,
                    SubtitlesAs::File,
                    "Separate files".into(),
                    None,
                );
                let stream_why = d.settings.container.subtitle_codec().is_none().then(|| {
                    format!(
                        ".{} files hold no subtitle streams",
                        d.settings.container.extension()
                    )
                });
                choice(
                    ui,
                    &mut how,
                    SubtitlesAs::Stream,
                    "Embedded in the file".into(),
                    stream_why,
                );
                let burn_why = d
                    .settings
                    .video
                    .is_none()
                    .then(|| "an export without video has no picture to burn them into".into());
                choice(
                    ui,
                    &mut how,
                    SubtitlesAs::Burn,
                    "Burned into the video".into(),
                    burn_why,
                );
            });
            ui.end_row();
            if how == SubtitlesAs::File {
                ui.label("Files");
                ui.horizontal(|ui| {
                    for (format, name) in [
                        (SubtitleFormat::Srt, "SRT"),
                        (SubtitleFormat::Vtt, "WebVTT"),
                        (SubtitleFormat::Ttml, "TTML (IMSC1)"),
                    ] {
                        ui.selectable_value(&mut d.subtitle_format, format, name);
                    }
                });
                ui.end_row();
            }
            ui.label(if how == SubtitlesAs::Burn {
                "Track"
            } else {
                "Tracks"
            });
            ui.horizontal_wrapped(|ui| {
                let mut chosen = d.subtitle_tracks.clone();
                for (id, name, _) in tracks {
                    if how == SubtitlesAs::Burn {
                        let on = chosen.first() == Some(id);
                        if ui.radio(on, name).clicked() {
                            chosen = vec![id.clone()];
                        }
                    } else {
                        let mut on = chosen.contains(id);
                        if ui.checkbox(&mut on, name).changed() {
                            if on {
                                chosen.push(id.clone());
                            } else {
                                chosen.retain(|c| c != id);
                            }
                        }
                    }
                }
                if how == SubtitlesAs::Burn {
                    chosen.truncate(1);
                }
                d.subtitle_tracks = tracks
                    .iter()
                    .filter(|t| chosen.contains(&t.0))
                    .map(|t| t.0.clone())
                    .collect();
            });
            ui.end_row();
        });
    d.subtitles = Some(how);
}

fn new_video(c: Container) -> VideoSettings {
    VideoSettings {
        codec: video_codecs(c)[0],
        encoder: EncoderChoice::Auto,
        quality: Some(20),
        bitrate: None,
        speed: None,
        profile: None,
        bit_depth: None,
        width: None,
        height: None,
        hdr: HdrMode::Auto,
        passes: None,
        first_pass: Default::default(),
        multipass: Default::default(),
        lookahead: None,
        hdr_metadata: Default::default(),
        mastering_display: None,
        max_cll: None,
        alpha: false,
        alpha_mode: Default::default(),
        filename_digits: None,
        first_frame: None,
        compression_level: None,
        bypass_reencode: false,
        use_proxy_media: false,
    }
}

fn format_row(ui: &mut egui::Ui, d: &mut Render) {
    super::dialog::row(ui, "Export Video", |ui| {
        let mut on = d.settings.video.is_some();
        let sequence = d.settings.container.image_sequence();
        let can_choose = !d.settings.container.audio_only() && !sequence;
        let r = ui.add_enabled(can_choose, egui::Checkbox::without_text(&mut on));
        if !can_choose {
            super::dialog::why(
                ui,
                format!(
                    "{} holds only {}",
                    d.settings.container.extension().to_uppercase(),
                    if sequence { "frames" } else { "sound" },
                ),
            );
        }
        if r.changed() {
            if on {
                d.settings.video = d
                    .stashed_video
                    .take()
                    .or(d.base.video.clone())
                    .or(Some(new_video(d.settings.container)));
                fit_codecs(&mut d.settings);
            } else {
                d.stashed_video = d.settings.video.take();
            }
        }
    });
    super::dialog::row(ui, "Format", |ui| {
        let before = d.settings.container;
        egui::ComboBox::from_id_salt("export-container")
            .selected_text(d.settings.container.extension())
            .show_ui(ui, |ui| {
                for c in CONTAINERS {
                    ui.selectable_value(&mut d.settings.container, c, c.extension());
                }
            });
        if d.settings.container != before {
            if d.settings.container.audio_only() && d.settings.video.is_some() {
                d.stashed_video = d.settings.video.take();
            } else if !d.settings.container.audio_only()
                && (before.audio_only() || d.settings.container.image_sequence())
                && d.settings.video.is_none()
            {
                let c = d.settings.container;
                d.settings.video = (d.stashed_video.take())
                    .or(d.base.video.clone())
                    .or(c.image_sequence().then(|| new_video(c)));
            }
            if before.image_sequence() && d.settings.audio.is_none() {
                d.settings.audio = d.base.audio.clone();
            }
            fit_codecs(&mut d.settings);
        }
    });
}

pub(super) fn video_tab(ui: &mut egui::Ui, d: &mut Render, can: &Value, size: (u32, u32)) {
    let plan = d.plan_result.as_ref().and_then(|r| r.as_ref().ok());
    let planned = plan.map(|p| p["video_encoder"].clone()).unwrap_or_default();
    let picked = plan.map(|p| p["auto"].clone()).unwrap_or_default();
    let mut drops: Vec<&str> = Vec::new();
    for o in plan
        .and_then(|p| p["outputs"].as_array())
        .into_iter()
        .flatten()
    {
        for line in crate::export::graded_drops(&o["metadata"]) {
            if !drops.contains(&line) {
                drops.push(line);
            }
        }
    }
    super::dialog::form(ui, "export-video", |ui| {
        format_row(ui, d);
        let container = d.settings.container;
        let Some(v) = &mut d.settings.video else {
            return;
        };
        super::dialog::row(ui, "Codec", |ui| {
            egui::ComboBox::from_id_salt("export-vcodec")
                .selected_text(name(&v.codec))
                .show_ui(ui, |ui| {
                    for c in video_codecs(container) {
                        let why = why_video(can, *c, v.encoder);
                        if choice(ui, &mut v.codec, *c, name(c), why).changed() {
                            v.profile = None;
                            if fixed_format(*c) {
                                v.bit_depth = None;
                            }
                        }
                    }
                });
        });
        super::dialog::row(ui, "Encoder", |ui| {
            let auto = auto_label(&picked, "encoder", |e| e.as_str().map(String::from));
            let encoder = |e: &EncoderChoice| match e {
                EncoderChoice::Auto => auto.clone(),
                _ => name(e),
            };
            egui::ComboBox::from_id_salt("export-encoder")
                .selected_text(encoder(&v.encoder))
                .show_ui(ui, |ui| {
                    for &e in [EncoderChoice::Auto, EncoderChoice::Cpu]
                        .iter()
                        .chain(GPU_ENCODERS)
                    {
                        let why = why_video(can, v.codec, e);
                        choice(ui, &mut v.encoder, e, encoder(&e), why);
                    }
                });
        });
        super::dialog::row(ui, "Rate Control", |ui| {
            let mut by_quality = v.bitrate.is_none();
            let bitrate_only = vidcut_av::export::encode::bitrate_only(v.codec, v.encoder);
            let quality = ui
                .add_enabled_ui(!bitrate_only, |ui| {
                    ui.radio_value(&mut by_quality, true, "Quality")
                })
                .inner;
            if quality.changed() {
                v.bitrate = None;
                v.quality = v.quality.or(Some(20));
                v.passes = None;
            }
            if ui.radio_value(&mut by_quality, false, "Bitrate").changed() {
                v.bitrate = Some(20_000_000);
                v.quality = None;
            }
            if bitrate_only {
                super::dialog::why(ui, format!("{} takes only a bitrate", v.encoder.title()));
            }
        });
        if v.bitrate.is_none() {
            super::dialog::row(ui, "Quality", |ui| {
                let mut q = v.quality.unwrap_or(20);
                if ui
                    .add(egui::DragValue::new(&mut q).range(0..=v.codec.worst_quality()))
                    .changed()
                {
                    v.quality = Some(q);
                }
            });
        } else {
            super::dialog::row(ui, "Bitrate", |ui| {
                let mut mbps = v.bitrate.unwrap_or(20_000_000) as f64 / 1e6;
                if ui
                    .add(
                        egui::DragValue::new(&mut mbps)
                            .range(0.1..=500.0)
                            .speed(0.1)
                            .suffix(" Mb/s"),
                    )
                    .changed()
                {
                    v.bitrate = Some((mbps * 1e6).round() as u64);
                }
            });
        }
        if v.bitrate.is_some() && !fixed_format(v.codec) {
            let auto = auto_label(&picked, "passes", |p| {
                p["kind"].as_str().map(|k| match k {
                    "one" => "1".to_string(),
                    _ => "2".to_string(),
                })
            });
            super::dialog::row(ui, "Passes", |ui| passes_row(ui, v, &planned, auto));
        }
        super::dialog::row(ui, "Resolution", |ui| {
            let mut custom = v.width.is_some();
            if ui
                .radio_value(
                    &mut custom,
                    false,
                    format!("Timeline ({}×{})", size.0, size.1),
                )
                .changed()
            {
                v.width = None;
                v.height = None;
            }
            if ui.radio_value(&mut custom, true, "Custom").changed() {
                v.width = Some(size.0);
                v.height = Some(size.1);
            }
        });
        if let (Some(w), Some(h)) = (&mut v.width, &mut v.height) {
            super::dialog::row(ui, "", |ui| {
                let (low, high) = vidcut_core::model::TIMELINE_SIDE;
                ui.add(egui::DragValue::new(w).range(low..=high));
                ui.label("×");
                ui.add(egui::DragValue::new(h).range(low..=high));
            });
        }
        let options = profiles(v.codec);
        if !options.is_empty() {
            super::dialog::row(ui, "Profile", |ui| {
                let usual = match planned["profile"].as_str() {
                    Some(p) => format!("default ({p})"),
                    None => "default".to_string(),
                };
                egui::ComboBox::from_id_salt("export-profile")
                    .selected_text(v.profile.clone().unwrap_or(usual.clone()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut v.profile, None, usual);
                        for p in options {
                            ui.selectable_value(&mut v.profile, Some(p.to_string()), *p);
                        }
                    });
            });
        }
        if v.dwa() {
            super::dialog::row(ui, "Compression Level", |ui| {
                use vidcut_core::export::{DWA_LEVEL, DWA_LEVELS};
                let mut level = v.compression_level.unwrap_or(DWA_LEVEL);
                if ui
                    .add(egui::DragValue::new(&mut level).range(DWA_LEVELS))
                    .changed()
                {
                    v.compression_level = Some(level);
                }
            });
        } else {
            v.compression_level = None;
        }
        if !fixed_format(v.codec) {
            super::dialog::row(ui, "Bit Depth", |ui| {
                let auto = auto_label(&picked, "bit_depth", |b| b.as_u64().map(|b| b.to_string()));
                egui::ComboBox::from_id_salt("export-depth")
                    .selected_text(v.bit_depth.map_or(auto.clone(), |b| b.to_string()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut v.bit_depth, None, auto);
                        ui.selectable_value(&mut v.bit_depth, Some(8), "8");
                        ui.selectable_value(&mut v.bit_depth, Some(10), "10");
                    });
            });
        }
        if v.holds_alpha() {
            super::dialog::row(ui, "Export Alpha", |ui| {
                ui.add(egui::Checkbox::without_text(&mut v.alpha));
                if v.alpha {
                    egui::ComboBox::from_id_salt("export-alpha-mode")
                        .selected_text(v.alpha_mode.title())
                        .show_ui(ui, |ui| {
                            for &m in v.alpha_modes() {
                                ui.selectable_value(&mut v.alpha_mode, m, m.title());
                            }
                        });
                }
            });
        } else {
            v.alpha = false;
        }
    });
    let Some(v) = &mut d.settings.video else {
        return;
    };
    egui::CollapsingHeader::new("Advanced Settings")
        .id_salt("export-video-advanced")
        .default_open(false)
        .show_unindented(ui, |ui| {
            super::dialog::form(ui, "export-video-advanced-form", |ui| {
                video_advanced(ui, v, &picked, &planned, &drops)
            });
        });
}

fn video_advanced(
    ui: &mut egui::Ui,
    v: &mut VideoSettings,
    picked: &Value,
    planned: &Value,
    drops: &[&str],
) {
    if vidcut_core::export::color::carries_hdr(v.codec) {
        let range = |h: &str| match h {
            "pq" => "HDR (PQ)",
            "hlg" => "HDR (HLG)",
            _ => "SDR",
        };
        let auto = auto_label(picked, "hdr", |h| h.as_str().map(|h| range(h).to_string()));
        let label = |h: HdrMode| match h {
            HdrMode::Auto => auto.clone(),
            HdrMode::Sdr => "SDR".to_string(),
            HdrMode::Pq => "HDR (PQ)".to_string(),
            HdrMode::Hlg => "HDR (HLG)".to_string(),
        };
        super::dialog::row(ui, "Dynamic Range", |ui| {
            egui::ComboBox::from_id_salt("export-hdr")
                .selected_text(label(v.hdr))
                .show_ui(ui, |ui| {
                    for h in [HdrMode::Auto, HdrMode::Sdr, HdrMode::Pq, HdrMode::Hlg] {
                        ui.selectable_value(&mut v.hdr, h, label(h));
                    }
                });
        });
        if v.hdr.transfer().is_some() && v.bit_depth == Some(8) {
            v.bit_depth = None;
        }
        if v.hdr != HdrMode::Sdr && v.image_bits().is_none() {
            use vidcut_core::export::HdrMetadata as M;
            let kind = |m: M| match m {
                M::Auto => "auto",
                M::Static => "HDR10 only",
                M::Hdr10plus => "HDR10 + HDR10+",
                M::DolbyVision => "HDR10 + Dolby Vision",
                M::All => "HDR10 + HDR10+ + DV",
                M::None => "none",
            };
            let auto = auto_label(picked, "metadata", |m| {
                serde_json::from_value::<M>(m.clone())
                    .ok()
                    .map(|m| kind(m).to_string())
            });
            let label = |m: M| match m {
                M::Auto => auto.clone(),
                m => kind(m).to_string(),
            };
            super::dialog::row(ui, "HDR Metadata", |ui| {
                egui::ComboBox::from_id_salt("export-hdr-metadata")
                    .selected_text(label(v.hdr_metadata))
                    .show_ui(ui, |ui| {
                        for m in [
                            M::Auto,
                            M::Static,
                            M::Hdr10plus,
                            M::DolbyVision,
                            M::All,
                            M::None,
                        ] {
                            ui.selectable_value(&mut v.hdr_metadata, m, label(m));
                        }
                    });
            });
            for line in drops {
                super::dialog::row(ui, "", |ui| ui.label(dim(*line)));
            }
        }
    } else if v.hdr.transfer().is_some() {
        v.hdr = HdrMode::Auto;
    }
    super::dialog::row(ui, "Speed", |ui| {
        let mut speed = v.speed.clone().unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::singleline(&mut speed)
                    .hint_text(planned["speed"].as_str().unwrap_or("default"))
                    .desired_width(70.0),
            )
            .changed()
        {
            let s = speed.trim();
            v.speed = (!s.is_empty()).then(|| s.to_string());
        }
    });
    if v.image_bits().is_none() {
        super::dialog::row(ui, "", |ui| {
            ui.checkbox(&mut v.bypass_reencode, "Bypass Re-encode When Possible")
        });
    } else {
        v.bypass_reencode = false;
    }
    super::dialog::row(ui, "", |ui| {
        ui.checkbox(&mut v.use_proxy_media, "Use Proxy Media")
    });
}

pub(super) fn audio_tab(ui: &mut egui::Ui, d: &mut Render, can: &Value, audio: (u32, u16)) {
    super::dialog::form(ui, "export-audio", |ui| {
        super::dialog::row(ui, "Export Audio", |ui| {
            let mut on = d.settings.audio.is_some();
            let can_hold = !audio_codecs(d.settings.container).is_empty();
            let r = ui.add_enabled(can_hold, egui::Checkbox::without_text(&mut on));
            if !can_hold {
                super::dialog::why(ui, "Frames hold no sound");
            }
            if r.changed() {
                d.settings.audio = on.then(|| AudioSettings {
                    codec: audio_codecs(d.settings.container)[0],
                    bitrate: None,
                    sample_rate: None,
                    channels: None,
                    loudness: None,
                });
                fit_codecs(&mut d.settings);
            }
        });
        let container = d.settings.container;
        let Some(a) = &mut d.settings.audio else {
            return;
        };
        super::dialog::row(ui, "Codec", |ui| {
            egui::ComboBox::from_id_salt("export-acodec")
                .selected_text(name(&a.codec))
                .show_ui(ui, |ui| {
                    for c in audio_codecs(container) {
                        let why = why_audio(can, *c);
                        if choice(ui, &mut a.codec, *c, name(c), why).changed() {
                            a.bitrate = lossy(a.codec).then_some(usual_bitrate(a.codec));
                        }
                    }
                });
        });
        if lossy(a.codec) {
            super::dialog::row(ui, "Bit Rate", |ui| {
                let mut kbps = a.bitrate.unwrap_or(usual_bitrate(a.codec)) / 1000;
                let range = match a.codec {
                    AudioCodec::Aac => AAC_BITRATES.start() / 1000..=AAC_BITRATES.end() / 1000,
                    _ => 32..=512,
                };
                if ui
                    .add(egui::DragValue::new(&mut kbps).range(range).suffix(" kb/s"))
                    .changed()
                {
                    a.bitrate = Some(kbps * 1000);
                }
            });
        }
        super::dialog::row(ui, "Sample Rate", |ui| {
            egui::ComboBox::from_id_salt("export-rate")
                .selected_text(
                    a.sample_rate
                        .map_or(format!("{} Hz", audio.0), |r| format!("{r} Hz")),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut a.sample_rate,
                        None,
                        format!("project ({} Hz)", audio.0),
                    );
                    let rates: &[u32] = match a.codec {
                        AudioCodec::Aac => &AAC_RATES,
                        _ => &[44_100, 48_000, 96_000],
                    };
                    for &r in rates {
                        ui.selectable_value(&mut a.sample_rate, Some(r), format!("{r} Hz"));
                    }
                });
        });
        super::dialog::row(ui, "Channels", |ui| {
            let channels = |c: u16| match c {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                6 => "5.1".to_string(),
                n => format!("{n} ch"),
            };
            egui::ComboBox::from_id_salt("export-channels")
                .selected_text(a.channels.map_or(channels(audio.1), channels))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut a.channels,
                        None,
                        format!("project ({})", channels(audio.1)),
                    );
                    for c in 1..=vidcut_core::model::Settings::MOST_CHANNELS {
                        ui.selectable_value(&mut a.channels, Some(c), channels(c));
                    }
                });
        });
        super::dialog::row(ui, "Loudness", |ui| {
            let mut on = a.loudness.is_some();
            if ui.checkbox(&mut on, "Normalize to").changed() {
                a.loudness = on.then_some(-14.0);
            }
            if let Some(target) = &mut a.loudness {
                ui.add(
                    egui::DragValue::new(target)
                        .range(vidcut_core::ops::normalize::LOUDNESS)
                        .speed(0.1)
                        .suffix(" LUFS"),
                );
            }
        });
    });
}

pub(super) const CLIP_NAME: &str = "{source}";

fn clip_name_rows(ui: &mut egui::Ui, d: &mut Render) {
    super::dialog::row(ui, "Filename Uses", |ui| {
        super::dialog::switch(ui, |ui| {
            ui.selectable_value(&mut d.source_names, false, "Custom Name");
            ui.selectable_value(&mut d.source_names, true, "Source Name");
        })
    });
    if !d.source_names {
        super::dialog::row(ui, "Custom Name", |ui| {
            ui.add(egui::TextEdit::singleline(&mut d.template).desired_width(220.0))
        });
        super::dialog::row(ui, "File Suffix", |ui| {
            ui.add(egui::TextEdit::singleline(&mut d.suffix).desired_width(220.0))
        });
    }
    super::dialog::row(ui, "Unique Names", |ui| {
        let mut on = d.unique.is_some();
        if ui.add(egui::Checkbox::without_text(&mut on)).changed() {
            d.unique = on.then_some(UniqueAs::Prefix);
        }
    });
    if let Some(how) = &mut d.unique {
        super::dialog::row(ui, "Unique Name As", |ui| {
            super::dialog::switch(ui, |ui| {
                ui.selectable_value(how, UniqueAs::Prefix, "Prefix");
                ui.selectable_value(how, UniqueAs::Suffix, "Suffix");
            })
        });
    }
    if !d.source_names {
        super::dialog::row(ui, "Source Frame Count", |ui| {
            ui.add(egui::Checkbox::without_text(&mut d.frame_count))
        });
    }
    super::dialog::row(ui, "File Subfolder", |ui| {
        ui.add(egui::TextEdit::singleline(&mut d.subfolder).desired_width(220.0))
    });
}

fn clip_folder_rows(ui: &mut egui::Ui, d: &mut Render) {
    super::dialog::row(ui, "Separate Folders", |ui| {
        ui.add(egui::Checkbox::without_text(&mut d.separate_folders))
    });
    super::dialog::row(ui, "Source Folder Levels", |ui| {
        ui.add(egui::DragValue::new(&mut d.source_levels).range(0..=99))
    });
    if d.source_levels > 0 {
        super::dialog::row(ui, "Count Levels", |ui| {
            super::dialog::switch(ui, |ui| {
                ui.selectable_value(&mut d.levels_from, LevelsFrom::Head, "After Head");
                ui.selectable_value(&mut d.levels_from, LevelsFrom::Tail, "From Tail");
            })
        });
    }
}

pub(super) fn file_tab(ui: &mut egui::Ui, d: &mut Render) {
    super::dialog::form(ui, "export-file", |ui| {
        if d.per_clip {
            clip_name_rows(ui, d);
        }
        let per_clip = d.per_clip;
        if let Some(v) = d
            .settings
            .video
            .as_mut()
            .filter(|v| per_clip || v.image_bits().is_some())
        {
            use vidcut_core::export::{FILENAME_DIGIT_COUNTS, FILENAME_DIGITS};
            super::dialog::row(ui, "Filename Digits", |ui| {
                let mut digits = v.filename_digits.unwrap_or(FILENAME_DIGITS);
                if ui
                    .add(egui::DragValue::new(&mut digits).range(FILENAME_DIGIT_COUNTS))
                    .changed()
                {
                    v.filename_digits = Some(digits);
                }
            });
        }
        if let Some(v) = d
            .settings
            .video
            .as_mut()
            .filter(|v| v.image_bits().is_some())
        {
            super::dialog::row(ui, "First Frame", |ui| {
                let mut own = v.first_frame.is_some();
                if ui.add(egui::Checkbox::without_text(&mut own)).changed() {
                    v.first_frame = own.then_some(1);
                }
                if let Some(first) = &mut v.first_frame {
                    ui.add(egui::DragValue::new(first));
                }
            });
        }
        super::dialog::row(ui, "Replace Files", |ui| {
            ui.add(egui::Checkbox::without_text(&mut d.overwrite))
        });
        if d.per_clip {
            clip_folder_rows(ui, d);
        }
        super::dialog::row(ui, "Render Speed", |ui| {
            render_speed(ui, &mut d.settings.render_speed)
        });
    });
}

const RENDER_SPEEDS: [u32; 8] = [1, 2, 5, 10, 25, 50, 75, 100];

fn render_speed(ui: &mut egui::Ui, speed: &mut Option<std::num::NonZeroU32>) {
    let label =
        |s: Option<std::num::NonZeroU32>| s.map_or("Maximum".into(), |n| format!("{n} fps"));
    egui::ComboBox::from_id_salt("export-render-speed")
        .selected_text(label(*speed))
        .show_ui(ui, |ui| {
            let fixed = RENDER_SPEEDS.into_iter().map(std::num::NonZeroU32::new);
            for s in fixed.filter(Option::is_some).chain([None]) {
                ui.selectable_value(speed, s, label(s));
            }
        });
}

pub(super) fn subtitle_settings(
    ui: &mut egui::Ui,
    d: &mut Render,
    tracks: &[(String, String, bool)],
) {
    if tracks.is_empty() {
        return;
    }
    egui::CollapsingHeader::new("Subtitle Settings")
        .id_salt("export-subtitle-settings")
        .default_open(d.subtitles.is_some())
        .show_unindented(ui, |ui| subtitles_form(ui, d, tracks));
}

pub(super) fn plan_and_command(ui: &mut egui::Ui, d: &mut Render, project: Option<&Path>) {
    if let Some(set) = plan_view(ui, d) {
        keep_hdr(d, &set);
    }
    let Some(project) = project else {
        return;
    };
    let cmd = command(project, &d.params());
    egui::CollapsingHeader::new("Command")
        .id_salt("export-command")
        .default_open(false)
        .show_unindented(ui, |ui| {
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(cmd.clone());
            }
            ui.add(egui::Label::new(RichText::new(&cmd).monospace().small()).wrap());
        });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn a_job_bar_taller_than_it_is_wide_still_sweeps() {
        assert_eq!(super::sweep_span(10.0, 8.0), 10.0);
    }

    use super::{auto_label, command_params, decoded_on, detail_rows, details_text, metadata_line};
    use crate::live::JobInfo;

    fn job() -> JobInfo {
        JobInfo::new("j1".into(), "gui", json!({ "output": "out.mp4" }))
    }

    fn speech(language: Option<&str>, speakers_by: Option<&str>) -> vidcut_av::export::SpeechNote {
        vidcut_av::export::SpeechNote {
            model: "Parakeet Ultra".into(),
            find: Default::default(),
            device: Some("CPU".into()),
            language: language.map(String::from),
            speakers_by: speakers_by.map(String::from),
        }
    }

    #[test]
    fn a_transcription_says_what_it_uses() {
        let mut j = job();
        j.details.speech = Some(speech(Some("en"), Some("moss")));
        assert_eq!(
            detail_rows(&j, None),
            [
                ("Model", "Parakeet Ultra".to_string()),
                ("Device", "CPU".to_string()),
                ("Language", "English (en)".to_string()),
                ("Speakers", "by MOSS".to_string()),
                ("Find Speech By", "Sound Level".to_string()),
            ]
        );
    }

    #[test]
    fn a_language_not_yet_known_reads_auto() {
        let mut j = job();
        j.details.speech = Some(speech(None, None));
        assert!(detail_rows(&j, None).contains(&("Language", "Auto".to_string())));
    }

    #[test]
    fn an_automatic_setting_names_what_the_plan_picked() {
        let picked = json!({ "bit_depth": [10] });
        let depth = |b: &serde_json::Value| b.as_u64().map(|b| b.to_string());
        assert_eq!(auto_label(&picked, "bit_depth", depth), "auto (10)");
    }

    #[test]
    fn a_job_whose_encoder_fell_back_says_why() {
        let mut j = job();
        j.details.tried.push(vidcut_av::export::Tried {
            encoder: Some("h264_nvenc".into()),
            error: "no free session".into(),
        });
        assert_eq!(
            detail_rows(&j, None),
            [("Didn't open", "h264_nvenc: no free session".to_string())]
        );
    }

    #[test]
    fn copied_details_start_with_the_job() {
        assert_eq!(
            details_text(&job(), None).lines().next(),
            Some("out.mp4 (j1, running)")
        );
    }

    #[test]
    fn a_copied_command_waits_for_its_export() {
        assert_eq!(
            command_params(&json!({ "output": "a.mp4", "background": true })),
            json!({ "output": "a.mp4" })
        );
    }

    #[test]
    fn files_decoded_on_both_say_so() {
        let file = |gpu: Option<&str>| vidcut_av::export::SourceNote {
            path: "a.mp4".into(),
            decoding: vidcut_av::decode::Decoding {
                gpu: gpu.map(String::from),
                ..Default::default()
            },
            went: None,
        };
        assert_eq!(
            decoded_on(&[file(Some("NVDEC")), file(None)]),
            Some("GPU and CPU decode")
        );
    }

    #[test]
    fn the_metadata_line_names_each_kind_once() {
        let file = |mode: &str, cll: u32| {
            json!({ "metadata": {
                "mode": mode,
                "light": { "max_cll": cll, "max_fall": 400 },
                "why": [format!("MaxCLL {cll}")],
            }})
        };
        let outputs = [
            file("static", 1000),
            file("static", 1000),
            file("hdr10plus", 2000),
            json!({ "metadata": { "mode": "none", "why": [] } }),
        ];
        let (words, why) = metadata_line(&outputs).unwrap();
        assert_eq!(
            words,
            "HDR10: MaxCLL 1000, MaxFALL 400 / HDR10: MaxCLL 2000, MaxFALL 400; HDR10+"
        );
        assert_eq!(why, "MaxCLL 1000\nMaxCLL 2000");
        assert!(metadata_line(&outputs[3..]).is_none());
    }
}
