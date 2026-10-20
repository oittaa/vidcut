use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_core::Project;
use vidcut_core::model::Settings;
use vidcut_core::time::{Rational, STANDARD_RATES, TimeSpec};

use super::export::{Ask, Picked, near_center};
use super::theme;
use super::workspace::Workspace;

const SIZES: [(&str, u32, u32); 7] = [
    ("UHD", 3840, 2160),
    ("DCI 4K", 4096, 2160),
    ("QHD", 2560, 1440),
    ("Full HD", 1920, 1080),
    ("HD", 1280, 720),
    ("Vertical", 1080, 1920),
    ("Square", 1080, 1080),
];

const MAX_SIDE: u32 = 16384;

const START_HOURS: [i64; 3] = [0, 1, 10];

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Format {
    width: u32,
    height: u32,
    rate: String,
    drop_frame: bool,
    start: String,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            rate: "30".into(),
            drop_frame: false,
            start: "00:00:00:00".into(),
        }
    }
}

impl Format {
    fn of(p: &Project) -> Self {
        Self {
            width: p.settings.width,
            height: p.settings.height,
            rate: rate_text(p.settings.fps),
            drop_frame: p.settings.drop_frame,
            start: p.settings.timecode(0),
        }
    }

    fn fps(&self) -> Option<Rational> {
        self.rate.trim().parse().ok()
    }

    fn drop_frame(&self) -> bool {
        self.drop_frame && self.fps().is_some_and(Rational::has_drop_frame)
    }

    fn start(&self) -> Option<&str> {
        let t = self.start.trim();
        matches!(TimeSpec::parse(t), Ok(TimeSpec::Timecode(..))).then_some(t)
    }

    fn starts_at_zero(&self) -> bool {
        self.start()
            .is_some_and(|t| t.replace(';', ":") == "00:00:00:00")
    }

    fn edit_start(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new(ui.scope_id().with("start"))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let label = ui.label("Start timecode");
                ui.horizontal(|ui| {
                    let sep = if self.drop_frame() { ';' } else { ':' };
                    let offered: Vec<String> = START_HOURS
                        .iter()
                        .map(|h| format!("{h:02}:00:00{sep}00"))
                        .collect();
                    let same = |a: &str, b: &str| a.trim().replace(';', ":") == b.replace(';', ":");
                    let named = offered
                        .iter()
                        .find(|o| same(&self.start, o))
                        .cloned()
                        .unwrap_or_else(|| "Other".into());
                    egui::ComboBox::from_id_salt("start timecode")
                        .selected_text(named)
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for o in &offered {
                                if ui.selectable_label(same(&self.start, o), o).clicked() {
                                    self.start = o.clone();
                                }
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    let r =
                        ui.add(egui::TextEdit::singleline(&mut self.start).desired_width(100.0));
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            "Start timecode value",
                        )
                    });
                    if self.start().is_none() {
                        ui.label(
                            RichText::new("a timecode, like 01:00:00:00")
                                .color(theme::colors().error),
                        );
                    }
                });
                ui.end_row();
            });
    }

    fn edit(&mut self, ui: &mut egui::Ui, rate_enabled: bool) {
        egui::Grid::new(ui.scope_id().with("format"))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                let label = ui.label("Resolution");
                ui.horizontal(|ui| {
                    let named = SIZES
                        .iter()
                        .find(|s| (s.1, s.2) == (self.width, self.height))
                        .map_or("Custom".to_string(), |s| size_name(*s));
                    egui::ComboBox::from_id_salt("resolution")
                        .selected_text(named)
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            for s in SIZES {
                                let on = (s.1, s.2) == (self.width, self.height);
                                if ui.selectable_label(on, size_name(s)).clicked() {
                                    (self.width, self.height) = (s.1, s.2);
                                }
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    for (side, name) in [(&mut self.width, "Width"), (&mut self.height, "Height")] {
                        let r = ui.add(
                            egui::DragValue::new(side)
                                .range(16..=MAX_SIDE)
                                .speed(2.0)
                                .suffix(" px"),
                        );
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::SpinButton, true, name)
                        });
                        *side += *side % 2;
                        if name == "Width" {
                            ui.label("×");
                        }
                    }
                });
                ui.end_row();
                let label = ui.label("Frame rate");
                if !rate_enabled {
                    super::dialog::why(ui, "only while the timeline is empty");
                }
                ui.add_enabled_ui(rate_enabled, |ui| {
                    ui.horizontal(|ui| {
                        let current = self.fps();
                        egui::ComboBox::from_id_salt("frame rate")
                            .selected_text(match current {
                                Some(f) if is_standard(f) => format!("{} fps", f.decimal()),
                                _ => "Other".into(),
                            })
                            .width(110.0)
                            .show_ui(ui, |ui| {
                                for (n, d) in STANDARD_RATES {
                                    let f = Rational::new(n, d).expect("standard rate");
                                    let text = format!("{} fps", f.decimal());
                                    if ui.selectable_label(current == Some(f), text).clicked() {
                                        self.rate = f.decimal();
                                    }
                                }
                            })
                            .response
                            .labelled_by(label.id);
                        let r =
                            ui.add(egui::TextEdit::singleline(&mut self.rate).desired_width(90.0));
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::TextInput,
                                true,
                                "Frame rate value",
                            )
                        });
                        if current.is_none() {
                            ui.label(
                                RichText::new("a rate above 0, like 25 or 30000/1001")
                                    .color(theme::colors().error),
                            );
                        }
                    });
                });
                ui.end_row();
                if self.fps().is_some_and(Rational::has_drop_frame) {
                    ui.label("Timecode");
                    ui.checkbox(&mut self.drop_frame, "Drop-frame");
                    ui.end_row();
                }
            });
    }
}

