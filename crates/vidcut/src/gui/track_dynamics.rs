use std::time::Instant;

use eframe::egui::{self, Pos2, Rect, Sense, Shape, Stroke, vec2};
use serde::Serialize;
use serde_json::{Value, json};
use vidcut_core::compressor::{HOLD_MS, RELEASE_MS};
use vidcut_core::dynamics::{
    self, EXPANDER_ATTACK_MS, EXPANDER_RATIO, EXPANDER_THRESHOLD_DB, LIMITER_ATTACK_MS,
    LIMITER_THRESHOLD_DB, MAKEUP_DB, RANGE_DB, steady_db,
};
use vidcut_core::model::{Project, Track, TrackDynamics};

use super::compressor::{compressor_rows, value_row, value_row_if};
use super::dialog;
use super::equalizer::Draft;
use super::theme;
use super::timeline::tools::landing;
use super::workspace::{Workspace, dim};

const WIDTH: f32 = 760.0;
const GRAPH_SIZE: f32 = 150.0;
const FLOOR_DB: f64 = -60.0;

#[derive(Debug, Clone, Default)]
pub struct TrackDynamicsUi {
    open: Option<String>,
    draft: Option<Draft<TrackDynamics>>,
}

fn fields(from: &impl Serialize, to: &impl Serialize) -> Option<Value> {
    let (Value::Object(from), Value::Object(to)) = (json!(from), json!(to)) else {
        return None;
    };
    let set: serde_json::Map<String, Value> = to
        .into_iter()
        .filter(|(k, v)| from.get(k) != Some(v))
        .collect();
    (!set.is_empty()).then_some(Value::Object(set))
}

fn change(from: &TrackDynamics, to: &TrackDynamics) -> Option<Value> {
    let mut set = json!({ "on": to.on });
    let modules = [
        ("expander", fields(&from.expander, &to.expander)),
        ("compressor", fields(&from.compressor, &to.compressor)),
        ("limiter", fields(&from.limiter, &to.limiter)),
    ];
    for (name, changed) in modules {
        if let Some(changed) = changed {
            set[name] = changed;
        }
    }
    if from.makeup_db != to.makeup_db {
        set["makeup_db"] = json!(to.makeup_db);
    }
    (from != to).then_some(set)
}

fn plot(ui: &egui::Ui, rect: Rect, d: &TrackDynamics) {
    let c = theme::colors();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let inner = rect.shrink(1.0);
    let at = |input: f64, output: f64| {
        let x = (input - FLOOR_DB) / -FLOOR_DB;
        let y = (output - FLOOR_DB) / -FLOOR_DB;
        Pos2::new(
            inner.left() + x as f32 * inner.width(),
            inner.bottom() - y as f32 * inner.height(),
        )
    };
    painter.line_segment(
        [at(FLOOR_DB, FLOOR_DB), at(0.0, 0.0)],
        Stroke::new(1.0, c.line),
    );
    let steps = inner.width().max(2.0) as usize;
    let points = (0..=steps)
        .map(|i| {
            let input = FLOOR_DB - FLOOR_DB * i as f64 / steps as f64;
            let output = if d.on { steady_db(d, input) } else { input };
            at(input, output)
        })
        .collect();
    let color = if d.heard() { c.selection } else { c.text_dim };
    painter.add(Shape::line(points, Stroke::new(1.0, color)));
}

fn module_switch(ui: &mut egui::Ui, on: &mut bool) {
    dialog::switch(ui, |ui| {
        ui.selectable_value(on, false, "Off");
        ui.selectable_value(on, true, "On");
    });
}

fn expander_column(ui: &mut egui::Ui, d: &mut TrackDynamics) -> bool {
    let e = &mut d.expander;
    let mut held = false;
    ui.label(dim("Expander/Gate"));
    dialog::form(ui, "track-dynamics-expander", |ui| {
        dialog::row(ui, "Mode", |ui| {
            dialog::switch(ui, |ui| {
                if ui.selectable_label(!e.on, "Off").clicked() {
                    e.on = false;
                }
                if ui.selectable_label(e.on && !e.gate, "Expander").clicked() {
                    (e.on, e.gate) = (true, false);
                }
                if ui.selectable_label(e.on && e.gate, "Gate").clicked() {
                    (e.on, e.gate) = (true, true);
                }
            });
        });
        let free = e.on;
        held |= value_row_if(
            ui,
            free,
            "Threshold",
            &mut e.threshold_db,
            EXPANDER_THRESHOLD_DB,
            1,
            " dB",
        );
        held |= value_row_if(ui, free, "Range", &mut e.range_db, RANGE_DB, 1, " dB");
        held |= value_row_if(
            ui,
            free && !e.gate,
            "Ratio",
            &mut e.ratio,
            EXPANDER_RATIO,
            1,
            ":1",
        );
        held |= value_row_if(
            ui,
            free,
            "Attack",
            &mut e.attack_ms,
            EXPANDER_ATTACK_MS,
            1,
            " ms",
        );
        held |= value_row_if(ui, free, "Hold", &mut e.hold_ms, HOLD_MS, 0, " ms");
        held |= value_row_if(ui, free, "Release", &mut e.release_ms, RELEASE_MS, 0, " ms");
    });
    held
}

fn compressor_column(ui: &mut egui::Ui, d: &mut TrackDynamics) -> bool {
    let c = &mut d.compressor;
    let mut held = false;
    ui.label(dim("Compressor"));
    dialog::form(ui, "track-dynamics-compressor", |ui| {
        dialog::row(ui, "Compressor", |ui| module_switch(ui, &mut c.on));
        let free = c.on;
        held = compressor_rows(ui, free, c);
    });
    held
}

