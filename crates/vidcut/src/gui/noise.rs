use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::model::NoiseReduction;
use vidcut_core::noise_reduction::{
    self, ATTACK_MS, DRY_WET, FREQUENCY_SMOOTHING, LEVEL_DB, RATIO, SENSITIVITY_DB, THRESHOLD_DB,
};

use super::dialog;
use super::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct NoiseDialog {
    clips: Vec<String>,
    settings: NoiseReduction,
}

impl NoiseDialog {
    fn params(&self) -> Value {
        let n = &self.settings;
        json!({
            "clips": self.clips,
            "noise_reduction": {
                "on": n.on,
                "threshold_db": n.threshold_db,
                "attack_ms": n.attack_ms,
                "sensitivity_db": n.sensitivity_db,
                "ratio": n.ratio,
                "frequency_smoothing": n.frequency_smoothing,
                "time_smoothing": n.time_smoothing,
                "dry_wet": n.dry_wet,
                "level_db": n.level_db,
            },
        })
    }
}

fn number(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    decimals: usize,
    unit: &str,
) {
    let speed = (range.end() - range.start()) / 300.0;
    dialog::row(ui, name, |ui| {
        ui.add(
            egui::DragValue::new(value)
                .update_while_editing(false)
                .range(range)
                .speed(speed)
                .fixed_decimals(decimals)
                .suffix(unit),
        )
    });
}

impl Workspace {
    pub(super) fn open_noise(&mut self, clips: Vec<String>) {
        let stored = self.project_shared().and_then(|p| {
            let clip = p.clip(clips.first()?)?;
            clip.audio.as_ref()?.noise_reduction
        });
        let settings = stored.unwrap_or_else(noise_reduction::first);
        self.timeline.noise = Some(NoiseDialog { clips, settings });
    }

    pub(super) fn noise_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.noise.take() else {
            return;
        };
        let (mut open, mut done, mut apply) = (true, false, false);
        dialog::window(ctx, "Noise Reduction", &mut open, |ui| {
            let n = &mut d.settings;
            dialog::form(ui, "noise-form", |ui| {
                dialog::row(ui, "On", |ui| ui.checkbox(&mut n.on, ""));
                number(ui, "Threshold", &mut n.threshold_db, THRESHOLD_DB, 1, " dB");
                number(ui, "Attack", &mut n.attack_ms, ATTACK_MS, 0, " ms");
                number(
                    ui,
                    "Sensitivity",
                    &mut n.sensitivity_db,
                    SENSITIVITY_DB,
                    1,
                    " dB",
                );
                number(ui, "Ratio", &mut n.ratio, RATIO, 1, "");
                number(
                    ui,
                    "Frequency Smoothing",
                    &mut n.frequency_smoothing,
                    FREQUENCY_SMOOTHING,
                    0,
                    " %",
                );
                dialog::row(ui, "Time Smoothing", |ui| {
                    ui.checkbox(&mut n.time_smoothing, "")
                });
                number(ui, "Dry/Wet", &mut n.dry_wet, DRY_WET, 0, " %");
                number(ui, "Level", &mut n.level_db, LEVEL_DB, 1, " dB");
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
            self.timeline.noise = Some(d);
        }
    }
}
