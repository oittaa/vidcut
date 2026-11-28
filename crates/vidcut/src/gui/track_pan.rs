use eframe::egui::{self, Sense, Stroke, pos2, vec2};
use serde_json::json;
use vidcut_core::model::Track;
use vidcut_core::validate::PAN;

use super::dialog;
use super::theme;
use super::workspace::Workspace;

const WIDTH: f32 = 340.0;
const SLIDER_WIDTH: f32 = 150.0;
const FIELD_WIDTH: f32 = 52.0;
const DOT: f32 = 3.0;

#[derive(Debug, Clone, Default)]
pub struct TrackPanUi {
    open: Option<String>,
    dragging: Option<(String, f64)>,
}

fn pan_text(pan: f64) -> String {
    let size = (pan.abs() * 10.0).round() / 10.0;
    if size == 0.0 {
        "C".into()
    } else {
        format!("{size}{}", if pan < 0.0 { "L" } else { "R" })
    }
}

fn typed_pan(text: &str) -> Option<f64> {
    let text = text.trim().to_ascii_uppercase();
    if text == "C" {
        return Some(0.0);
    }
    let (size, side) = match text.strip_suffix('L') {
        Some(size) => (size, -1.0),
        None => (text.strip_suffix('R')?, 1.0),
    };
    let size = size
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)?;
    Some(side * size.min(*PAN.end()))
}

fn pan_field(ui: &mut egui::Ui, value: f64) -> Option<f64> {
    let now = pan_text(value);
    let id = ui.make_persistent_id("track-pan-left-right");
    let typed = ui.data_mut(|d| d.get_temp::<String>(id));
    let mut text = typed.unwrap_or_else(|| now.clone());
    let field = ui.add(egui::TextEdit::singleline(&mut text).desired_width(FIELD_WIDTH));
    if field.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    (field.lost_focus() && text.trim() != now)
        .then(|| typed_pan(&text))
        .flatten()
}

impl Workspace {
    pub(super) fn open_track_pan(&mut self, id: &str) -> Result<(), String> {
        let p = self.project_shared().ok_or("Open a project first")?;
        let t = p.track(id).ok_or_else(|| format!("no track {id}"))?;
        if !t.has_sound() {
            return Err(format!("{id} has no sound"));
        }
        self.track_pan = TrackPanUi {
            open: Some(id.to_string()),
            dragging: None,
        };
        Ok(())
    }

    pub(super) fn close_track_pan(&mut self) {
        self.track_pan = TrackPanUi::default();
    }

    pub(super) fn track_pan_open(&self) -> bool {
        self.track_pan.open.is_some()
    }

    fn shown_pan(&self, t: &Track) -> f64 {
        self.track_pan
            .dragging
            .as_ref()
            .filter(|(id, _)| *id == t.id)
            .map_or(t.pan, |(_, v)| *v)
    }

    fn put_pan(&mut self, t: &Track, pan: f64) {
        self.track_pan.dragging = None;
        if pan != t.pan {
            self.send("track.set", json!({ "track": t.id, "pan": pan }));
        }
    }

    pub(super) fn pan_controls(&mut self, ui: &mut egui::Ui, t: &Track) {
        let pan = self.shown_pan(t);
        ui.horizontal(|ui| {
            ui.label("Pan");
            let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
            let (rect, r) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let c = theme::colors();
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
            painter.vline(rect.center().x, rect.y_range(), Stroke::new(1.0, c.line));
            let half = (rect.width() / 2.0 - DOT).max(1.0);
            let x = rect.center().x + half * (pan / *PAN.end()) as f32;
            let color = if t.pan_on { c.selection } else { c.text_dim };
            painter.circle_filled(pos2(x, rect.center().y), DOT, color);
            let r = r.on_hover_text("Drag To Pan, Double-Click To Open");
            if r.double_clicked() {
                self.track_pan.dragging = None;
                let _ = self.open_track_pan(&t.id);
            } else if r.dragged() {
                let moved = f64::from(r.drag_delta().x / half) * *PAN.end();
                let to = (pan + moved).clamp(*PAN.start(), *PAN.end());
                self.track_pan.dragging = Some((t.id.clone(), to));
            }
            if r.drag_stopped() {
                self.put_pan(t, self.shown_pan(t).round());
            }
        });
    }

    pub(super) fn track_pan_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.track_pan.open.clone() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(t) = p.track(&id).filter(|t| t.has_sound()) else {
            self.track_pan = TrackPanUi::default();
            return;
        };
        let name = t.name.clone().unwrap_or_else(|| t.id.clone());
        let mut on = t.pan_on;
        let mut value = self.shown_pan(t);
        let (mut sliding, mut slid, mut typed) = (false, false, None);
        let (mut reset, mut close, mut open) = (false, false, true);
        let title = format!("Pan - {name}");
        dialog::window_of(ctx, &title, &mut open, WIDTH, |ui| {
            dialog::form(ui, "track-pan-form", |ui| {
                dialog::row(ui, "Pan", |ui| {
                    dialog::switch(ui, |ui| {
                        ui.selectable_value(&mut on, false, "Off");
                        ui.selectable_value(&mut on, true, "On");
                    });
                });
                dialog::row(ui, "Left/Right", |ui| {
                    ui.add_enabled_ui(on, |ui| {
                        ui.spacing_mut().slider_width = SLIDER_WIDTH;
                        let r = ui.add(
                            egui::Slider::new(&mut value, PAN)
                                .show_value(false)
                                .step_by(1.0),
                        );
                        sliding = r.dragged();
                        slid = r.drag_stopped() || (r.changed() && !r.dragged());
                        typed = pan_field(ui, value);
                    });
                });
            });
            dialog::footer(ui, |ui| {
                close = dialog::main_button(ui, "Close", true);
                let changed = t.pan != 0.0 || !t.pan_on;
                reset = ui
                    .add_enabled(changed, egui::Button::new("Reset"))
                    .clicked();
            });
        });
        if reset {
            self.track_pan.dragging = None;
            self.send(
                "track.set",
                json!({ "track": id, "pan": 0.0, "pan_on": true }),
            );
        } else {
            if on != t.pan_on {
                self.send("track.set", json!({ "track": id, "pan_on": on }));
            }
            if let Some(v) = typed {
                self.put_pan(t, v);
            } else if slid {
                self.put_pan(t, value);
            } else if sliding {
                self.track_pan.dragging = Some((id.clone(), value));
            }
        }
        if !open || close {
            self.track_pan = TrackPanUi::default();
        }
    }
}
