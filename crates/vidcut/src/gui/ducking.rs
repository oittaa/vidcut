use std::ops::RangeInclusive;

use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::duck;
use vidcut_core::model::{Duck, DuckDetect, Project, Track};
use vidcut_core::request::DuckSet;

use super::dialog;
use super::workspace::Workspace;

#[derive(Default)]
pub(super) struct DuckingUi {
    asking: Option<(String, Value)>,
    amount: Option<(String, f64)>,
    advanced: Option<Advanced>,
}

struct Advanced {
    track: String,
    name: String,
    detect: DuckDetect,
    threshold_db: f64,
    lookahead_ms: f64,
    rise_ms: f64,
    hold_ms: f64,
    recovery_ms: f64,
}

impl Advanced {
    fn of(t: &Track, d: &Duck) -> Self {
        Self {
            track: t.id.clone(),
            name: t.name.clone().unwrap_or_else(|| t.id.clone()),
            detect: d.detect(),
            threshold_db: d.threshold(),
            lookahead_ms: d.lookahead(),
            rise_ms: d.rise(),
            hold_ms: d.hold(),
            recovery_ms: d.recovery(),
        }
    }

    fn set(&self) -> Value {
        json!({
            "detect": self.detect,
            "threshold_db": tenths(self.threshold_db),
            "lookahead_ms": self.lookahead_ms.round(),
            "rise_ms": self.rise_ms.round(),
            "hold_ms": self.hold_ms.round(),
            "recovery_ms": self.recovery_ms.round(),
        })
    }
}

