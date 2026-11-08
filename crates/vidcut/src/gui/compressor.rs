use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::compressor::{
    self, ATTACK_MS, HOLD_MS, KNEE_DB, MAKEUP_DB, MIX, RATIO, RELEASE_MS, THRESHOLD_DB,
};
use vidcut_core::model::Compressor;

use super::dialog;
use super::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct CompressorDialog {
    clips: Vec<String>,
    compressor: Compressor,
}

impl CompressorDialog {
    fn params(&self) -> Value {
        let c = &self.compressor;
        json!({
            "clips": self.clips,
            "compressor": {
                "on": c.on,
                "threshold_db": c.threshold_db,
                "ratio": c.ratio,
                "knee_db": c.knee_db,
                "mix": c.mix,
                "attack_ms": c.attack_ms,
                "hold_ms": c.hold_ms,
                "release_ms": c.release_ms,
                "makeup_db": c.makeup_db,
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
    pub(super) fn open_compressor(&mut self, clips: Vec<String>) {
        let stored = self.project_shared().and_then(|p| {
            let clip = p.clip(clips.first()?)?;
            clip.audio.as_ref()?.compressor
        });
        let compressor = stored.unwrap_or_else(compressor::first);
        self.timeline.compressor = Some(CompressorDialog { clips, compressor });
    }

    pub(super) fn compressor_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.compressor.take() else {
            return;
        };
        let (mut open, mut done, mut apply) = (true, false, false);
        dialog::window(ctx, "Compressor", &mut open, |ui| {
            let c = &mut d.compressor;
            dialog::form(ui, "compressor-form", |ui| {
                dialog::row(ui, "On", |ui| ui.checkbox(&mut c.on, ""));
                number(ui, "Threshold", &mut c.threshold_db, THRESHOLD_DB, 1, " dB");
                number(ui, "Ratio", &mut c.ratio, RATIO, 1, ":1");
                number(ui, "Knee", &mut c.knee_db, KNEE_DB, 1, " dB");
                number(ui, "Mix", &mut c.mix, MIX, 0, " %");
                number(ui, "Attack", &mut c.attack_ms, ATTACK_MS, 1, " ms");
                number(ui, "Hold", &mut c.hold_ms, HOLD_MS, 0, " ms");
                number(ui, "Release", &mut c.release_ms, RELEASE_MS, 0, " ms");
                number(ui, "Makeup Gain", &mut c.makeup_db, MAKEUP_DB, 1, " dB");
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
            self.timeline.compressor = Some(d);
        }
    }
}
