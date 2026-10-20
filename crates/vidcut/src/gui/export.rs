use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_core::export::{
    AudioCodec, AudioSettings, Container, EncoderChoice, ExportSettings, HdrMetadata, HdrMode,
    VideoCodec, VideoSettings, apply_overrides, validate,
};
use vidcut_core::request::{ImportMode, SubtitleFormat, SubtitlesAs};
use vidcut_core::{Envelope, Error, Response};

use super::folders::Kind;
use super::theme;
use super::workspace::{Workspace, dim, escape_cancels};
use crate::live::{self, JobInfo, JobState, Origin};

#[derive(Debug)]
pub(super) struct Ask {
    slot: Arc<Mutex<Option<vidcut_core::Result<Response>>>>,
}

impl Ask {
    pub(super) fn send(
        live: &live::Handle,
        ctx: &egui::Context,
        method: &str,
        params: Value,
    ) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let (out, live, ctx) = (slot.clone(), live.clone(), ctx.clone());
        let envelope = Envelope::from_json(json!({ "method": method, "params": params }));
        let _ = crate::threads::spawn("vidcut-ask", crate::threads::Work::Interactive, move || {
            let result = envelope
                .map_err(Error::internal)
                .and_then(|e| live.call(e, Origin::Gui { drag: false }, None, &mut |_| true));
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            ctx.request_repaint();
        });
        Self { slot }
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

#[derive(Clone)]
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
    #[serde(default)]
    pub user: bool,
    pub settings: ExportSettings,
    #[serde(default)]
    pub unavailable: Option<String>,
}

#[derive(Default)]
pub struct ExportUi {
    presets: Vec<PresetInfo>,
    presets_ask: Option<Ask>,
    encodable: Value,
    encodable_ask: Option<Ask>,
    quick: Option<Quick>,
    dialog: Option<Box<Dialog>>,
    dialog_wanted: bool,
    starting: Option<Starting>,
    pub show_jobs: bool,
    jobs_seen: std::collections::HashMap<String, JobState>,
    jobs_glow_until: f64,
    jobs_failed_unseen: bool,
    otio: Option<Otio>,
    flatten: bool,
    warnings: Option<(String, Vec<String>)>,
    confirm_quit: bool,
    pub quit_confirmed: bool,
}

impl ExportUi {
    pub(super) fn windows(&self) -> (bool, bool) {
        (
            self.quick.is_some(),
            self.dialog.is_some() || self.dialog_wanted,
        )
    }

    pub(super) fn close_quick(&mut self) {
        self.quick = None;
    }

    pub(super) fn close_dialog(&mut self) {
        self.dialog = None;
        self.dialog_wanted = false;
    }

    pub(super) fn asking(&self, jobs: &[JobInfo]) -> bool {
        matches!(self.otio, Some(Otio::Choose(_) | Otio::Missing { .. }))
            || (self.confirm_quit && jobs.iter().any(JobInfo::unfinished))
    }
}

struct Quick {
    selected: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum From {
    Quick,
    Dialog,
}

struct Starting {
    params: Value,
    from: From,
    picking: Option<Picked>,
    ask: Option<Ask>,
}

enum Otio {
    PickImport(Picked),
    Choose(PathBuf),
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
enum RangeChoice {
    All,
    InOut,
    Custom,
}

struct Dialog {
    preset: String,
    base: ExportSettings,
    settings: ExportSettings,
    stashed_video: Option<VideoSettings>,
    range: RangeChoice,
    from: String,
    to: String,
    per_clip: bool,
    template: String,
    output: String,
    overwrite: bool,
    picking: Option<Picked>,
    plan: Option<Ask>,
    planned: Option<Value>,
    plan_result: Option<std::result::Result<Value, String>>,
    error: Option<String>,
    save: Option<SavePreset>,
    subtitles: Option<SubtitlesAs>,
    subtitle_tracks: Vec<String>,
    subtitle_format: SubtitleFormat,
}

struct SavePreset {
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
    }
}

const CONTAINERS: [Container; 9] = [
    Container::Mp4,
    Container::Mov,
    Container::Mkv,
    Container::Webm,
    Container::Wav,
    Container::Flac,
    Container::Mp3,
    Container::M4a,
    Container::Ogg,
];

fn profiles(c: VideoCodec) -> &'static [&'static str] {
    match c {
        VideoCodec::Prores => &["proxy", "lt", "standard", "hq", "4444"],
        VideoCodec::Dnxhr => &["lb", "sq", "hq", "hqx", "444"],
        VideoCodec::H264 => &["baseline", "main", "high"],
        VideoCodec::Hevc => &["main", "main10"],
        VideoCodec::Av1 | VideoCodec::Vp9 => &[],
    }
}

