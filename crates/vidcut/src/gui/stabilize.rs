use eframe::egui::{self, Align, Layout};
use serde_json::{Map, Value, json};
use vidcut_core::model::{Project, Stabilization, StabilizeMode, Stabilizing};

use super::dialog;
use super::inspector::{label, switched_group};
use super::workspace::Workspace;

const NAME: &str = "Stabilization";

#[derive(Debug, Clone)]
pub struct Drag {
    clips: Vec<String>,
    key: &'static str,
    value: f64,
}

fn of(p: &Project, id: &str) -> Stabilization {
    let c = p.clip(id);
    let picture = c.and_then(|c| c.picture.as_ref());
    picture
        .and_then(|x| x.stabilization.clone())
        .unwrap_or_default()
}

fn takes(p: &Project, id: &str) -> bool {
    p.find_clip(id).is_some_and(|(ti, ci)| {
        let c = &p.tracks[ti].clips[ci];
        vidcut_core::ops::picture::footage(p, ti, c).is_ok()
    })
}

fn number(
    ui: &mut egui::Ui,
    (name, key, usual): (&str, &'static str, f64),
    (now, (low, high), on): (f64, (f64, f64), bool),
    clips: &[String],
    drag: &mut Option<Drag>,
) -> Option<f64> {
    label(ui, name, now != usual);
    let mut value = match drag {
        Some(d) if d.clips == clips && d.key == key => d.value,
        _ => now,
    };
    let r = ui.add_enabled(
        on,
        egui::DragValue::new(&mut value)
            .update_while_editing(false)
            .range(low..=high)
            .speed(0.005)
            .fixed_decimals(3),
    );
    if r.changed() {
        *drag = Some(Drag {
            clips: clips.to_vec(),
            key,
            value,
        });
    }
    let mine = drag
        .as_ref()
        .is_some_and(|d| d.clips == clips && d.key == key);
    let mut put = None;
    if mine
        && (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged()))
        && let Some(d) = drag.take()
        && d.value != now
    {
        put = Some(d.value);
    }
    ui.end_row();
    put
}

fn rows(
    ui: &mut egui::Ui,
    v: Stabilizing,
    clips: &[String],
    drag: &mut Option<Drag>,
    put: &mut Map<String, Value>,
) {
    label(ui, "Mode", !v.mode.is_usual());
    dialog::switch(ui, |ui| {
        for m in StabilizeMode::ALL {
            let picked = ui.add(egui::Button::selectable(m == v.mode, m.label()));
            if picked.clicked() && m != v.mode {
                put.insert("mode".into(), json!(m));
            }
        }
    });
    ui.end_row();
    for (name, key, now, usual) in [
        ("Camera Lock", "camera_lock", v.camera_lock, false),
        ("Zoom", "zoom", v.zoom, true),
    ] {
        label(ui, name, now != usual);
        let mut on = now;
        if ui.checkbox(&mut on, "").clicked() {
            put.insert(key.into(), json!(on));
        }
        ui.end_row();
    }
    let free = !v.camera_lock;
    for (row, now, range) in [
        (
            (
                "Cropping Ratio",
                "cropping_ratio",
                Stabilizing::USUAL_CROPPING_RATIO,
            ),
            v.cropping_ratio(),
            Stabilizing::CROPPING_RATIO,
        ),
        (
            ("Smooth", "smooth", Stabilizing::USUAL_SMOOTH),
            v.smooth(),
            Stabilizing::SMOOTH,
        ),
        (
            ("Strength", "strength", Stabilizing::USUAL_STRENGTH),
            v.strength(),
            Stabilizing::STRENGTH,
        ),
    ] {
        if let Some(value) = number(ui, row, (now, range, free), clips, drag) {
            put.insert(row.1.into(), json!(value));
        }
    }
}

impl Workspace {
    pub(super) fn stabilization_section(&mut self, ui: &mut egui::Ui, p: &Project, ids: &[String]) {
        let clips: Vec<String> = ids.iter().filter(|id| takes(p, id)).cloned().collect();
        let Some(first) = clips.first().map(|id| of(p, id)) else {
            return;
        };
        let running = (self.snap.jobs.iter()).any(|j| j.unfinished() && j.kind == "stabilize");
        let ready = clips.iter().all(|id| of(p, id).ready());
        let set = clips.iter().any(|id| !of(p, id).is_usual());
        let drag = &mut self.timeline.steady;
        let mut put = Map::new();
        let mut go = false;
        let header = |ui: &mut egui::Ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled(set, egui::Button::new("Reset")).clicked()
            })
            .inner
        };
        let (flipped, cleared) = switched_group(ui, NAME, Some(!first.off), header, |ui| {
            rows(ui, first.values, &clips, drag, &mut put);
            label(ui, "", false);
            let button = egui::Button::new("Stabilize");
            go = ui.add_enabled(!ready && !running, button).clicked();
            ui.end_row();
        });
        if let Some(on) = flipped {
            put.insert("on".into(), json!(on));
        }
        if !put.is_empty() {
            self.send(
                "picture.set",
                json!({ "clips": clips, "stabilization": put }),
            );
        }
        if cleared {
            self.send(
                "picture.clear",
                json!({ "clips": clips, "tool": "stabilization" }),
            );
        }
        if go {
            for id in clips.iter().filter(|id| !of(p, id).ready()) {
                self.send("picture.stabilize", json!({ "clip": id }));
            }
        }
    }
}