fn tenths(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn speech_needs() -> Vec<vidcut_av::models::Need> {
    vidcut_av::transcribe::needed(&crate::paths::models_dir(), &[])
}

fn by_speech(d: &Duck) -> bool {
    d.on && d.detect() == DuckDetect::Speech
}

fn named(p: &Project, id: &str) -> String {
    p.track(id)
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn sources_label(p: &Project, d: &Duck) -> String {
    let names: Vec<String> = d.sources.iter().map(|s| named(p, s)).collect();
    match names.len() {
        0 => "No Sources".into(),
        1 | 2 => format!("Under {}", names.join(", ")),
        n => format!("Under {n} Tracks"),
    }
}

fn millis(ui: &mut egui::Ui, value: &mut f64, range: RangeInclusive<f64>) -> egui::Response {
    ui.add(
        egui::DragValue::new(value)
            .update_while_editing(false)
            .range(range)
            .speed(1.0)
            .fixed_decimals(0)
            .suffix(" ms"),
    )
}

impl Workspace {
    pub(super) fn duck(&mut self, track: &str, set: Value) {
        let speech = self
            .project()
            .and_then(|p| {
                let s: DuckSet = serde_json::from_value(set.clone()).ok()?;
                duck::patched(p, track, &s).ok()
            })
            .is_some_and(|d| by_speech(&d));
        if speech && !speech_needs().is_empty() {
            self.ducking.asking = Some((track.to_string(), set));
            return;
        }
        self.send_duck(track, set, false);
    }

    fn send_duck(&self, track: &str, set: Value, download: bool) {
        let mut params = json!({ "track": track, "duck": set });
        if download {
            params["download"] = json!(true);
        }
        self.send("track.set", params);
    }

    pub(super) fn toggle_duck(&mut self, track: &str) {
        let Some(t) = self.project().and_then(|p| p.track(track)) else {
            return;
        };
        let set = match &t.duck {
            None => json!({}),
            Some(d) => json!({ "on": !d.on }),
        };
        self.duck(track, set);
    }

    pub(super) fn ducking_controls(&mut self, ui: &mut egui::Ui, p: &Project, t: &Track) {
        let mut on = t.duck.as_ref().is_some_and(|d| d.on);
        if ui.checkbox(&mut on, "Ducking").clicked() {
            let set = match &t.duck {
                None => json!({}),
                Some(_) => json!({ "on": on }),
            };
            self.duck(&t.id, set);
        }
        let Some(d) = t.duck.as_ref().filter(|d| d.on) else {
            return;
        };
        ui.menu_button(sources_label(p, d), |ui| {
            for x in p.tracks.iter().filter(|x| x.id != t.id && x.has_sound()) {
                let mut under = d.sources.contains(&x.id);
                if ui.checkbox(&mut under, named(p, &x.id)).clicked() {
                    let sources: Vec<&str> = p
                        .tracks
                        .iter()
                        .map(|y| y.id.as_str())
                        .filter(|id| match *id == x.id {
                            true => under,
                            false => d.sources.iter().any(|s| s == id),
                        })
                        .collect();
                    self.duck(&t.id, json!({ "sources": sources }));
                }
            }
        });
        let stored = d.amount();
        let mut value = match &self.ducking.amount {
            Some((id, v)) if *id == t.id => *v,
            _ => stored,
        };
        let r = ui.add(
            egui::DragValue::new(&mut value)
                .update_while_editing(false)
                .range(duck::AMOUNT)
                .speed(0.1)
                .fixed_decimals(1)
                .prefix("Amount ")
                .suffix(" dB"),
        );
        if r.changed() {
            self.ducking.amount = Some((t.id.clone(), value));
        }
        if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged()))
            && let Some((id, v)) = self.ducking.amount.take()
            && tenths(v) != stored
        {
            self.duck(&id, json!({ "amount_db": tenths(v) }));
        }
        if by_speech(d) && !speech_needs().is_empty() && ui.button("Download Speech").clicked() {
            self.ducking.asking = Some((t.id.clone(), json!({ "on": true })));
        }
        if ui.button("Advanced").clicked() {
            self.ducking.advanced = Some(Advanced::of(t, d));
        }
    }

    pub(super) fn ducking_windows(&mut self, ctx: &egui::Context) {
        if let Some((track, set)) = self.ducking.asking.clone() {
            let needs = speech_needs();
            if needs.is_empty() {
                self.ducking.asking = None;
                self.send_duck(&track, set, false);
            } else {
                match super::speech::ask_download(ctx, "ducking-download", &needs, "Download") {
                    Some(true) => {
                        self.ducking.asking = None;
                        self.send_duck(&track, set, true);
                    }
                    Some(false) => self.ducking.asking = None,
                    None => {}
                }
            }
        }
        let Some(mut a) = self.ducking.advanced.take() else {
            return;
        };
        let mut open = true;
        let (mut apply, mut cancel) = (false, false);
        let needs = match a.detect {
            DuckDetect::Speech => speech_needs(),
            DuckDetect::Level => Vec::new(),
        };
        dialog::window(ctx, "Ducking", &mut open, |ui| {
            dialog::form(ui, "ducking-form", |ui| {
                dialog::row(ui, "Track", |ui| ui.label(a.name.as_str()));
                dialog::row(ui, "Detect", |ui| {
                    dialog::switch(ui, |ui| {
                        ui.selectable_value(&mut a.detect, DuckDetect::Speech, "Speech");
                        ui.selectable_value(&mut a.detect, DuckDetect::Level, "Sound Level");
                    });
                });
                if a.detect == DuckDetect::Level {
                    dialog::row(ui, "Threshold", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut a.threshold_db)
                                .update_while_editing(false)
                                .range(duck::THRESHOLD)
                                .speed(0.5)
                                .suffix(" dB"),
                        );
                    });
                }
                dialog::row(ui, "Lookahead", |ui| {
                    millis(ui, &mut a.lookahead_ms, duck::LOOKAHEAD)
                });
                dialog::row(ui, "Rise Time", |ui| millis(ui, &mut a.rise_ms, duck::RISE));
                dialog::row(ui, "Hold", |ui| millis(ui, &mut a.hold_ms, duck::HOLD));
                dialog::row(ui, "Recovery Time", |ui| {
                    millis(ui, &mut a.recovery_ms, duck::RECOVERY)
                });
            });
            if !needs.is_empty() {
                dialog::note(
                    ui,
                    format!(
                        "Speech needs {}, downloaded once.",
                        vidcut_av::models::listed(&needs)
                    ),
                );
            }
            dialog::footer(ui, |ui| {
                apply = dialog::main_button(ui, "Apply", true);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if apply {
            let set = a.set();
            self.duck(&a.track, set);
        } else if open && !cancel {
            self.ducking.advanced = Some(a);
        }
    }
}
