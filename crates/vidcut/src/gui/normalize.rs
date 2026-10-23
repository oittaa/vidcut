use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::ops::normalize::{PEAKS, is_peak, rule, targets, unit};
use vidcut_core::request::NormalizeMode;

use super::dialog;
use super::export::Ask;
use super::workspace::Workspace;

const MODES: [(NormalizeMode, &str); 14] = [
    (NormalizeMode::SamplePeak, "Sample Peak"),
    (NormalizeMode::TruePeak, "True Peak"),
    (NormalizeMode::Bs1770v1, "ITU-R BS.1770-1"),
    (NormalizeMode::Bs1770v4, "ITU-R BS.1770-4"),
    (NormalizeMode::AtscA85, "ATSC A/85"),
    (NormalizeMode::EbuR128, "EBU R128"),
    (NormalizeMode::Op59, "OP-59"),
    (NormalizeMode::TrB32, "TR-B32"),
    (NormalizeMode::Agcom, "AGCOM 219/09/CSP"),
    (NormalizeMode::Netflix, "Netflix"),
    (NormalizeMode::Youtube, "YouTube"),
    (NormalizeMode::DisneyStereo, "Disney 2.0"),
    (NormalizeMode::DisneySurround, "Disney 5.1+"),
    (NormalizeMode::TencentVideo, "Tencent Video"),
];

fn label(mode: NormalizeMode) -> &'static str {
    MODES
        .iter()
        .find(|(m, _)| *m == mode)
        .map_or("", |(_, l)| *l)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Choice {
    mode: NormalizeMode,
    target: f64,
    limit: f64,
    relative: bool,
}

impl Choice {
    fn of(mode: NormalizeMode, relative: bool) -> Self {
        let r = rule(mode);
        Self {
            mode,
            target: r.target,
            limit: r.limit.unwrap_or(-1.0),
            relative,
        }
    }
}

#[derive(Debug)]
pub struct NormalizeDialog {
    clips: Vec<String>,
    choice: Choice,
    asked: Option<Ask>,
    error: Option<String>,
}

impl NormalizeDialog {
    fn new(clips: Vec<String>, last: Option<Choice>) -> Self {
        Self {
            clips,
            choice: last.unwrap_or_else(|| Choice::of(NormalizeMode::default(), false)),
            asked: None,
            error: None,
        }
    }

    fn params(&self) -> Value {
        let c = &self.choice;
        let r = rule(c.mode);
        let mut p = json!({
            "clips": self.clips,
            "mode": c.mode,
            "relative": c.relative,
        });
        if r.free_target {
            p["target"] = json!(c.target);
        }
        if r.free_limit {
            p["peak_limit"] = json!(c.limit);
        }
        p
    }
}

fn count(result: &Value, key: &str) -> usize {
    result["clips"]
        .as_array()
        .map_or(0, |c| c.iter().filter(|c| c[key] == true).count())
}

fn level(
    ui: &mut egui::Ui,
    value: &mut f64,
    free: bool,
    range: std::ops::RangeInclusive<f64>,
    unit: &str,
) {
    ui.add_enabled(
        free,
        egui::DragValue::new(value)
            .update_while_editing(false)
            .range(range)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(format!(" {unit}")),
    );
}

impl Workspace {
    pub(super) fn open_normalize(&mut self, clips: Vec<String>) {
        if clips.is_empty() {
            return;
        }
        let last = self.timeline.normalize_last;
        self.timeline.normalize = Some(NormalizeDialog::new(clips, last));
    }

    pub(super) fn normalize_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.normalize.take() else {
            return;
        };
        if let Some(result) = d.asked.as_ref().and_then(Ask::take) {
            d.asked = None;
            match result {
                Ok(r) => {
                    self.timeline.normalize_last = Some(d.choice);
                    let (held, limited) = (count(&r.result, "held"), count(&r.result, "limited"));
                    let text = match (held, limited) {
                        (0, 0) => None,
                        (n, 0) => Some(format!("{n} of the clips stopped at the volume limit")),
                        (_, n) => Some(format!("{n} of the clips stopped at the peak limit")),
                    };
                    if let Some(text) = text {
                        self.flash = Some((text, std::time::Instant::now()));
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
                        egui::ComboBox::from_id_salt("normalize-mode")
                            .selected_text(label(d.choice.mode))
                            .show_ui(ui, |ui| {
                                for (mode, text) in MODES {
                                    if ui.selectable_label(d.choice.mode == mode, text).clicked()
                                        && d.choice.mode != mode
                                    {
                                        d.choice = Choice::of(mode, d.choice.relative);
                                    }
                                }
                            });
                    });
                    let (mode, r) = (d.choice.mode, rule(d.choice.mode));
                    let name = if is_peak(mode) { "Target" } else { "Loudness" };
                    dialog::row(ui, name, |ui| {
                        level(
                            ui,
                            &mut d.choice.target,
                            r.free_target,
                            targets(mode),
                            unit(mode),
                        );
                    });
                    if r.limit.is_some() {
                        dialog::row(ui, "Peak Limit", |ui| {
                            level(ui, &mut d.choice.limit, r.free_limit, PEAKS, "dBTP");
                        });
                    }
                    if d.clips.len() > 1 {
                        dialog::row(ui, "Set Level", |ui| {
                            dialog::switch(ui, |ui| {
                                ui.selectable_value(&mut d.choice.relative, true, "Relative");
                                ui.selectable_value(&mut d.choice.relative, false, "Independent");
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
