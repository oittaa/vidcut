use eframe::egui::{self, Align, Layout, RichText};
use serde_json::{Value, json};
use vidcut_core::leveler::{OUTPUT_DB, PRESETS, label as preset_label};
use vidcut_core::model::{DialogueLeveler, Project};

use super::inspector::{label, switched_group};
use super::workspace::{Workspace, dim};

const NAME: &str = "Dialogue Leveler";

#[derive(Debug, Clone)]
pub struct GainDrag {
    clips: Vec<String>,
    value: f64,
}

fn header(count: usize) -> impl FnOnce(&mut egui::Ui) {
    move |ui| {
        if count > 1 {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(dim(format!("{count} clips")));
            });
        }
    }
}

fn switched_off(ui: &mut egui::Ui, count: usize) -> bool {
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().indent);
        let mut on = false;
        let flipped = ui.checkbox(&mut on, "").clicked();
        ui.label(RichText::new(NAME).strong());
        header(count)(ui);
        flipped
    })
    .inner
}

fn rows(
    ui: &mut egui::Ui,
    l: DialogueLeveler,
    clips: &[String],
    drag: &mut Option<GainDrag>,
) -> Option<Value> {
    let mut put = None;
    label(ui, "Mode", false);
    egui::ComboBox::from_id_salt("inspector-leveler-mode")
        .selected_text(preset_label(l.preset))
        .show_ui(ui, |ui| {
            for preset in PRESETS {
                let picked = ui.selectable_label(l.preset == preset, preset_label(preset));
                if picked.clicked() && l.preset != preset {
                    put = Some(json!({ "preset": preset }));
                }
            }
        });
    ui.end_row();
    for (name, key, now) in [
        ("Reduce Loud Dialogue", "reduce_loud", l.reduce_loud),
        ("Lift Soft Dialogue", "lift_soft", l.lift_soft),
        (
            "Reduce Background",
            "background_reduction",
            l.background_reduction,
        ),
    ] {
        label(ui, name, false);
        let mut on = now;
        if ui.checkbox(&mut on, "").clicked() {
            put = Some(json!({ key: on }));
        }
        ui.end_row();
    }
    label(ui, "Output Gain", false);
    let mut value = match drag {
        Some(d) if d.clips == clips => d.value,
        _ => l.output_gain_db,
    };
    let r = ui.add_enabled(
        l.reduce_loud,
        egui::DragValue::new(&mut value)
            .update_while_editing(false)
            .range(OUTPUT_DB)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" dB"),
    );
    if r.changed() {
        *drag = Some(GainDrag {
            clips: clips.to_vec(),
            value,
        });
    }
    if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged()))
        && let Some(d) = drag.take()
        && d.value != l.output_gain_db
    {
        put = Some(json!({ "output_gain_db": d.value }));
    }
    ui.end_row();
    put
}

impl Workspace {
    pub(super) fn leveler_section(&mut self, ui: &mut egui::Ui, p: &Project, clips: &[String]) {
        let shown = clips
            .first()
            .and_then(|id| p.clip(id))
            .and_then(|c| c.audio.as_ref())
            .and_then(|a| a.dialogue_leveler)
            .filter(|l| l.on);
        let count = clips.len();
        let put = match shown {
            Some(l) => {
                let drag = &mut self.timeline.leveler;
                let mut put = None;
                let (flipped, ()) = switched_group(ui, NAME, Some(true), header(count), |ui| {
                    put = rows(ui, l, clips, drag);
                });
                flipped.map(|on| json!({ "on": on })).or(put)
            }
            None => switched_off(ui, count).then(|| json!({ "on": true })),
        };
        if let Some(change) = put {
            self.send(
                "clip.set",
                json!({ "clips": clips, "dialogue_leveler": change }),
            );
        }
    }
}
