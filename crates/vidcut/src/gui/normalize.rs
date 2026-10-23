use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::ops::normalize::{targets, unit, usual_target};
use vidcut_core::request::NormalizeMode;

use super::dialog;
use super::export::Ask;
use super::workspace::Workspace;

const MODES: [(NormalizeMode, &str); 3] = [
    (NormalizeMode::Loudness, "Loudness"),
    (NormalizeMode::SamplePeak, "Sample Peak"),
    (NormalizeMode::TruePeak, "True Peak"),
];

#[derive(Debug)]
pub struct NormalizeDialog {
    clips: Vec<String>,
    mode: NormalizeMode,
    target: f64,
    relative: bool,
    asked: Option<Ask>,
    error: Option<String>,
}

impl NormalizeDialog {
    fn new(clips: Vec<String>) -> Self {
        let mode = NormalizeMode::default();
        Self {
            clips,
            mode,
            target: usual_target(mode),
            relative: false,
            asked: None,
            error: None,
        }
    }

    fn params(&self) -> Value {
        json!({
            "clips": self.clips,
            "mode": self.mode,
            "target": self.target,
            "relative": self.relative,
        })
    }
}

fn held(result: &Value) -> usize {
    result["clips"]
        .as_array()
        .map_or(0, |c| c.iter().filter(|c| c["held"] == true).count())
}

impl Workspace {
    pub(super) fn open_normalize(&mut self, clips: Vec<String>) {
        if clips.is_empty() {
            return;
        }
        self.timeline.normalize = Some(NormalizeDialog::new(clips));
    }

    pub(super) fn normalize_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.normalize.take() else {
            return;
        };
        if let Some(result) = d.asked.as_ref().and_then(Ask::take) {
            d.asked = None;
            match result {
                Ok(r) => {
                    let n = held(&r.result);
                    if n > 0 {
                        self.flash = Some((
                            format!("{n} of the clips stopped at the volume limit"),
                            std::time::Instant::now(),
                        ));
                    }
                    return;
                }
                Err(e) => d.error = Some(e.message),
            }
        }
        let measuring = d.asked.is_some();
        let mut open = true;
        let mut done = false;
        dialog::window(ctx, "Normalize Audio Levels", &mut open, |ui| {
            ui.add_enabled_ui(!measuring, |ui| {
                dialog::form(ui, "normalize-form", |ui| {
                    dialog::row(ui, "Clips", |ui| ui.label(d.clips.len().to_string()));
                    dialog::row(ui, "Mode", |ui| {
                        dialog::switch(ui, |ui| {
                            for (mode, label) in MODES {
                                if ui.selectable_label(d.mode == mode, label).clicked()
                                    && d.mode != mode
                                {
                                    d.mode = mode;
                                    d.target = usual_target(mode);
                                }
                            }
                        });
                    });
                    dialog::row(ui, "Target", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut d.target)
                                .update_while_editing(false)
                                .range(targets(d.mode))
                                .speed(0.1)
                                .fixed_decimals(1)
                                .suffix(format!(" {}", unit(d.mode))),
                        );
                    });
                    if d.clips.len() > 1 {
                        dialog::row(ui, "Set Level", |ui| {
                            dialog::switch(ui, |ui| {
                                ui.selectable_value(&mut d.relative, true, "Relative");
                                ui.selectable_value(&mut d.relative, false, "Independent");
                            });
                        });
                    }
                });
            });
            if measuring {
                ui.horizontal(|ui| {
                    ui.spinner();
                    dialog::note(ui, "Measuring…");
                });
            }
            if let Some(e) = &d.error {
                dialog::warn(ui, e.clone());
            }
            dialog::footer(ui, |ui| {
                let go = dialog::main_button(ui, "Normalize", !measuring)
                    || (!measuring && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if go {
                    d.error = None;
                    d.asked = Some(Ask::send(&self.live, ctx, "clip.normalize", d.params()));
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
        });
        if !open || done {
            if let Some(a) = &d.asked {
                a.stop();
            }
            return;
        }
        self.timeline.normalize = Some(d);
    }
}