fn size_name((name, w, h): (&str, u32, u32)) -> String {
    format!("{name} {w} × {h}")
}

fn is_standard(f: Rational) -> bool {
    STANDARD_RATES.contains(&(f.num(), f.den()))
}

fn rate_text(f: Rational) -> String {
    let d = f.decimal();
    if d.parse::<Rational>() == Ok(f) {
        d
    } else {
        f.to_string()
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct NewProject {
    choose: bool,
    format: Format,
    from: Option<OtioFile>,
}

#[derive(Debug, Clone)]
pub(super) struct OtioFile {
    otio: PathBuf,
    target: PathBuf,
    ours: bool,
    format: Format,
}

impl OtioFile {
    pub(super) fn read(otio: PathBuf) -> Result<Self, String> {
        use vidcut_core::Env;
        let file = crate::env::AvEnv::new()
            .open_timeline(&otio, None, false)
            .map_err(|e| e.message)?;
        let doc: Value = serde_json::from_str(&file.text)
            .map_err(|e| format!("{} isn't an OTIO file: {e}", file_name(&otio)))?;
        let t = vidcut_core::otio::import(&doc, None).map_err(|e| e.message)?;
        let settings = Settings {
            fps: t.fps,
            start_tc: t.start_tc,
            ..Settings::default()
        };
        Ok(Self {
            target: otio.with_extension("vidcut"),
            ours: vidcut_core::otio::holds_project(&doc),
            format: Format {
                rate: rate_text(t.fps),
                start: settings.timecode(0),
                ..Format::default()
            },
            otio,
        })
    }
}

pub(super) type Found = Arc<Mutex<Option<Result<OtioFile, String>>>>;

enum OpenOtio {
    Taken(OtioFile),
    SaveAs(OtioFile, Picked),
    Opening(OtioFile, Ask),
}

#[derive(Default)]
pub(super) struct ProjectUi {
    new: Option<NewProject>,
    settings: Option<(Format, Format)>,
    pub(super) found: Found,
    otio: Option<OpenOtio>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned())
}

impl Workspace {
    pub(super) fn open_new_project(&mut self) {
        self.project_ui.new = Some(NewProject::default());
    }

    fn otio_next(&mut self, ctx: &egui::Context, f: OtioFile) {
        if f.target.exists() {
            self.project_ui.otio = Some(OpenOtio::Taken(f));
        } else if f.ours {
            self.open_otio(ctx, f, json!({}));
        } else {
            self.project_ui.new = Some(NewProject {
                choose: false,
                format: f.format.clone(),
                from: Some(f),
            });
        }
    }

    fn open_otio(&mut self, ctx: &egui::Context, f: OtioFile, format: Value) {
        let mut params = json!({ "path": f.otio, "save_as": f.target });
        if let (Some(p), Some(more)) = (params.as_object_mut(), format.as_object()) {
            p.extend(more.clone());
        }
        self.say(format!("Opening {}…", file_name(&f.otio)));
        let ask = Ask::send(&self.live, ctx, "project.open", params);
        self.project_ui.otio = Some(OpenOtio::Opening(f, ask));
    }

    fn open_otio_window(&mut self, ctx: &egui::Context) {
        let found = self
            .project_ui
            .found
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        match found {
            Some(Ok(f)) => self.otio_next(ctx, f),
            Some(Err(message)) => self.say(message),
            None => {}
        }
        let Some(state) = self.project_ui.otio.take() else {
            return;
        };
        self.project_ui.otio = match state {
            OpenOtio::Taken(f) => {
                let mut choice = None;
                let modal = egui::Modal::new(egui::Id::unique("otio-open-taken")).show(ctx, |ui| {
                    ui.label(format!(
                        "{} is already there. Open it, or save the project from {} under another name.",
                        file_name(&f.target),
                        file_name(&f.otio)
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Open It").clicked() {
                            choice = Some(Some(true));
                        }
                        if ui.button("Save As…").clicked() {
                            choice = Some(Some(false));
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(None);
                        }
                    });
                });
                if choice.is_none() && super::workspace::escape_cancels(ctx, &modal) {
                    choice = Some(None);
                }
                match choice {
                    None => Some(OpenOtio::Taken(f)),
                    Some(None) => None,
                    Some(Some(true)) => {
                        self.send("project.open", json!({ "path": f.target }));
                        None
                    }
                    Some(Some(false)) => {
                        let dialogs = self.dialogs.clone();
                        let folder = f.target.parent().map(Path::to_path_buf).unwrap_or_default();
                        let name = file_name(&f.target);
                        let picked = Picked::ask(ctx, move || {
                            dialogs.save_file("Save the Project As", &folder, &name, &["vidcut"])
                        });
                        Some(OpenOtio::SaveAs(f, picked))
                    }
                }
            }
            OpenOtio::SaveAs(mut f, picked) => match picked.take() {
                None => Some(OpenOtio::SaveAs(f, picked)),
                Some(None) => None,
                Some(Some(mut path)) => {
                    if path.extension().is_none() {
                        path.set_extension("vidcut");
                    }
                    f.target = path;
                    self.otio_next(ctx, f);
                    self.project_ui.otio.take()
                }
            },
            OpenOtio::Opening(f, ask) => match ask.take() {
                None => Some(OpenOtio::Opening(f, ask)),
                Some(Ok(r)) => {
                    let warnings: Vec<String> = r.result["warnings"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|w| w.as_str().map(String::from))
                        .collect();
                    if !warnings.is_empty() {
                        self.show_import_warnings(file_name(&f.otio), warnings);
                    }
                    None
                }
                Some(Err(e)) => {
                    self.say(format!(
                        "Couldn't open {}: {}",
                        file_name(&f.otio),
                        e.message
                    ));
                    None
                }
            },
        };
    }

    pub(super) fn open_project_settings(&mut self) {
        if let Some(p) = self.project() {
            let now = Format::of(p);
            self.project_ui.settings = Some((now.clone(), now));
        }
    }

    pub(super) fn project_windows(&mut self, ctx: &egui::Context) {
        self.new_project_window(ctx);
        self.project_settings_window(ctx);
        self.open_otio_window(ctx);
    }

    fn new_project_window(&mut self, ctx: &egui::Context) {
        let Some(mut new) = self.project_ui.new.take() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut create = None;
        egui::Window::new("New Project")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 420.0, 200.0))
            .open(&mut open)
            .show(ctx, |ui| {
                let auto = match &new.from {
                    Some(f) => {
                        ui.label(format!(
                            "The project from {}, an OpenTimelineIO file from another editor:",
                            file_name(&f.otio)
                        ));
                        "Take the file's frame rate and start, and its first video's size"
                    }
                    None => "Take the format of the first video added",
                };
                ui.radio_value(&mut new.choose, false, auto);
                ui.radio_value(&mut new.choose, true, "Choose it now:");
                ui.add_enabled_ui(new.choose, |ui| {
                    ui.indent("format", |ui| new.format.edit(ui, true));
                });
                if new.from.is_none() || new.choose {
                    new.format.edit_start(ui);
                }
                ui.label(
                    RichText::new(
                        "The size can change later; the frame rate only while the timeline is empty.",
                    )
                    .small()
                    .color(theme::colors().text_dim),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ok = (!new.choose || new.format.fps().is_some())
                        && new.format.start().is_some();
                    let button = if new.from.is_some() {
                        "Create"
                    } else {
                        "Create…"
                    };
                    if ui.add_enabled(ok, egui::Button::new(button)).clicked() {
                        let mut format = if new.choose {
                            let fps = new.format.fps().expect("checked");
                            let mut format = json!({
                                "fps": rate_text(fps),
                                "width": new.format.width,
                                "height": new.format.height,
                            });
                            if new.format.drop_frame() {
                                format["drop_frame"] = json!(true);
                            }
                            format
                        } else {
                            json!({})
                        };
                        let start = match new.from {
                            Some(_) => new.choose,
                            None => !new.format.starts_at_zero(),
                        };
                        if start {
                            format["start_tc"] = json!(new.format.start());
                        }
                        create = Some(format);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(format) = create {
            match new.from {
                Some(f) => self.open_otio(ctx, f, format),
                None => self.pick_project(ctx, Some(format)),
            }
        } else if open && !cancel {
            self.project_ui.new = Some(new);
        }
    }

    fn project_settings_window(&mut self, ctx: &egui::Context) {
        let Some((mut format, was)) = self.project_ui.settings.take() else {
            return;
        };
        let Some(p) = self.project() else {
            return;
        };
        let empty = p.tracks.iter().all(|t| t.clips.is_empty());
        let mut open = true;
        let mut cancel = false;
        let mut apply = None;
        egui::Window::new("Project Settings")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 420.0, 160.0))
            .open(&mut open)
            .show(ctx, |ui| {
                format.edit(ui, empty);
                format.edit_start(ui);
                if !empty {
                    ui.label(
                        RichText::new(
                            "The frame rate can change only while the timeline is empty.",
                        )
                        .small()
                        .color(theme::colors().text_dim),
                    );
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let changes = changes(&format, &was);
                    let ok = format.fps().is_some()
                        && format.start().is_some()
                        && changes.as_object().is_some_and(|c| !c.is_empty());
                    if ui.add_enabled(ok, egui::Button::new("Apply")).clicked() {
                        apply = Some(changes);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(changes) = apply {
            self.send("project.set", changes);
        } else if open && !cancel {
            self.project_ui.settings = Some((format, was));
        }
    }
}

fn changes(now: &Format, was: &Format) -> Value {
    let mut set = serde_json::Map::new();
    if (now.width, now.height) != (was.width, was.height) {
        set.insert("width".into(), json!(now.width));
        set.insert("height".into(), json!(now.height));
    }
    if let Some(f) = now.fps().filter(|&f| Some(f) != was.fps()) {
        set.insert("fps".into(), json!(f.to_string()));
    }
    if now.drop_frame() != was.drop_frame() {
        set.insert("drop_frame".into(), json!(now.drop_frame()));
    }
    if let Some(start) = now.start().filter(|&s| Some(s) != was.start()) {
        set.insert("start_tc".into(), json!(start));
    }
    Value::Object(set)
}