fn name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn lossy(a: AudioCodec) -> bool {
    matches!(a, AudioCodec::Aac | AudioCodec::Opus | AudioCodec::Mp3)
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
        }
    }
    if let Some(a) = &mut s.audio {
        let ok = audio_codecs(s.container);
        if !ok.contains(&a.codec) {
            a.codec = ok[0];
            a.bitrate = lossy(a.codec).then_some(a.bitrate.unwrap_or(256_000));
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

fn clock(secs: f64) -> String {
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

impl Dialog {
    fn new(preset: &PresetInfo, output: String, range: RangeChoice) -> Self {
        Self {
            preset: preset.name.clone(),
            base: preset.settings.clone(),
            settings: preset.settings.clone(),
            stashed_video: None,
            range,
            from: String::new(),
            to: String::new(),
            per_clip: false,
            template: "{index:03}-{name}".into(),
            output,
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
        }
    }

    fn use_preset(&mut self, preset: &PresetInfo) {
        self.preset = preset.name.clone();
        self.base = preset.settings.clone();
        self.settings = preset.settings.clone();
        self.stashed_video = None;
        self.match_output();
    }

    fn match_output(&mut self) {
        if self.per_clip || self.output.trim().is_empty() {
            return;
        }
        let path = PathBuf::from(self.output.trim());
        self.output = path
            .with_extension(self.settings.container.extension())
            .display()
            .to_string();
    }

    fn params(&self) -> Value {
        let output = self.output.trim();
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
        let set = overrides(&base, &settings);
        if !set.is_empty() {
            p["set"] = json!(set);
        }
        p["range"] = json!(match self.range {
            RangeChoice::All => "start..end".to_string(),
            RangeChoice::InOut => "in..out".to_string(),
            RangeChoice::Custom => format!("{}..{}", self.from.trim(), self.to.trim()),
        });
        if self.per_clip {
            p["per_clip"] = json!(true);
            p["name"] = json!(self.template);
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

impl Workspace {
    pub(super) fn open_quick_export(&mut self, ctx: &egui::Context) {
        self.refresh_presets(ctx);
        self.export.quick = Some(Quick {
            selected: None,
            error: None,
        });
    }

    pub(super) fn open_export(&mut self, ctx: &egui::Context) {
        self.refresh_presets(ctx);
        self.export.dialog = None;
        self.export.quick = None;
        self.export.starting = None;
        self.export.dialog_wanted = true;
    }

    fn refresh_presets(&mut self, ctx: &egui::Context) {
        self.export.presets_ask = Some(Ask::send(&self.live, ctx, "presets.list", json!({})));
        self.export.encodable_ask = Some(Ask::send(&self.live, ctx, "encoders.list", json!({})));
    }

    fn export_folder(&self) -> PathBuf {
        self.folders
            .get(Kind::Exports)
            .or_else(|| self.project_folder())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn project_stem(&self) -> String {
        self.snap
            .project
            .as_ref()
            .and_then(|p| p.path.file_stem())
            .map_or("export".into(), |s| s.to_string_lossy().into_owned())
    }

    fn animated_track(&self) -> Option<(String, String)> {
        let p = self.project()?;
        let t = p.subtitles.iter().find(|t| t.shown)?;
        let words = t.style.words.is_some() && t.cues.iter().any(|c| !c.words.is_empty());
        words.then(|| (t.id.clone(), t.display_name()))
    }

    fn marks_set(&self) -> bool {
        self.project()
            .is_some_and(|p| p.marks.mark_in.is_some() && p.marks.mark_out.is_some())
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
        if self.export.dialog_wanted && !self.export.presets.is_empty() {
            self.export.dialog_wanted = false;
            let preset = self
                .export
                .presets
                .iter()
                .find(|p| p.name == "h265-master")
                .unwrap_or(&self.export.presets[0])
                .clone();
            let output = self
                .export_folder()
                .join(format!(
                    "{}.{}",
                    self.project_stem(),
                    preset.settings.container.extension()
                ))
                .display()
                .to_string();
            let range = if self.marks_set() {
                RangeChoice::InOut
            } else {
                RangeChoice::All
            };
            let mut dialog = Dialog::new(&preset, output, range);
            if let Some((track, _)) = self.animated_track() {
                dialog.subtitles = Some(SubtitlesAs::Burn);
                dialog.subtitle_tracks = vec![track];
            }
            self.export.dialog = Some(Box::new(dialog));
        }
        self.quick_window(ctx);
        self.dialog_window(ctx);
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
        let presets: Vec<PresetInfo> = self
            .export
            .presets
            .iter()
            .filter(|p| p.quick_export)
            .cloned()
            .collect();
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
        egui::Window::new("Quick Export")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 340.0, 320.0))
            .open(&mut open)
            .show(ctx, |ui| {
                if presets.is_empty() {
                    ui.label(dim("Loading presets…"));
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for p in &presets {
                        let selected = q.selected.as_deref() == Some(p.name.as_str());
                        let size = p
                            .settings
                            .video
                            .as_ref()
                            .and_then(|v| v.height.map(|h| format!("{h}p")))
                            .unwrap_or_else(|| "timeline size".into());
                        let text = format!(
                            "{}\n{} · {}",
                            p.title,
                            p.settings.container.extension().to_uppercase(),
                            size
                        );
                        let r = ui
                            .add_enabled_ui(p.unavailable.is_none(), |ui| {
                                ui.add_sized(
                                    [150.0, 56.0],
                                    egui::Button::selectable(selected, text),
                                )
                            })
                            .inner
                            .on_hover_text(&p.description)
                            .on_disabled_hover_text(p.unavailable.as_deref().unwrap_or_default());
                        if r.clicked() {
                            q.selected = Some(p.name.clone());
                            q.error = None;
                        }
                    }
                });
                ui.separator();
                if let Some(p) = presets
                    .iter()
                    .find(|p| Some(&p.name) == q.selected.as_ref())
                {
                    ui.label(&p.description);
                }
                ui.label(dim(&range));
                if let Some((_, name)) = &animated {
                    ui.label(dim(format!(
                        "Burns in {name}: its words show as they are spoken"
                    )));
                }
                if let Some(e) = &q.error {
                    ui.label(RichText::new(e).color(theme::colors().error));
                }
                let busy = self.export.starting.is_some();
                if ui
                    .add_enabled(
                        !busy
                            && presets.iter().any(|p| {
                                Some(&p.name) == q.selected.as_ref() && p.unavailable.is_none()
                            }),
                        egui::Button::new("Export…"),
                    )
                    .clicked()
                {
                    export = true;
                }
            });
        if export && let Some(name) = q.selected.clone() {
            let ext = presets
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
                picking: Some(picking),
                ask: None,
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
                    s.ask = Some(Ask::send(&self.live, ctx, "export.start", s.params.clone()));
                }
            }
        }
        if let Some(ask) = &s.ask
            && let Some(r) = ask.take()
        {
            match r {
                Ok(_) => match s.from {
                    From::Quick => self.export.quick = None,
                    From::Dialog => self.export.dialog = None,
                },
                Err(e) => match s.from {
                    From::Quick => {
                        if let Some(q) = &mut self.export.quick {
                            q.error = Some(e.message);
                        }
                    }
                    From::Dialog => {
                        if let Some(d) = &mut self.export.dialog {
                            d.error = Some(e.message);
                        }
                    }
                },
            }
            return;
        }
        self.export.starting = Some(s);
    }

    fn dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.export.dialog.take() else {
            return;
        };
        let presets = self.export.presets.clone();
        let project = self.snap.project.as_ref().map(|p| p.path.clone());
        let (width, height) = self
            .project()
            .map_or((1920, 1080), |p| (p.settings.width, p.settings.height));
        let (rate, channels) = self.project().map_or((48_000, 2), |p| {
            (p.settings.sample_rate, p.settings.channels)
        });
        let marks = self.marks_set();
        let in_out = self.project().map(|p| p.marks.clone()).unwrap_or_default();
        let subtitle_tracks: Vec<(String, String, bool)> =
            self.project().map_or_else(Vec::new, |p| {
                p.subtitles
                    .iter()
                    .map(|t| (t.id.clone(), t.display_name(), t.shown))
                    .collect()
            });
        if let Some(p) = &d.picking
            && let Some(choice) = p.take()
        {
            d.picking = None;
            if let Some(path) = choice {
                d.output = path.display().to_string();
                d.match_output();
                d.overwrite = !d.per_clip && path.exists();
            }
        }
        let mut open = true;
        let mut start = false;
        let mut browse = false;
        egui::Window::new("Export")
            .collapsible(false)
            .resizable(true)
            .default_width(600.0)
            .default_pos(near_center(ctx, 600.0, 640.0))
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(ctx.content_rect().height() * 0.7)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let label = ui.label("Preset");
                            let title = presets
                                .iter()
                                .find(|p| p.name == d.preset)
                                .map_or(d.preset.clone(), |p| p.title.clone());
                            let edited = d.settings != d.base;
                            let shown = if edited {
                                format!("{title} (changed)")
                            } else {
                                title
                            };
                            egui::ComboBox::from_id_salt("export-preset")
                                .selected_text(shown)
                                .width(240.0)
                                .show_ui(ui, |ui| {
                                    for p in &presets {
                                        let mark = if p.user { " (yours)" } else { "" };
                                        let r = ui
                                            .add_enabled_ui(p.unavailable.is_none(), |ui| {
                                                ui.selectable_label(
                                                    p.name == d.preset,
                                                    format!("{}{mark}", p.title),
                                                )
                                            })
                                            .inner
                                            .on_hover_text(&p.description)
                                            .on_disabled_hover_text(
                                                p.unavailable.as_deref().unwrap_or_default(),
                                            );
                                        if r.clicked() {
                                            d.use_preset(p);
                                        }
                                    }
                                })
                                .response
                                .labelled_by(label.id);
                            if ui.button("Save as Preset…").clicked() {
                                d.save = Some(SavePreset {
                                    name: String::new(),
                                    title: String::new(),
                                    quick_export: false,
                                    overwrite: false,
                                    ask: None,
                                    error: None,
                                });
                            }
                            let user = presets.iter().any(|p| p.name == d.preset && p.user);
                            if user && ui.button("Delete Preset").clicked() {
                                self.send("presets.delete", json!({ "name": d.preset }));
                                self.export.presets_ask =
                                    Some(Ask::send(&self.live, ctx, "presets.list", json!({})));
                            }
                        });
                        self.save_preset_form(ui, ctx, &mut d);
                        ui.separator();
                        let can = &self.export.encodable;
                        settings_form(ui, &mut d, can, (width, height), (rate, channels));
                        ui.separator();
                        if !subtitle_tracks.is_empty() {
                            subtitles_form(ui, &mut d, &subtitle_tracks);
                            ui.separator();
                        }
                        ui.horizontal(|ui| {
                            ui.label("Range");
                            ui.radio_value(&mut d.range, RangeChoice::All, "Whole timeline");
                            let label = match (in_out.mark_in, in_out.mark_out) {
                                (Some(a), Some(b)) => {
                                    format!("In–Out ({} – {})", self.tc(a), self.tc(b))
                                }
                                _ => "In–Out (not set)".into(),
                            };
                            ui.add_enabled_ui(marks, |ui| {
                                ui.radio_value(&mut d.range, RangeChoice::InOut, label);
                            });
                            ui.radio_value(&mut d.range, RangeChoice::Custom, "Custom");
                        });
                        if d.range == RangeChoice::Custom {
                            ui.horizontal(|ui| {
                                ui.label(dim("from"));
                                ui.add(
                                    egui::TextEdit::singleline(&mut d.from)
                                        .hint_text("00:00:00:00")
                                        .desired_width(110.0),
                                );
                                ui.label(dim("to"));
                                ui.add(
                                    egui::TextEdit::singleline(&mut d.to)
                                        .hint_text("00:01:00:00")
                                        .desired_width(110.0),
                                );
                                ui.label(dim("(to is excluded; 15s, 450f also work)"));
                            });
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("Output");
                            let before = d.per_clip;
                            ui.radio_value(&mut d.per_clip, false, "One file");
                            ui.radio_value(&mut d.per_clip, true, "One file per clip");
                            if d.per_clip != before {
                                let path = PathBuf::from(d.output.trim());
                                d.output = if d.per_clip {
                                    path.with_extension("").display().to_string()
                                } else {
                                    path.with_extension(d.settings.container.extension())
                                        .display()
                                        .to_string()
                                };
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(if d.per_clip { "Folder" } else { "File" });
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut d.output).desired_width(380.0),
                            );
                            if r.lost_focus()
                                && !d.per_clip
                                && let Some(c) = Path::new(d.output.trim())
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .and_then(Container::from_extension)
                                && c != d.settings.container
                            {
                                d.settings.container = c;
                                fit_codecs(&mut d.settings);
                            }
                            if ui
                                .add_enabled(d.picking.is_none(), egui::Button::new("Browse…"))
                                .clicked()
                            {
                                browse = true;
                            }
                        });
                        if d.per_clip {
                            ui.horizontal(|ui| {
                                ui.label("Names");
                                ui.add(
                                    egui::TextEdit::singleline(&mut d.template)
                                        .desired_width(200.0),
                                );
                                ui.label(dim("{index}, {index:03}, {name}, {clip}, {media}"));
                            });
                        }
                        ui.checkbox(&mut d.overwrite, "Replace existing files");
                        ui.separator();
                        if let Some(set) = plan_view(ui, &d) {
                            keep_hdr(&mut d, &set);
                        }
                        ui.separator();
                        let params = d.params();
                        if let Some(project) = &project {
                            let cmd = command(project, &params);
                            ui.horizontal(|ui| {
                                ui.label(dim("Command"));
                                if ui.small_button("Copy").clicked() {
                                    ui.ctx().copy_text(cmd.clone());
                                }
                            });
                            ui.add(
                                egui::Label::new(RichText::new(&cmd).monospace().small()).wrap(),
                            );
                        }
                    });
                ui.separator();
                if let Some(e) = &d.error {
                    ui.label(RichText::new(e).color(theme::colors().error));
                }
                ui.horizontal(|ui| {
                    let ready = matches!(d.plan_result, Some(Ok(_)))
                        && d.planned.as_ref() == Some(&d.params())
                        && self.export.starting.is_none();
                    if ui
                        .add_enabled(ready, egui::Button::new("Start Export"))
                        .clicked()
                    {
                        start = true;
                    }
                });
            });
        if browse {
            let (dialogs, folder) = (self.dialogs.clone(), self.export_folder());
            let per_clip = d.per_clip;
            let ext = d.settings.container.extension();
            let file = PathBuf::from(d.output.trim())
                .file_name()
                .map_or(format!("{}.{ext}", self.project_stem()), |n| {
                    n.to_string_lossy().into_owned()
                });
            d.picking = Some(Picked::ask(ctx, move || {
                if per_clip {
                    dialogs.pick_folder("Export Clips To", &folder)
                } else {
                    dialogs.save_file("Export", &folder, &file, &[ext])
                }
            }));
        }
        self.refresh_plan(ctx, &mut d);
        if start {
            let mut params = d.params();
            params["background"] = json!(true);
            d.error = None;
            self.folders
                .remember(Kind::Exports, Path::new(d.output.trim()));
            self.export.starting = Some(Starting {
                ask: Some(Ask::send(&self.live, ctx, "export.start", params.clone())),
                params,
                from: From::Dialog,
                picking: None,
            });
        }
        if open {
            self.export.dialog = Some(d);
        }
    }

    fn refresh_plan(&mut self, ctx: &egui::Context, d: &mut Dialog) {
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
        if d.planned.as_ref() == Some(&params) {
            return;
        }
        let mut settings = d.settings.clone();
        if !d.per_clip
            && let Some(c) = Path::new(d.output.trim())
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Container::from_extension)
        {
            settings.container = c;
            fit_codecs(&mut settings);
        }
        let local = if d.output.trim().is_empty() {
            Err("choose where to export to".to_string())
        } else {
            validate(&settings).map_err(|e| e.message)
        };
        d.planned = Some(params.clone());
        match local {
            Err(e) => d.plan_result = Some(Err(e)),
            Ok(()) => d.plan = Some(Ask::send(&self.live, ctx, "export.plan", params)),
        }
    }

    fn save_preset_form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, d: &mut Dialog) {
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
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(
                    egui::TextEdit::singleline(&mut s.name)
                        .hint_text("my-web-preset")
                        .desired_width(150.0),
                );
                ui.label("Title");
                ui.add(
                    egui::TextEdit::singleline(&mut s.title)
                        .hint_text("My Web Preset")
                        .desired_width(150.0),
                );
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.quick_export, "Add to Quick Export");
                ui.checkbox(&mut s.overwrite, "Replace a preset of that name");
            });
            if let Some(e) = &s.error {
                ui.label(RichText::new(e).color(theme::colors().error));
            }
            ui.horizontal(|ui| {
                let ok = !s.name.trim().is_empty() && s.ask.is_none();
                if ui
                    .add_enabled(ok, egui::Button::new("Save Preset"))
                    .clicked()
                {
                    let mut settings = d.settings.clone();
                    if !d.per_clip
                        && let Some(c) = Path::new(d.output.trim())
                            .extension()
                            .and_then(|e| e.to_str())
                            .and_then(Container::from_extension)
                    {
                        settings.container = c;
                    }
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
            });
        });
        if keep {
            d.save = Some(s);
        }
    }

    pub(super) fn jobs_button(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
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
        let running = jobs.iter().filter(|j| j.unfinished()).count();
        let label = match running {
            0 => "Jobs".to_string(),
            n => format!("Jobs ({n})"),
        };
        let label = match self.export.jobs_failed_unseen {
            true => RichText::new(label).color(theme::colors().error),
            false => RichText::new(label),
        };
        let button = ui
            .toggle_value(&mut self.export.show_jobs, label)
            .on_hover_text("Exports, proxy encodes and transcriptions, with progress and details");
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
    }

    fn jobs_window(&mut self, ctx: &egui::Context) {
        if !self.export.show_jobs {
            return;
        }
        self.export.jobs_failed_unseen = false;
        let jobs = self.snap.jobs.clone();
        let rate = self.project().map(|p| p.settings.fps.as_f64());
        let project = self.snap.project.as_ref().map(|p| p.path.clone());
        let mut open = true;
        egui::Window::new("Jobs")
            .default_width(460.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if jobs.is_empty() {
                    ui.label(dim(
                        "No exports or proxy encodes yet. Those from vidcut CLI commands show here too.",
                    ));
                }
                egui::ScrollArea::vertical()
                    .max_height(520.0)
                    .show(ui, |ui| {
                        for j in jobs.iter().rev() {
                            ui.group(|ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&j.title).strong());
                                    let from = match j.origin {
                                        "session" => "automatic".to_string(),
                                        o => format!("from {o}"),
                                    };
                                    ui.label(dim(format!("{} · {from}", j.id)));
                                });
                                job_chips(ui, j);
                                job_view(ui, j, |method, params| self.send(method, params));
                                egui::CollapsingHeader::new("Details")
                                    .id_salt(("job-details", &j.id))
                                    .show(ui, |ui| job_details(ui, j, rate, project.as_deref()));
                            });
                        }
                    });
            });
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
        let file = format!("{}.{ext}", self.project_stem());
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
            egui::Window::new("Imported With Warnings")
                .collapsible(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(format!("{name} was imported, except:"));
                    for w in &warnings {
                        ui.label(format!("• {w}"));
                    }
                });
            if !open {
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
                Some(Some(path)) => Some(Otio::Choose(path)),
            },
            Otio::Choose(path) => {
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
                    None => Some(Otio::Choose(path)),
                    Some(None) => None,
                    Some(Some(mode)) => Some(import(self, path, mode, None)),
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

fn job_view(ui: &mut egui::Ui, j: &JobInfo, send: impl Fn(&str, Value)) {
    match j.state {
        JobState::Queued => {
            ui.label(dim("Waiting for the jobs before it"));
            if ui.button("Cancel").clicked() {
                send("jobs.cancel", json!({ "job": j.id }));
            }
        }
        JobState::Running => {
            let text = match j.eta {
                _ if j.measuring => format!(
                    "measuring light for the HDR metadata · {:.0}%",
                    j.progress * 100.0
                ),
                Some(eta) => format!(
                    "{:.0}% · {} elapsed · about {} left",
                    j.progress * 100.0,
                    clock(j.elapsed),
                    clock(eta)
                ),
                None => "starting…".into(),
            };
            ui.add(
                egui::ProgressBar::new(j.progress as f32)
                    .text(text)
                    .desired_width(360.0),
            );
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
        && ui
            .small_button("Run Again")
            .on_hover_text("Start the same job again")
            .clicked()
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
    if chips.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for c in chips {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(6, 1))
                .show(ui, |ui| ui.label(RichText::new(c).small()));
        }
    });
}

