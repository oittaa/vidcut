use std::ops::RangeInclusive;

use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::compressor::{
    self, ATTACK_MS, HOLD_MS, KNEE, MAKEUP_DB, MIX, RATIO, RELEASE_MS, THRESHOLD_DB,
};
use vidcut_core::model::{ClipCompressor, Compressor};

use super::dialog;
use super::equalizer::number;
use super::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct CompressorDialog {
    clips: Vec<String>,
    compressor: ClipCompressor,
}

impl CompressorDialog {
    fn params(&self) -> Value {
        json!({ "clips": self.clips, "compressor": self.compressor })
    }
}

pub(super) fn value_row(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut f64,
    range: RangeInclusive<f64>,
    decimals: usize,
    unit: &str,
) -> bool {
    value_row_if(ui, true, name, value, range, decimals, unit)
}

pub(super) fn value_row_if(
    ui: &mut egui::Ui,
    free: bool,
    name: &str,
    value: &mut f64,
    range: RangeInclusive<f64>,
    decimals: usize,
    unit: &str,
) -> bool {
    let speed = (range.end() - range.start()) / 300.0;
    dialog::row(ui, name, |ui| {
        number(ui, free, value, range, speed, decimals, unit)
    })
}

pub(super) fn compressor_rows(ui: &mut egui::Ui, free: bool, c: &mut Compressor) -> bool {
    let t = &mut c.threshold_db;
    let mut held = value_row_if(ui, free, "Threshold", t, THRESHOLD_DB, 1, " dB");
    held |= value_row_if(ui, free, "Ratio", &mut c.ratio, RATIO, 1, ":1");
    held |= value_row_if(ui, free, "Knee", &mut c.knee, KNEE, 0, "");
    held |= value_row_if(ui, free, "Mix", &mut c.mix, MIX, 0, "");
    held |= value_row_if(ui, free, "Attack", &mut c.attack_ms, ATTACK_MS, 1, " ms");
    held |= value_row_if(ui, free, "Hold", &mut c.hold_ms, HOLD_MS, 0, " ms");
    held |= value_row_if(ui, free, "Release", &mut c.release_ms, RELEASE_MS, 0, " ms");
    held
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
                dialog::row(ui, "On", |ui| ui.checkbox(&mut c.compressor.on, ""));
                compressor_rows(ui, true, &mut c.compressor);
                value_row(ui, "Makeup Gain", &mut c.makeup_db, MAKEUP_DB, 1, " dB");
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
