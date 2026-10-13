//! New Project and File > Project Settings: the picture size, frame rate
//! (decision D37), drop-frame timecode (decision D47) and start timecode
//! (deviation X7). New Project sends one `project.open` with `create` and
//! the format, Project Settings one `project.set` with what changed.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_core::Project;
use vidcut_core::time::{Rational, STANDARD_RATES, TimeSpec};

use super::export::near_center;
use super::theme;
use super::workspace::Workspace;

/// Picture sizes to pick from; any even size from 16 up can be typed.
const SIZES: [(&str, u32, u32); 7] = [
    ("UHD", 3840, 2160),
    ("DCI 4K", 4096, 2160),
    ("QHD", 2560, 1440),
    ("Full HD", 1920, 1080),
    ("HD", 1280, 720),
    ("Vertical", 1080, 1920),
    ("Square", 1080, 1080),
];

/// The largest side a project can have here.
const MAX_SIDE: u32 = 16384;

/// The start timecodes offered, besides any other typed: zero, an hour
/// (where programmes start, with leader before it) and ten hours.
const START_HOURS: [i64; 3] = [0, 1, 10];

/// The picture size, frame rate and timecode being chosen.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Format {
    width: u32,
    height: u32,
    /// The frame rate as typed: `25`, `29.97`, `30000/1001`, `12.5`.
    rate: String,
    /// Timecode counts drop-frame, where the rate has it.
    drop_frame: bool,
    /// The start timecode as typed: `01:00:00:00`, `01:00:00;00`.
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

    /// The typed rate, if it is one.
    fn fps(&self) -> Option<Rational> {
        self.rate.trim().parse().ok()
    }

    /// Drop-frame counting as chosen, at a rate that has it.
    fn drop_frame(&self) -> bool {
        self.drop_frame && self.fps().is_some_and(Rational::has_drop_frame)
    }

    /// The typed start timecode, if it is one.
    fn start(&self) -> Option<&str> {
        let t = self.start.trim();
        matches!(TimeSpec::parse(t), Ok(TimeSpec::Timecode(..))).then_some(t)
    }

    /// Whether the typed start is the zero one.
    fn starts_at_zero(&self) -> bool {
        self.start()
            .is_some_and(|t| t.replace(';', ":") == "00:00:00:00")
    }

    /// The start timecode: an offered one, or any other typed.
    fn edit_start(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new(ui.id().with("start"))
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
                    // `;` and `:` name the same preset.
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
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut self.start).desired_width(100.0))
                        .on_hover_text("Any timecode, like 00:59:30:00 (drop-frame 01:00:00;00)");
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::TextEdit,
                            true,
                            "Start timecode value",
                        )
                    });
                    if self.start().is_none() {
                        ui.label(RichText::new("a timecode, like 01:00:00:00").color(theme::ERROR));
                    }
                });
                ui.end_row();
            });
    }

    /// The size and rate fields, `rate_enabled` or not.
    fn edit(&mut self, ui: &mut egui::Ui, rate_enabled: bool) {
        egui::Grid::new(ui.id().with("format"))
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
                        let r = ui
                            .add(
                                egui::DragValue::new(side)
                                    .range(16..=MAX_SIDE)
                                    .speed(2.0)
                                    .suffix(" px"),
                            )
                            .on_hover_text(name);
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, name)
                        });
                        // Encoders take even sizes.
                        *side += *side % 2;
                        if name == "Width" {
                            ui.label("×");
                        }
                    }
                });
                ui.end_row();
                let label = ui.label("Frame rate");
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
                        let r = ui
                            .add(egui::TextEdit::singleline(&mut self.rate).desired_width(90.0))
                            .on_hover_text(
                                "Any rate: 24, 29.97, 30000/1001 or 12.5 frames a second",
                            );
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::TextEdit,
                                true,
                                "Frame rate value",
                            )
                        });
                        if current.is_none() {
                            ui.label(
                                RichText::new("a rate above 0, like 25 or 30000/1001")
                                    .color(theme::ERROR),
                            );
                        }
                    });
                });
                ui.end_row();
                // At 29.97 and 59.94 only: timecode counted drop-frame
                // (`01:00:00;00`), which keeps to the clock.
                if self.fps().is_some_and(Rational::has_drop_frame) {
                    ui.label("Timecode");
                    ui.checkbox(&mut self.drop_frame, "Drop-frame")
                        .on_hover_text("01:00:00;00 is an hour of the clock: two frame numbers a minute are skipped (four at 59.94), but every tenth");
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

/// How a rate is written in the field: as people write it when that reads
/// back as the same rate, else exactly.
fn rate_text(f: Rational) -> String {
    let d = f.decimal();
    if d.parse::<Rational>() == Ok(f) {
        d
    } else {
        f.to_string()
    }
}

/// New Project's choice.
#[derive(Debug, Clone, Default)]
pub(super) struct NewProject {
    /// Set the format now rather than by the first video.
    choose: bool,
    format: Format,
}

#[derive(Default)]
pub(super) struct ProjectUi {
    new: Option<NewProject>,
    /// Project Settings while open: the fields, and what the project had
    /// when it opened.
    settings: Option<(Format, Format)>,
}

impl Workspace {
    pub(super) fn open_new_project(&mut self) {
        self.project_ui.new = Some(NewProject::default());
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
                ui.radio_value(&mut new.choose, false, "Take the format of the first video added");
                ui.radio_value(&mut new.choose, true, "Choose it now:");
                ui.add_enabled_ui(new.choose, |ui| {
                    ui.indent("format", |ui| new.format.edit(ui, true));
                });
                new.format.edit_start(ui);
                ui.label(
                    RichText::new(
                        "The size can change later; the frame rate only while the timeline is empty.",
                    )
                    .small()
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let ok = (!new.choose || new.format.fps().is_some())
                        && new.format.start().is_some();
                    if ui.add_enabled(ok, egui::Button::new("Create…")).clicked() {
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
                        if !new.format.starts_at_zero() {
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
            self.pick_project(Some(format));
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
                        .color(theme::TEXT_DIM),
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

/// `project.set`'s params for what `now` changes from `was`.
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
    // After drop-frame, which keeps the start's label (deviation X7).
    if let Some(start) = now.start().filter(|&s| Some(s) != was.start()) {
        set.insert("start_tc".into(), json!(start));
    }
    Value::Object(set)
}