fn job_details(ui: &mut egui::Ui, j: &JobInfo, rate: Option<f64>, project: Option<&Path>) {
    ui.horizontal(|ui| {
        if ui
            .small_button("Copy Details")
            .on_hover_text("These details as text, for a bug report")
            .clicked()
        {
            ui.ctx().copy_text(details_text(j, rate));
        }
        if let Some(method) = job_method(j)
            && ui
                .small_button("Copy as Command")
                .on_hover_text("A vidcut command that runs this job again from a terminal")
                .clicked()
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
        ui.label(dim("Known once the job starts"));
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

fn job_method(j: &JobInfo) -> Option<&'static str> {
    match j.kind {
        "export" => Some("export.start"),
        "proxy" => Some("proxy.generate"),
        "subtitles" => Some("subtitles.transcribe"),
        "words" => Some("analyze.words"),
        "transcript" => Some("media.transcribe"),
        "retime" => Some("subtitles.retime"),
        "word_times" => Some("subtitles.time_words"),
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
            env!("CARGO_PKG_VERSION"),
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

fn detail_rows(j: &JobInfo, rate: Option<f64>) -> Vec<(&'static str, String)> {
    let d = &j.details;
    let mut rows = Vec::new();
    for o in &j.outputs {
        rows.push(("Writes", o.path.display().to_string()));
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
    for t in &d.tried {
        let text = match &t.encoder {
            Some(e) => format!("{e}: {}", t.error),
            None => t.error.clone(),
        };
        rows.push(("Didn't open", text));
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
        rows.push(("HDR metadata", metadata_name(m.mode).into()));
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
            rows.push(("", format!("{name}: {}", s.decoding)));
        }
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

fn plan_view(ui: &mut egui::Ui, d: &Dialog) -> Option<Vec<String>> {
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
            if let Some(line) = color_line(&outputs) {
                ui.label(line.0).on_hover_text(line.1);
            }
            if let Some(line) = metadata_line(&outputs) {
                ui.label(dim(line.0)).on_hover_text(line.1);
            }
            let frames: i64 = outputs.iter().filter_map(|o| o["frames"].as_i64()).sum();
            ui.label(format!("{} file(s), {frames} frames", outputs.len()));
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
                    if !set.is_empty()
                        && ui
                            .small_button("Export HDR")
                            .on_hover_text(set.join("  "))
                            .clicked()
                    {
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
        .filter(|m| m.is_object() && m["mode"].as_str() != Some("none"))
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

fn keep_hdr(d: &mut Dialog, set: &[String]) {
    let mut settings = d.settings.clone();
    if apply_overrides(&mut settings, set).is_ok() {
        d.settings = settings;
        fit_codecs(&mut d.settings);
    }
}

fn passes_row(ui: &mut egui::Ui, v: &mut VideoSettings, planned: &Value) {
    use vidcut_core::export::{FirstPass, Multipass};
    let label = |p: Option<u8>| match p {
        None => "auto".to_string(),
        Some(n) => n.to_string(),
    };
    ui.label(dim("passes"));
    egui::ComboBox::from_id_salt("export-passes")
        .selected_text(label(v.passes))
        .show_ui(ui, |ui| {
            for p in [None, Some(1), Some(2)] {
                ui.selectable_value(&mut v.passes, p, label(p));
            }
        })
        .response
        .on_hover_text(
            "auto: two where the encoder makes them. Two reach the bitrate more closely: \
             x264, x265 and VP9 encode twice, NVENC looks at each frame twice.",
        );
    match planned["passes"]["kind"].as_str() {
        Some("two_runs") => {
            ui.label(dim("first pass"));
            egui::ComboBox::from_id_salt("export-first-pass")
                .selected_text(name(&v.first_pass))
                .show_ui(ui, |ui| {
                    for f in [FirstPass::Fast, FirstPass::Full] {
                        ui.selectable_value(&mut v.first_pass, f, name(&f));
                    }
                })
                .response
                .on_hover_text("fast: quicker settings for the measuring run");
        }
        Some("two_inside") => {
            ui.label(dim("first look"));
            egui::ComboBox::from_id_salt("export-multipass")
                .selected_text(name(&v.multipass))
                .show_ui(ui, |ui| {
                    for m in [Multipass::Quarter, Multipass::Full] {
                        ui.selectable_value(&mut v.multipass, m, name(&m));
                    }
                })
                .response
                .on_hover_text(
                    "NVENC's first pass over each frame: at a quarter or full resolution",
                );
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
            .on_hover_text("frames NVENC looks ahead for rate control; 0: none")
            .changed()
        {
            v.lookahead = (n != vidcut_core::export::DEFAULT_LOOKAHEAD).then_some(n);
        }
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
    let r = ui
        .add_enabled_ui(why.is_none(), |ui| {
            ui.selectable_value(current, value, text)
        })
        .inner;
    match why {
        Some(why) => r.on_disabled_hover_text(why),
        None => r,
    }
}

fn subtitles_form(ui: &mut egui::Ui, d: &mut Dialog, tracks: &[(String, String, bool)]) {
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

fn settings_form(
    ui: &mut egui::Ui,
    d: &mut Dialog,
    can: &Value,
    size: (u32, u32),
    audio: (u32, u16),
) {
    let planned = d
        .plan_result
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|p| p["video_encoder"].clone())
        .unwrap_or_default();
    egui::Grid::new("export-settings")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Format");
            ui.horizontal(|ui| {
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
                        && before.audio_only()
                        && d.settings.video.is_none()
                    {
                        d.settings.video = d.stashed_video.take().or(d.base.video.clone());
                    }
                    fit_codecs(&mut d.settings);
                    d.match_output();
                }
                let mut audio_only = d.settings.video.is_none();
                let can_choose = !d.settings.container.audio_only();
                if ui
                    .add_enabled(
                        can_choose,
                        egui::Checkbox::new(&mut audio_only, "Audio only"),
                    )
                    .changed()
                {
                    if audio_only {
                        d.stashed_video = d.settings.video.take();
                    } else {
                        d.settings.video = d.stashed_video.take().or(d.base.video.clone()).or(
                            Some(VideoSettings {
                                codec: video_codecs(d.settings.container)[0],
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
                            }),
                        );
                        fit_codecs(&mut d.settings);
                    }
                }
            });
            ui.end_row();

            if let Some(v) = &mut d.settings.video {
                ui.label("Video");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("export-vcodec")
                        .selected_text(name(&v.codec))
                        .show_ui(ui, |ui| {
                            for c in video_codecs(d.settings.container) {
                                let why = why_video(can, *c, v.encoder);
                                if choice(ui, &mut v.codec, *c, name(c), why).changed() {
                                    v.profile = None;
                                    if matches!(c, VideoCodec::Prores | VideoCodec::Dnxhr) {
                                        v.bit_depth = None;
                                    }
                                }
                            }
                        });
                    ui.label(dim("encoder"));
                    egui::ComboBox::from_id_salt("export-encoder")
                        .selected_text(name(&v.encoder))
                        .show_ui(ui, |ui| {
                            let gpus: &[EncoderChoice] = if cfg!(windows) {
                                &[EncoderChoice::Nvenc, EncoderChoice::Amf, EncoderChoice::Qsv]
                            } else if cfg!(target_os = "macos") {
                                &[EncoderChoice::VideoToolbox]
                            } else {
                                &[
                                    EncoderChoice::Nvenc,
                                    EncoderChoice::Vaapi,
                                    EncoderChoice::Qsv,
                                ]
                            };
                            for &e in [EncoderChoice::Auto, EncoderChoice::Cpu].iter().chain(gpus) {
                                let why = why_video(can, v.codec, e);
                                choice(ui, &mut v.encoder, e, name(&e), why);
                            }
                        });
                });
                ui.end_row();

                ui.label("");
                ui.horizontal(|ui| {
                    let mut by_quality = v.bitrate.is_none();
                    let bitrate_only = vidcut_av::export::encode::bitrate_only(v.codec, v.encoder);
                    let quality = ui
                        .add_enabled_ui(!bitrate_only, |ui| {
                            ui.radio_value(&mut by_quality, true, "Quality")
                        })
                        .inner
                        .on_disabled_hover_text(format!(
                            "{} encodes {} only at a bitrate",
                            v.encoder.title(),
                            v.codec.title()
                        ));
                    if quality.changed() {
                        v.bitrate = None;
                        v.quality = v.quality.or(Some(20));
                        v.passes = None;
                    }
                    if ui.radio_value(&mut by_quality, false, "Bitrate").changed() {
                        v.bitrate = Some(20_000_000);
                        v.quality = None;
                    }
                    if by_quality {
                        let mut q = v.quality.unwrap_or(20);
                        if ui
                            .add(egui::DragValue::new(&mut q).range(0..=v.codec.worst_quality()))
                            .on_hover_text("lower is better (CRF / CQ)")
                            .changed()
                        {
                            v.quality = Some(q);
                        }
                    } else {
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
                    }
                });
                ui.end_row();

                if v.bitrate.is_some() && !matches!(v.codec, VideoCodec::Prores | VideoCodec::Dnxhr)
                {
                    ui.label("");
                    ui.horizontal(|ui| passes_row(ui, v, &planned));
                    ui.end_row();
                }

                ui.label("");
                ui.horizontal(|ui| {
                    let mut custom = v.width.is_some();
                    if ui
                        .radio_value(
                            &mut custom,
                            false,
                            format!("Timeline size ({}×{})", size.0, size.1),
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
                    if let (Some(w), Some(h)) = (&mut v.width, &mut v.height) {
                        ui.add(egui::DragValue::new(w).range(16..=8192).speed(2));
                        ui.label("×");
                        ui.add(egui::DragValue::new(h).range(16..=8192).speed(2));
                        *w &= !1;
                        *h &= !1;
                    }
                });
                ui.end_row();

                ui.label("");
                ui.horizontal(|ui| {
                    let options = profiles(v.codec);
                    if !options.is_empty() {
                        ui.label(dim("profile"));
                        egui::ComboBox::from_id_salt("export-profile")
                            .selected_text(v.profile.as_deref().unwrap_or("default"))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut v.profile, None, "default");
                                for p in options {
                                    ui.selectable_value(&mut v.profile, Some(p.to_string()), *p);
                                }
                            });
                    }
                    if !matches!(v.codec, VideoCodec::Prores | VideoCodec::Dnxhr) {
                        ui.label(dim("bit depth"));
                        egui::ComboBox::from_id_salt("export-depth")
                            .selected_text(v.bit_depth.map_or("auto".into(), |b| b.to_string()))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut v.bit_depth, None, "auto");
                                ui.selectable_value(&mut v.bit_depth, Some(8), "8");
                                ui.selectable_value(&mut v.bit_depth, Some(10), "10");
                            })
                            .response
                            .on_hover_text(
                                "auto: 10 bits for HDR, and when most of the picture has 10 \
                                 bits or more (not in H.264); else 8",
                            );
                    }
                    if vidcut_core::export::color::carries_hdr(v.codec) {
                        let label = |h: HdrMode| match h {
                            HdrMode::Auto => "auto",
                            HdrMode::Sdr => "SDR",
                            HdrMode::Pq => "HDR (PQ)",
                            HdrMode::Hlg => "HDR (HLG)",
                        };
                        ui.label(dim("range"));
                        egui::ComboBox::from_id_salt("export-hdr")
                            .selected_text(label(v.hdr))
                            .show_ui(ui, |ui| {
                                for h in [HdrMode::Auto, HdrMode::Sdr, HdrMode::Pq, HdrMode::Hlg] {
                                    ui.selectable_value(&mut v.hdr, h, label(h));
                                }
                            })
                            .response
                            .on_hover_text(
                                "auto: HDR when most of the picture is HDR. SDR: HDR sources \
                                 are tone-mapped. HDR: BT.2020 in 10 bits, SDR sources with \
                                 their white at 203 nits.",
                            );
                        if v.hdr.transfer().is_some() && v.bit_depth == Some(8) {
                            v.bit_depth = None;
                        }
                        if v.hdr != HdrMode::Sdr {
                            use vidcut_core::export::HdrMetadata as M;
                            let label = |m: M| match m {
                                M::Auto => "auto",
                                M::Static => "HDR10 only",
                                M::Hdr10plus => "HDR10 + HDR10+",
                                M::DolbyVision => "HDR10 + Dolby Vision",
                                M::All => "HDR10 + HDR10+ + DV",
                                M::None => "none",
                            };
                            ui.label(dim("metadata"));
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
                                })
                                .response
                                .on_hover_text(
                                    "auto: the sources' mastering display and light levels \
                                     (PQ), HDR10+ when any picture has it (made for the frames \
                                     without), and Dolby Vision when every picture has it. \
                                     Dolby Vision is written by the CPU's encoders.",
                                );
                        }
                    } else if v.hdr.transfer().is_some() {
                        v.hdr = HdrMode::Auto;
                    }
                    ui.label(dim("speed"));
                    let mut speed = v.speed.clone().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut speed)
                                .hint_text("default")
                                .desired_width(70.0),
                        )
                        .on_hover_text("encoder speed preset: slow, p5, 8, …")
                        .changed()
                    {
                        let s = speed.trim();
                        v.speed = (!s.is_empty()).then(|| s.to_string());
                    }
                });
                ui.end_row();
            }

            ui.label("Audio");
            ui.horizontal(|ui| {
                let mut on = d.settings.audio.is_some();
                if ui.checkbox(&mut on, "").changed() {
                    d.settings.audio = on.then(|| AudioSettings {
                        codec: audio_codecs(d.settings.container)[0],
                        bitrate: None,
                        sample_rate: None,
                        channels: None,
                        loudness: None,
                    });
                    fit_codecs(&mut d.settings);
                }
                if let Some(a) = &mut d.settings.audio {
                    egui::ComboBox::from_id_salt("export-acodec")
                        .selected_text(name(&a.codec))
                        .show_ui(ui, |ui| {
                            for c in audio_codecs(d.settings.container) {
                                let why = why_audio(can, *c);
                                if choice(ui, &mut a.codec, *c, name(c), why).changed() {
                                    a.bitrate = lossy(a.codec).then_some(256_000);
                                }
                            }
                        });
                    if lossy(a.codec) {
                        let mut kbps = a.bitrate.unwrap_or(256_000) / 1000;
                        if ui
                            .add(
                                egui::DragValue::new(&mut kbps)
                                    .range(32..=512)
                                    .suffix(" kb/s"),
                            )
                            .changed()
                        {
                            a.bitrate = Some(kbps * 1000);
                        }
                    }
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
                            for r in [44_100, 48_000, 96_000] {
                                ui.selectable_value(&mut a.sample_rate, Some(r), format!("{r} Hz"));
                            }
                        });
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
                            for c in [1, 2, 6] {
                                ui.selectable_value(&mut a.channels, Some(c), channels(c));
                            }
                        });
                }
            });
            ui.end_row();
            if let Some(a) = &mut d.settings.audio {
                ui.label("Loudness");
                ui.horizontal(|ui| {
                    let mut on = a.loudness.is_some();
                    if ui
                        .checkbox(&mut on, "Normalize to")
                        .on_hover_text(
                            "Measure the mix (EBU R128) and change it by one gain to this \
                             loudness, keeping true peaks at or below -1 dBTP. YouTube: -14.",
                        )
                        .changed()
                    {
                        a.loudness = on.then_some(-14.0);
                    }
                    if let Some(target) = &mut a.loudness {
                        ui.add(
                            egui::DragValue::new(target)
                                .range(-60.0..=-5.0)
                                .speed(0.1)
                                .suffix(" LUFS"),
                        );
                    }
                });
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{command_params, decoded_on, detail_rows, details_text, metadata_line};
    use crate::live::JobInfo;

    fn job() -> JobInfo {
        JobInfo::new("j1".into(), "gui", json!({ "output": "out.mp4" }))
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
