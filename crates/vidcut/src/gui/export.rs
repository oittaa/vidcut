//! Quick Export (Ch. 39, p. 788), the Export dialog (deviation X3), timeline
//! interchange (OpenTimelineIO, X9) and the jobs panel. All of it is a view
//! over requests: `presets.list`, `export.plan`, `export.start`,
//! `presets.save`, `presets.delete`, `timeline.import` and the session's
//! jobs (docs/decisions/gui.md §9).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_core::export::{
    AudioCodec, AudioSettings, Container, EncoderChoice, ExportSettings, VideoCodec, VideoSettings,
    validate,
};
use vidcut_core::{Envelope, Error, Response};

use super::theme;
use super::workspace::{Workspace, dim};
use crate::live::{self, JobInfo, JobState, Origin};

// ---------------------------------------------------------------- plumbing

/// A request answered on a helper thread, so the UI never waits for it.
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
        std::thread::spawn(move || {
            let result = envelope
                .map_err(Error::internal)
                .and_then(|e| live.call(e, Origin::Gui { drag: false }, None, &mut |_| true));
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            ctx.request_repaint();
        });
        Self { slot }
    }

    /// The answer, once it is there (only once).
    pub(super) fn take(&self) -> Option<vidcut_core::Result<Response>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// File dialogs. They run on helper threads (portal dialogs block); tests
/// replace them.
pub trait Dialogs: Send + Sync {
    /// Where to save a file; `None` if cancelled.
    fn save_file(
        &self,
        title: &str,
        folder: &Path,
        name: &str,
        extensions: &[&str],
    ) -> Option<PathBuf>;
    fn pick_file(&self, title: &str, folder: &Path, extensions: &[&str]) -> Option<PathBuf>;
    fn pick_folder(&self, title: &str, folder: &Path) -> Option<PathBuf>;
    /// Files to open, under named filters; none if cancelled.
    fn pick_files(&self, _title: &str, _filters: &[(&str, &[&str])]) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// The desktop's file dialogs (XDG portal).
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

    fn pick_files(&self, title: &str, filters: &[(&str, &[&str])]) -> Vec<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        for (name, extensions) in filters {
            dialog = dialog.add_filter(*name, extensions);
        }
        dialog
            .add_filter("all files", &["*"])
            .pick_files()
            .unwrap_or_default()
    }
}

/// A file dialog's answer, filled in by its helper thread.
#[derive(Clone)]
struct Picked(Arc<Mutex<Option<Option<PathBuf>>>>);

impl Picked {
    fn ask(ctx: &egui::Context, f: impl FnOnce() -> Option<PathBuf> + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let (out, ctx) = (slot.clone(), ctx.clone());
        std::thread::spawn(move || {
            let path = f();
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
            ctx.request_repaint();
        });
        Self(slot)
    }

    /// `Some(None)`: cancelled.
    fn take(&self) -> Option<Option<PathBuf>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// A preset as `presets.list` reports it.
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
}

// ---------------------------------------------------------------- state

/// The export windows' state (view state; the work is the session's).
#[derive(Default)]
pub struct ExportUi {
    presets: Vec<PresetInfo>,
    presets_ask: Option<Ask>,
    /// Quick Export is open.
    quick: Option<Quick>,
    /// The Export dialog is open.
    dialog: Option<Box<Dialog>>,
    /// Open the Export dialog once the presets are known.
    dialog_wanted: bool,
    /// An export being started: the file dialog, then `export.start`.
    starting: Option<Starting>,
    /// Jobs started here whose progress window is showing.
    watching: Vec<String>,
    /// The jobs window.
    pub show_jobs: bool,
    otio: Option<Otio>,
    /// Import warnings to show.
    warnings: Option<(String, Vec<String>)>,
    /// Closing was asked for while exports run.
    confirm_quit: bool,
    /// Closing is fine now (or was asked for by `gui.quit`).
    pub quit_confirmed: bool,
    /// Where the last export went.
    last_folder: Option<PathBuf>,
}

struct Quick {
    selected: Option<String>,
    error: Option<String>,
}

