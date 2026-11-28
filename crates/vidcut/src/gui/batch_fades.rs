use eframe::egui::{self, Color32, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};
use vidcut_core::fades::{SHAPES, shaped};

use super::dialog;
use super::export::Ask;
use super::theme;
use super::workspace::Workspace;

const LINEAR: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Side {
    on: bool,
    shape: usize,
    frames: i64,
    overwrite: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cross {
    on: bool,
    gain: bool,
    frames: i64,
    overwrite: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    fade_in: Side,
    cross: Cross,
    fade_out: Side,
}

impl Default for Settings {
    fn default() -> Self {
        let side = Side {
            on: false,
            shape: LINEAR,
            frames: 1,
            overwrite: false,
        };
        Self {
            fade_in: side,
            cross: Cross {
                on: false,
                gain: false,
                frames: 2,
                overwrite: false,
            },
            fade_out: side,
        }
    }
}

#[derive(Debug)]
pub struct BatchDialog {
    clips: Vec<String>,
    settings: Settings,
    asked: Option<Ask>,
    error: Option<String>,
}

fn side(s: Side, falling: bool) -> Value {
    if !s.on {
        return Value::Null;
    }
    let f = shaped(s.frames as f64, s.shape, falling);
    json!({
        "frames": s.frames,
        "x_level": f.map(|f| f.x_level),
        "x_point": f.map(|f| f.x_point),
        "overwrite": s.overwrite,
    })
}

impl Settings {
    fn params(&self, clips: &[String]) -> Value {
        let c = self.cross;
        let mut p = json!({
            "clips": clips,
            "fade_in": side(self.fade_in, false),
            "fade_out": side(self.fade_out, true),
        });
        if c.on {
            p["cross_fade"] = json!({
                "frames": c.frames,
                "kind": if c.gain { "equal_gain" } else { "equal_power" },
                "overwrite": c.overwrite,
            });
        }
        p
    }

    fn any(&self) -> bool {
        self.fade_in.on || self.fade_out.on || self.cross.on
    }
}

fn shape_button(ui: &mut egui::Ui, shape: usize, falling: bool, chosen: bool) -> bool {
    let (rect, r) = ui.allocate_exact_size(vec2(26.0, 18.0), Sense::click());
    let colors = theme::colors();
    let painter = ui.painter_at(rect);
    let fill = if chosen {
        colors.accent.gamma_multiply(0.35)
    } else if r.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.inactive.bg_fill
    };
    painter.rect_filled(rect, 2.0, fill);
    if let Some(f) = shaped(1.0, shape, falling) {
        let inner = rect.shrink(3.0);
        let points: Vec<egui::Pos2> = (0..=16)
            .map(|i| {
                let u = f64::from(i) / 16.0;
                let g = f.along(u, falling) as f32;
                pos2(
                    inner.left() + inner.width() * u as f32,
                    inner.bottom() - inner.height() * g,
                )
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(1.5, Color32::WHITE)));
    }
    r.clicked()
}

fn side_rows(ui: &mut egui::Ui, name: &str, s: &mut Side, falling: bool) {
    dialog::row(ui, name, |ui| ui.checkbox(&mut s.on, ""));
    ui.add_enabled_ui(s.on, |ui| {
        dialog::row(ui, "Shape", |ui| {
            ui.horizontal(|ui| {
                for k in 0..SHAPES.len() {
                    if shape_button(ui, k, falling, s.shape == k) {
                        s.shape = k;
                    }
                }
            })
        });
        dialog::row(ui, "Length", |ui| {
            ui.add(
                egui::DragValue::new(&mut s.frames)
                    .range(1..=100_000)
                    .suffix(" frames"),
            )
        });
        dialog::row(ui, "Overwrite Existing", |ui| {
            ui.checkbox(&mut s.overwrite, "")
        });
    });
}

fn told(result: &Value) -> Option<String> {
    let skipped = result["skipped"].as_array().map_or(0, Vec::len);
    (skipped > 0).then(|| format!("{skipped} ends kept: an existing fade or no room"))
}

impl Workspace {
    pub(super) fn open_batch_fades(&mut self, clips: Vec<String>) {
        if clips.is_empty() {
            return;
        }
        self.timeline.batch_fades = Some(BatchDialog {
            clips,
            settings: self.timeline.batch_last.unwrap_or_default(),
            asked: None,
            error: None,
        });
    }

    pub(super) fn batch_fades_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.batch_fades.take() else {
            return;
        };
        if let Some(result) = d.asked.as_ref().and_then(Ask::take) {
            d.asked = None;
            match result {
                Ok(r) => {
                    self.timeline.batch_last = Some(d.settings);
                    if let Some(text) = told(&r.result) {
                        self.flash = Some((text, std::time::Instant::now()));
                    }
                    return;
                }
                Err(e) => d.error = Some(e.message),
            }
        }
        let busy = d.asked.is_some();
        let mut open = true;
        let mut done = false;
        dialog::window(ctx, "Batch Fades", &mut open, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                dialog::form(ui, "batch-fades-form", |ui| {
                    side_rows(ui, "Fade In", &mut d.settings.fade_in, false);
                    let c = &mut d.settings.cross;
                    dialog::row(ui, "Crossfade", |ui| ui.checkbox(&mut c.on, ""));
                    ui.add_enabled_ui(c.on, |ui| {
                        dialog::row(ui, "Level", |ui| {
                            dialog::switch(ui, |ui| {
                                ui.selectable_value(&mut c.gain, false, "Equal Power");
                                ui.selectable_value(&mut c.gain, true, "Equal Gain");
                            })
                        });
                        dialog::row(ui, "Length", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut c.frames)
                                    .range(1..=100_000)
                                    .suffix(" frames"),
                            )
                        });
                        dialog::row(ui, "Overwrite Existing", |ui| {
                            ui.checkbox(&mut c.overwrite, "")
                        });
                    });
                    side_rows(ui, "Fade Out", &mut d.settings.fade_out, true);
                });
            });
            if let Some(e) = &d.error {
                dialog::warn(ui, e.clone());
            }
            dialog::footer(ui, |ui| {
                if dialog::main_button(ui, "Apply", !busy && d.settings.any()) {
                    d.error = None;
                    let params = d.settings.params(&d.clips);
                    d.asked = Some(Ask::send(&self.live, ctx, "clip.batch_fades", params));
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
                if ui.button("Reset").clicked() {
                    d.settings = Settings::default();
                }
            });
        });
        if !open || done {
            return;
        }
        self.timeline.batch_fades = Some(d);
    }
}
