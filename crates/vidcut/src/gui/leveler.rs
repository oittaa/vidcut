use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::leveler::{self, OUTPUT_DB, PRESETS, label};
use vidcut_core::model::DialogueLeveler;

use super::dialog;
use super::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct LevelerDialog {
    clips: Vec<String>,
    leveler: DialogueLeveler,
}

impl LevelerDialog {
    fn params(&self) -> Value {
        let l = &self.leveler;
        json!({
            "clips": self.clips,
            "dialogue_leveler": {
                "on": l.on,
                "preset": l.preset,
                "reduce_loud": l.reduce_loud,
                "lift_soft": l.lift_soft,
                "background_reduction": l.background_reduction,
                "output_gain_db": l.output_gain_db,
            },
        })
    }
}

impl Workspace {
    pub(super) fn open_leveler(&mut self, clips: Vec<String>) {
        let stored = self.project_shared().and_then(|p| {
            let clip = p.clip(clips.first()?)?;
            clip.audio.as_ref()?.dialogue_leveler
        });
        let leveler = stored.unwrap_or_else(leveler::first);
        self.timeline.leveler = Some(LevelerDialog { clips, leveler });
    }

    pub(super) fn leveler_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.leveler.take() else {
            return;
        };
        let (mut open, mut done, mut apply) = (true, false, false);
        dialog::window(ctx, "Dialogue Leveler", &mut open, |ui| {
            let l = &mut d.leveler;
            dialog::form(ui, "leveler-form", |ui| {
                dialog::row(ui, "On", |ui| ui.checkbox(&mut l.on, ""));
                dialog::row(ui, "Preset", |ui| {
                    egui::ComboBox::from_id_salt("leveler-preset")
                        .selected_text(label(l.preset))
                        .show_ui(ui, |ui| {
                            for preset in PRESETS {
                                ui.selectable_value(&mut l.preset, preset, label(preset));
                            }
                        });
                });
                dialog::row(ui, "Reduce Loud", |ui| ui.checkbox(&mut l.reduce_loud, ""));
                dialog::row(ui, "Lift Soft", |ui| ui.checkbox(&mut l.lift_soft, ""));
                dialog::row(ui, "Background", |ui| {
                    ui.checkbox(&mut l.background_reduction, "")
                });
                dialog::row(ui, "Output Gain", |ui| {
                    ui.add_enabled(
                        l.reduce_loud,
                        egui::DragValue::new(&mut l.output_gain_db)
                            .update_while_editing(false)
                            .range(OUTPUT_DB)
                            .speed(0.1)
                            .fixed_decimals(1)
                            .suffix(" dB"),
                    )
                });
            });
            dialog::footer(ui, |ui| {
                apply = dialog::main_button(ui, "Apply", true);
                done = ui.button("Cancel").clicked();
            });
        });
        if apply {
            self.send("clip.set", d.params());
        }
        if open && !done && !apply {
            self.timeline.leveler = Some(d);
        }
    }
}