/// Where an export's errors go back to.
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
    /// Asking which file to import.
    PickImport(Picked),
    /// The timeline has clips: append or replace?
    Choose(PathBuf),
    Importing {
        ask: Ask,
        path: PathBuf,
        replace: bool,
    },
    /// Media files weren't where the file says: look in a folder? (p. 529)
    Missing {
        path: PathBuf,
        replace: bool,
        message: String,
    },
    PickSearch {
        picked: Picked,
        path: PathBuf,
        replace: bool,
    },
    /// Where to write, and the extension it gets if it has none.
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
    /// The preset the settings start from (always one: GUI exports read as
    /// `--preset NAME --set …` on the command line).
    preset: String,
    base: ExportSettings,
    settings: ExportSettings,
    /// Video settings while Audio only is on.
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
    /// The params of the plan shown (or being made).
    planned: Option<Value>,
    plan_result: Option<std::result::Result<Value, String>>,
    error: Option<String>,
    save: Option<SavePreset>,
}

struct SavePreset {
    name: String,
    title: String,
    quick_export: bool,
    overwrite: bool,
    ask: Option<Ask>,
    error: Option<String>,
}

// ---------------------------------------------------------------- helpers

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

/// The wire name of an enum value (`h264`, `pcm_s24`).
fn name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn lossy(a: AudioCodec) -> bool {
    matches!(a, AudioCodec::Aac | AudioCodec::Opus | AudioCodec::Mp3)
}

/// Codecs the container can't hold are swapped for ones it can.
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