fn limiter_column(ui: &mut egui::Ui, d: &mut TrackDynamics) -> bool {
    let l = &mut d.limiter;
    let mut held = false;
    ui.label(dim("Limiter"));
    dialog::form(ui, "track-dynamics-limiter", |ui| {
        dialog::row(ui, "Limiter", |ui| module_switch(ui, &mut l.on));
        let free = l.on;
        held |= value_row_if(
            ui,
            free,
            "Threshold",
            &mut l.threshold_db,
            LIMITER_THRESHOLD_DB,
            1,
            " dB",
        );
        held |= value_row_if(
            ui,
            free,
            "Attack",
            &mut l.attack_ms,
            LIMITER_ATTACK_MS,
            2,
            " ms",
        );
        held |= value_row_if(ui, free, "Hold", &mut l.hold_ms, HOLD_MS, 0, " ms");
        held |= value_row_if(ui, free, "Release", &mut l.release_ms, RELEASE_MS, 0, " ms");
    });
    held
}

fn stored(p: &Project, id: &str) -> Option<TrackDynamics> {
    p.track(id)
        .map(|t| t.dynamics.unwrap_or_else(dynamics::first))
}

impl Workspace {
    pub(super) fn open_track_dynamics(&mut self, id: &str) -> Result<(), String> {
        let p = self.project_shared().ok_or("Open a project first")?;
        let t = p.track(id).ok_or_else(|| format!("no track {id}"))?;
        if !t.has_sound() {
            return Err(format!("{id} has no sound"));
        }
        self.track_dynamics = TrackDynamicsUi {
            open: Some(id.to_string()),
            draft: None,
        };
        Ok(())
    }

    pub(super) fn close_track_dynamics(&mut self) {
        self.track_dynamics = TrackDynamicsUi::default();
    }

    pub(super) fn track_dynamics_open(&self) -> bool {
        self.track_dynamics.open.is_some()
    }

    pub(super) fn dynamics_controls(&mut self, ui: &mut egui::Ui, t: &Track) {
        let d = t.dynamics.unwrap_or_else(dynamics::first);
        ui.horizontal(|ui| {
            ui.label("DY");
            let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
            let (rect, r) = ui.allocate_exact_size(size, Sense::click());
            plot(ui, rect, &d);
            let r = r.on_hover_text("Dynamics: Click To Bypass, Double-Click To Open");
            if r.double_clicked() {
                let _ = self.open_track_dynamics(&t.id);
            } else if r.clicked() {
                self.send(
                    "track.set",
                    json!({ "track": t.id, "dynamics": { "on": !d.on } }),
                );
            }
        });
    }

    pub(super) fn track_dynamics_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.track_dynamics.open.clone() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let (Some(stored), Some(t)) = (stored(&p, &id), p.track(&id)) else {
            self.track_dynamics = TrackDynamicsUi::default();
            return;
        };
        let draft = self.track_dynamics.draft.take().filter(|d| {
            d.owners == [id.clone()]
                && d.since
                    .is_none_or(|s| landing(&d.to, s.elapsed(), &d.from, &stored).is_some())
        });
        let shown = draft.as_ref().map_or(stored, |d| d.to);
        let mut d = shown;
        let name = t.name.clone().unwrap_or_else(|| t.id.clone());
        let mut open = true;
        let (mut held, mut reset, mut close) = (false, false, false);
        let title = format!("Dynamics - {name}");
        dialog::window_of(ctx, &title, &mut open, WIDTH, |ui| {
            ui.horizontal_top(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(GRAPH_SIZE, GRAPH_SIZE), Sense::hover());
                plot(ui, rect, &d);
                ui.vertical(|ui| {
                    dialog::form(ui, "track-dynamics-form", |ui| {
                        dialog::row(ui, "Dynamics", |ui| module_switch(ui, &mut d.on));
                        held |= value_row(ui, "Make Up", &mut d.makeup_db, MAKEUP_DB, 1, " dB");
                    });
                });
            });
            ui.add_enabled_ui(d.on, |ui| {
                ui.columns(3, |cols| {
                    held |= expander_column(&mut cols[0], &mut d);
                    held |= compressor_column(&mut cols[1], &mut d);
                    held |= limiter_column(&mut cols[2], &mut d);
                });
            });
            dialog::footer(ui, |ui| {
                close = dialog::main_button(ui, "Close", true);
                let changed = stored != dynamics::first();
                reset = ui
                    .add_enabled(changed, egui::Button::new("Reset"))
                    .clicked();
            });
        });
        let now = Some(Instant::now());
        let draft_of = |from: TrackDynamics, to: TrackDynamics, since| Draft {
            owners: vec![id.clone()],
            from,
            to,
            since,
        };
        self.track_dynamics.draft = if reset {
            self.send("track.set", json!({ "track": id, "dynamics": null }));
            Some(draft_of(stored, dynamics::first(), now))
        } else if held {
            Some(draft_of(stored, d, None))
        } else if d != shown || draft.as_ref().is_some_and(|d| d.since.is_none()) {
            match change(&stored, &d) {
                Some(set) => {
                    self.send("track.set", json!({ "track": id, "dynamics": set }));
                    Some(draft_of(stored, d, now))
                }
                None => None,
            }
        } else {
            draft
        };
        if !open || close {
            self.track_dynamics = TrackDynamicsUi::default();
        }
    }
}