/// `--set` overrides that turn `base` into `settings`, as dotted keys.
pub(super) fn overrides(base: &ExportSettings, settings: &ExportSettings) -> Vec<String> {
    fn raw(v: &Value) -> String {
        match v {
            // Plain strings stay plain unless they'd read as JSON ("8").
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

/// The shell command that makes the same export.
pub(super) fn command(project: &Path, p: &Value) -> String {
    let mut a: Vec<String> = vec![
        "vidcut".into(),
        "-p".into(),
        project.display().to_string(),
        "export".into(),
        p["output"].as_str().unwrap_or_default().into(),
    ];
    if let Some(x) = p["preset"].as_str() {
        a.extend(["--preset".into(), x.into()]);
    }
    for s in p["set"].as_array().into_iter().flatten() {
        a.extend(["--set".into(), s.as_str().unwrap_or_default().into()]);
    }
    if let Some(r) = p["range"].as_str() {
        a.extend(["--range".into(), r.into()]);
    }
    if p["per_clip"] == true {
        a.push("--per-clip".into());
    }
    if let Some(n) = p["name"].as_str() {
        a.extend(["--name".into(), n.into()]);
    }
    if p["audio_only"] == true {
        a.push("--audio-only".into());
    }
    if p["overwrite"] == true {
        a.push("--overwrite".into());
    }
    shlex::try_join(a.iter().map(String::as_str)).unwrap_or_default()
}

/// Where a window of about `w`×`h` starts so it sits near the middle. Its
/// top-left stays put afterwards, so it doesn't move as its content changes.
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

/// Opens the folder holding `path` in the file manager.
fn show_in_folder(path: &Path) {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
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
        }
    }

    /// Starts from another preset, keeping the output and range.
    fn use_preset(&mut self, preset: &PresetInfo) {
        self.preset = preset.name.clone();
        self.base = preset.settings.clone();
        self.settings = preset.settings.clone();
        self.stashed_video = None;
        self.match_output();
    }

    /// The output file's extension follows the container.
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

    /// The `export.start` / `export.plan` params.
    fn params(&self) -> Value {
        let output = self.output.trim();
        let mut p = json!({ "output": output, "preset": self.preset });
        let (mut base, mut settings) = (self.base.clone(), self.settings.clone());
        if settings.video.is_none() && base.video.is_some() {
            p["audio_only"] = json!(true);
            base.video = None;
            settings.video = None;
        }
        // A single file's extension sets the container anyway.
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
        p
    }
}

// ---------------------------------------------------------------- windows

impl Workspace {
    /// Quick Export, opened from the top bar (p. 788).
    pub(super) fn open_quick_export(&mut self, ctx: &egui::Context) {
        self.refresh_presets(ctx);
        self.export.quick = Some(Quick {
            selected: None,
            error: None,
        });
    }

    /// The Export dialog (deviation X3).
    pub(super) fn open_export(&mut self, ctx: &egui::Context) {
        self.refresh_presets(ctx);
        self.export.dialog = None;
        self.export.quick = None;
        // Opened once the presets are known (see `export_windows`).
        self.export.starting = None;
        self.export.dialog_wanted = true;
    }

    fn refresh_presets(&mut self, ctx: &egui::Context) {
        self.export.presets_ask = Some(Ask::send(&self.live, ctx, "presets.list", json!({})));
    }

    /// Where exports go by default: the last folder, else the project's.
    fn export_folder(&self) -> PathBuf {
        self.export
            .last_folder
            .clone()
            .or_else(|| {
                self.snap
                    .project
                    .as_ref()
                    .and_then(|p| p.path.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn project_stem(&self) -> String {
        self.snap
            .project
            .as_ref()
            .and_then(|p| p.path.file_stem())
            .map_or("export".into(), |s| s.to_string_lossy().into_owned())
    }

    fn marks_set(&self) -> bool {
        self.project()
            .is_some_and(|p| p.marks.mark_in.is_some() && p.marks.mark_out.is_some())
    }

    /// Draws the export windows; called once per frame.
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
        if self.export.dialog_wanted && !self.export.presets.is_empty() {
            self.export.dialog_wanted = false;
            let preset = self
                .export
                .presets
                .iter()
                .find(|p| p.name == "h264-master")
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
            self.export.dialog = Some(Box::new(Dialog::new(&preset, output, range)));
        }
        self.quick_window(ctx);
        self.dialog_window(ctx);
        self.starting(ctx);
        self.progress_windows(ctx);
        self.jobs_window(ctx);
        self.otio_windows(ctx);
        self.quit_window(ctx);
    }

    fn quick_window(&mut self, ctx: &egui::Context) {
        let Some(mut q) = self.export.quick.take() else {
            return;
        };
        let presets: Vec<PresetInfo> = self
            .export
            .presets
            .iter()
            .filter(|p| p.quick_export)
            .cloned()
            .collect();
        if q.selected.is_none() {
            q.selected = presets.first().map(|p| p.name.clone());
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
        let mut more = false;
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
                            .add_sized([150.0, 56.0], egui::Button::selectable(selected, text))
                            .on_hover_text(&p.description);
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
                if let Some(e) = &q.error {
                    ui.label(RichText::new(e).color(theme::ERROR));
                }
                ui.horizontal(|ui| {
                    let busy = self.export.starting.is_some();
                    if ui
                        .add_enabled(!busy && q.selected.is_some(), egui::Button::new("Export…"))
                        .clicked()
                    {
                        export = true;
                    }
                    if ui.button("More Settings…").clicked() {
                        more = true;
                    }
                });
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
            self.export.starting = Some(Starting {
                // Quick Export semantics: In–Out if set, else everything (p. 789).
                params: json!({ "preset": name, "overwrite": true, "background": true }),
                from: From::Quick,
                picking: Some(picking),
                ask: None,
            });
        }
        if more {
            self.open_export(ctx);
            return;
        }
        if open {
            self.export.quick = Some(q);
        }
    }

    /// An export on its way: the file dialog, then `export.start`.
    fn starting(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.export.starting.take() else {
            return;
        };
        if let Some(p) = &s.picking
            && let Some(choice) = p.take()
        {
            s.picking = None;
            match choice {
                None => return, // cancelled
                Some(path) => {
                    self.export.last_folder = path.parent().map(Path::to_path_buf);
                    s.params["output"] = json!(path);
                    s.ask = Some(Ask::send(&self.live, ctx, "export.start", s.params.clone()));
                }
            }
        }
        if let Some(ask) = &s.ask
            && let Some(r) = ask.take()
        {
            match r {
                Ok(r) => {
                    if let Some(id) = r.result["id"].as_str() {
                        self.export.watching.push(id.to_string());
                    }
                    match s.from {
                        From::Quick => self.export.quick = None,
                        From::Dialog => self.export.dialog = None,
                    }
                }
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
        // Browse… answered.
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
        let mut close = false;
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
                        // Preset.
                        ui.horizontal(|ui| {
                            ui.label("Preset");
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
                                        if ui
                                            .selectable_label(
                                                p.name == d.preset,
                                                format!("{}{mark}", p.title),
                                            )
                                            .on_hover_text(&p.description)
                                            .clicked()
                                        {
                                            d.use_preset(p);
                                        }
                                    }
                                });
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
                        settings_form(ui, &mut d, (width, height), (rate, channels));
                        ui.separator();
                        // Range.
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
                        // Output.
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
                            if r.lost_focus() && !d.per_clip {
                                // A typed extension picks the container.
                                if let Some(c) = Path::new(d.output.trim())
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .and_then(Container::from_extension)
                                    && c != d.settings.container
                                {
                                    d.settings.container = c;
                                    fit_codecs(&mut d.settings);
                                }
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
                        plan_view(ui, &d);
                        ui.separator();
                        // The same export from a shell.
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
                // The actions stay in view below the scrolling settings.
                ui.separator();
                if let Some(e) = &d.error {
                    ui.label(RichText::new(e).color(theme::ERROR));
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
                    if ui.button("Close").clicked() {
                        close = true;
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
            self.export.last_folder = Path::new(d.output.trim()).parent().map(Path::to_path_buf);
            self.export.starting = Some(Starting {
                ask: Some(Ask::send(&self.live, ctx, "export.start", params.clone())),
                params,
                from: From::Dialog,
                picking: None,
            });
        }
        if open && !close {
            self.export.dialog = Some(d);
        }
    }

    /// Plans the export again when its params change (one plan at a time).
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
        // Mistakes the dialog can see itself don't need the session.
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
                    // The dialog now starts from the saved preset.
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
                ui.label(RichText::new(e).color(theme::ERROR));
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

    /// A progress window for each export started here (p. 790).
    fn progress_windows(&mut self, ctx: &egui::Context) {
        let jobs = self.snap.jobs.clone();
        let mut close = Vec::new();
        for id in &self.export.watching {
            let Some(j) = jobs.iter().find(|j| &j.id == id) else {
                continue;
            };
            let mut open = true;
            egui::Window::new(format!("Export {}", j.title))
                .id(egui::Id::new(("export-progress", id)))
                .collapsible(false)
                .resizable(false)
                .default_pos(near_center(ctx, 380.0, 120.0))
                .open(&mut open)
                .show(ctx, |ui| {
                    if job_view(ui, j, true, |method, params| self.send(method, params)) {
                        close.push(id.clone());
                    }
                    if j.running()
                        && ui
                            .button("Hide")
                            .on_hover_text("Keep exporting; Jobs shows it")
                            .clicked()
                    {
                        close.push(id.clone());
                    }
                });
            if !open {
                close.push(id.clone());
            }
        }
        self.export.watching.retain(|id| !close.contains(id));
    }

    fn jobs_window(&mut self, ctx: &egui::Context) {
        if !self.export.show_jobs {
            return;
        }
        let jobs = self.snap.jobs.clone();
        let mut open = true;
        egui::Window::new("Jobs")
            .default_width(420.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if jobs.is_empty() {
                    ui.label(dim(
                        "No exports yet. Exports from vidcut CLI commands show here too.",
                    ));
                }
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        for j in jobs.iter().rev() {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&j.title).strong());
                                    ui.label(dim(format!("{} · from {}", j.id, j.origin)));
                                });
                                job_view(ui, j, false, |method, params| self.send(method, params));
                            });
                        }
                    });
            });
        self.export.show_jobs = open;
    }

    /// File > Import Timeline / Export Timeline (OpenTimelineIO). An
    /// `.otiod` bundle imports through its `content.otio`.
    pub(super) fn import_timeline(&mut self, ctx: &egui::Context) {
        let (dialogs, folder) = (self.dialogs.clone(), self.export_folder());
        self.export.otio = Some(Otio::PickImport(Picked::ask(ctx, move || {
            dialogs.pick_file("Import Timeline", &folder, &["otio", "otioz"])
        })));
    }

    /// The edit alone (`.otio`), or with its media in a bundle (`.otioz`, as
    /// Resolve's File > Export > Timeline offers both).
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
        let import = |ws: &mut Self, path: PathBuf, replace: bool, search: Option<PathBuf>| {
            if vidcut_core::editor::is_otioz(&path) {
                ws.say(format!(
                    "Unpacking the media of {}…",
                    path.file_name()
                        .map_or(String::new(), |n| n.to_string_lossy().into_owned())
                ));
            }
            let mut params = json!({ "path": path, "replace": replace });
            if let Some(dir) = search {
                params["search"] = json!([dir]);
            }
            Otio::Importing {
                ask: Ask::send(&ws.live, ctx, "timeline.import", params),
                path,
                replace,
            }
        };
        self.export.otio = match state {
            Otio::PickImport(p) => match p.take() {
                None => Some(Otio::PickImport(p)),
                Some(None) => None,
                Some(Some(path)) => {
                    let empty = self
                        .project()
                        .is_none_or(|p| p.tracks.iter().all(|t| t.clips.is_empty()));
                    Some(if empty {
                        import(self, path, false, None)
                    } else {
                        Otio::Choose(path)
                    })
                }
            },
            Otio::Choose(path) => {
                let mut choice = None;
                egui::Modal::new(egui::Id::new("otio-import-choice")).show(ctx, |ui| {
                    ui.label(format!(
                        "Import {}:",
                        path.file_name()
                            .map_or(String::new(), |n| n.to_string_lossy().into_owned())
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Append to the Timeline").clicked() {
                            choice = Some(Some(false));
                        }
                        if ui.button("Replace the Timeline").clicked() {
                            choice = Some(Some(true));
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(None);
                        }
                    });
                });
                match choice {
                    None => Some(Otio::Choose(path)),
                    Some(None) => None,
                    Some(Some(replace)) => Some(import(self, path, replace, None)),
                }
            }
            Otio::Missing {
                path,
                replace,
                message,
            } => {
                let mut choice = None;
                egui::Modal::new(egui::Id::new("otio-missing-media")).show(ctx, |ui| {
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
                match choice {
                    None => Some(Otio::Missing {
                        path,
                        replace,
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
                            replace,
                        })
                    }
                }
            }
            Otio::PickSearch {
                picked,
                path,
                replace,
            } => match picked.take() {
                None => Some(Otio::PickSearch {
                    picked,
                    path,
                    replace,
                }),
                Some(None) => None,
                Some(Some(dir)) => Some(import(self, path, replace, Some(dir))),
            },
            Otio::Importing { ask, path, replace } => match ask.take() {
                None => Some(Otio::Importing { ask, path, replace }),
                Some(Ok(r)) => {
                    let n = r.result["clips"].as_array().map_or(0, Vec::len);
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
                    self.say(format!("Imported {n} clip(s) from {name}"));
                    if !warnings.is_empty() {
                        self.export.warnings = Some((name, warnings));
                    }
                    None
                }
                Some(Err(e)) if e.code == vidcut_core::ErrorCode::Media => Some(Otio::Missing {
                    path,
                    replace,
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
                    self.export.last_folder = path.parent().map(Path::to_path_buf);
                    // A bundle copies media: a job, as media exports are.
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
                    self.export.watching.push(id);
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

    /// Asked to close while exports run.
    fn quit_window(&mut self, ctx: &egui::Context) {
        let running = self.snap.jobs.iter().filter(|j| j.running()).count();
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
        egui::Modal::new(egui::Id::new("quit-while-exporting")).show(ctx, |ui| {
            ui.label(format!(
                "{running} export(s) still running. Quitting cancels them."
            ));
            ui.horizontal(|ui| {
                if ui.button("Keep Exporting").clicked() {
                    self.export.confirm_quit = false;
                }
                if ui.button("Cancel Exports and Quit").clicked() {
                    self.export.confirm_quit = false;
                    self.export.quit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}

/// One job: progress, time, and what to do with it. With `closable`, an
/// ended job has a Close button; returns whether it was clicked.
fn job_view(ui: &mut egui::Ui, j: &JobInfo, closable: bool, send: impl Fn(&str, Value)) -> bool {
    let mut closed = false;
    match j.state {
        JobState::Running => {
            let text = match j.eta {
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
            ui.label(dim(format!(
                "{} / {} frames{}",
                j.frames,
                j.total,
                if j.outputs.len() > 1 {
                    format!(" · file {} of {}", j.output + 1, j.outputs.len())
                } else {
                    String::new()
                }
            )));
            if ui.button("Cancel").clicked() {
                send("jobs.cancel", json!({ "job": j.id }));
            }
        }
        JobState::Done => {
            ui.label(RichText::new(format!("Done in {}", clock(j.elapsed))).color(theme::OK));
            ui.horizontal(|ui| {
                if let Some(path) = job_paths(j) {
                    ui.label(dim(path.display().to_string()));
                    if ui.small_button("Show in Folder").clicked() {
                        show_in_folder(path);
                    }
                }
            });
            closed = closable && ui.button("Close").clicked();
        }
        JobState::Cancelled => {
            ui.label(dim("Cancelled; nothing was written"));
            closed = closable && ui.button("Close").clicked();
        }
        JobState::Failed => {
            let e = j.error.as_ref().map_or("", |e| e.message.as_str());
            ui.label(RichText::new(format!("Failed: {e}")).color(theme::ERROR));
            closed = closable && ui.button("Close").clicked();
        }
    }
    closed
}

/// What `export.plan` says the export will do.
fn plan_view(ui: &mut egui::Ui, d: &Dialog) {
    ui.label(dim("Plan"));
    match &d.plan_result {
        None => {
            ui.label(dim("planning…"));
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::ERROR));
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
                        .color(theme::ERROR),
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
        }
    }
}

/// Container, video and audio settings.
fn settings_form(ui: &mut egui::Ui, d: &mut Dialog, size: (u32, u32), audio: (u32, u16)) {
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
                                hdr: None,
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
                                if ui.selectable_value(&mut v.codec, *c, name(c)).changed() {
                                    v.profile = None;
                                }
                            }
                        });
                    ui.label(dim("encoder"));
                    egui::ComboBox::from_id_salt("export-encoder")
                        .selected_text(name(&v.encoder))
                        .show_ui(ui, |ui| {
                            for e in [
                                EncoderChoice::Auto,
                                EncoderChoice::Cpu,
                                EncoderChoice::Nvenc,
                                EncoderChoice::Vaapi,
                            ] {
                                ui.selectable_value(&mut v.encoder, e, name(&e));
                            }
                        });
                });
                ui.end_row();

                ui.label("");
                ui.horizontal(|ui| {
                    let mut by_quality = v.bitrate.is_none();
                    if ui.radio_value(&mut by_quality, true, "Quality").changed() {
                        v.bitrate = None;
                        v.quality = v.quality.or(Some(20));
                    }
                    if ui.radio_value(&mut by_quality, false, "Bitrate").changed() {
                        v.bitrate = Some(20_000_000);
                        v.quality = None;
                    }
                    if by_quality {
                        let mut q = v.quality.unwrap_or(20);
                        if ui
                            .add(egui::DragValue::new(&mut q).range(0..=63))
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
                    ui.label(dim("bit depth"));
                    egui::ComboBox::from_id_salt("export-depth")
                        .selected_text(v.bit_depth.map_or("default".into(), |b| b.to_string()))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut v.bit_depth, None, "default");
                            ui.selectable_value(&mut v.bit_depth, Some(8), "8");
                            ui.selectable_value(&mut v.bit_depth, Some(10), "10");
                        });
                    // Dynamic range: HDR needs a 10-bit HEVC, AV1 or VP9.
                    if matches!(
                        v.codec,
                        VideoCodec::Hevc | VideoCodec::Av1 | VideoCodec::Vp9
                    ) {
                        use vidcut_core::model::Hdr;
                        let label = |h: Option<Hdr>| match h {
                            None => "SDR",
                            Some(Hdr::Pq) => "HDR (PQ)",
                            Some(Hdr::Hlg) => "HDR (HLG)",
                        };
                        ui.label(dim("range"));
                        egui::ComboBox::from_id_salt("export-hdr")
                            .selected_text(label(v.hdr))
                            .show_ui(ui, |ui| {
                                for h in [None, Some(Hdr::Pq), Some(Hdr::Hlg)] {
                                    ui.selectable_value(&mut v.hdr, h, label(h));
                                }
                            })
                            .response
                            .on_hover_text(
                                "SDR: HDR sources are tone-mapped. HDR: BT.2020 in 10 bits, \
                                 SDR sources with their white at 203 nits.",
                            );
                        if v.hdr.is_some() && v.bit_depth == Some(8) {
                            v.bit_depth = None;
                        }
                    } else {
                        v.hdr = None;
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
                                if ui.selectable_value(&mut a.codec, *c, name(c)).changed() {
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
