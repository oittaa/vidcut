use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_av::transcribe;

use super::dialog;
use super::export::Ask;
use super::speech::Hear;
use super::theme;
use super::workspace::{Workspace, dim};

const LISTED: usize = 200;

#[derive(Debug, Clone, PartialEq)]
struct Key {
    words: bool,
    heard: Vec<String>,
}

#[derive(Debug)]
pub struct Retime {
    track: String,
    words: bool,
    hear: Hear,
    heard: Vec<String>,
    asked: Option<(Key, Ask)>,
    found: Option<(Key, Value)>,
    error: Option<(Key, String)>,
}

impl Retime {
    fn key(&self) -> Key {
        Key {
            words: self.words,
            heard: self.heard.clone(),
        }
    }

    fn params(&self, download: bool) -> Value {
        let mut p = json!({ "track": self.track, "words": self.words, "tracks": self.heard });
        if download {
            p["download"] = json!(true);
        }
        p
    }

    fn current(&self) -> Option<&Value> {
        let key = self.key();
        self.found
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }
}

fn secs(v: &Value) -> String {
    format!("{:+.2} s", v.as_f64().unwrap_or_default())
}

fn speed(c: &Value) -> Option<String> {
    let s = c["speed"].as_f64().filter(|&s| s != 1.0)?;
    Some(match c["ratio"].as_str() {
        Some(r) => format!("{r} the speed (×{s:.5})"),
        None => format!("×{s:.5} the speed"),
    })
}

fn errors(e: &Value) -> String {
    format!(
        "median {:.2} s, 90% within {:.2} s, {:.1}% within 0.5 s",
        e["median"].as_f64().unwrap_or_default(),
        e["p90"].as_f64().unwrap_or_default(),
        e["within"].as_f64().unwrap_or_default() * 100.0
    )
}

impl Workspace {
    pub(crate) fn open_retime(&mut self, track: &str) {
        self.timeline.retime = Some(Retime {
            track: track.to_string(),
            words: false,
            hear: Hear::default(),
            heard: Vec::new(),
            asked: None,
            found: None,
            error: None,
        });
    }

    fn result(&self, ui: &mut egui::Ui, v: &Value) {
        let c = &v["correction"];
        let sections = c["sections"].as_array().cloned().unwrap_or_default();
        let fps = self.fps().as_f64().max(1.0);
        let colors = theme::colors();
        egui::Frame::new()
            .fill(colors.faint)
            .stroke(egui::Stroke::new(1.0, colors.line))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                let head = match sections.as_slice() {
                    [one] => secs(&one["offset"]),
                    many => format!("{} offsets", many.len()),
                };
                ui.label(RichText::new(head).size(20.0).strong());
                if sections.len() > 1 {
                    for s in &sections {
                        let from = (s["from"].as_f64().unwrap_or_default() * fps).round() as i64;
                        dialog::note(ui, format!("{} from {}", secs(&s["offset"]), self.tc(from)));
                    }
                }
                if let Some(s) = speed(c) {
                    dialog::note(ui, format!("At {s}"));
                }
                if let Some(n) = c["snapped"].as_u64().filter(|&n| n > 0) {
                    dialog::note(ui, format!("{n} matched subtitle(s) go to their words"));
                }
                ui.label(format!("{} of {} subtitles move", v["moved"], v["cues"]));
                for warning in v["warnings"].as_array().into_iter().flatten() {
                    dialog::warn(ui, warning.as_str().unwrap_or_default());
                }
                egui::CollapsingHeader::new("Details")
                    .id_salt("retime-details")
                    .show(ui, |ui| self.details(ui, v));
            });
    }

    fn details(&self, ui: &mut egui::Ui, v: &Value) {
        let s = &v["speech"];
        if s.is_object() {
            let mut line = format!("Speech: {}", secs(&s["offset"]));
            if let Some(sp) = speed(s) {
                line.push_str(&format!(" at {sp}"));
            }
            ui.label(line);
            let clarity = s["clarity"]
                .as_f64()
                .map_or("far above".into(), |c| format!("{c:.2} times"));
            dialog::note(
                ui,
                format!(
                    "Peak {:.3}, {clarity} the next best: {:.3} at {}; needs {}",
                    s["peak"].as_f64().unwrap_or_default(),
                    s["rival"].as_f64().unwrap_or_default(),
                    secs(&s["rival_offset"]),
                    s["needed"]
                ),
            );
        }
        let w = &v["words"];
        if w.is_object() {
            ui.label(format!(
                "Words: {} of {} subtitles matched ({}, {})",
                w["matched"],
                v["cues"],
                w["model"].as_str().unwrap_or_default(),
                w["language"].as_str().unwrap_or("language not told")
            ));
            if w["speech_errors"].is_object() {
                dialog::note(
                    ui,
                    format!("After the speech's offset: {}", errors(&w["speech_errors"])),
                );
            }
        }
        if v["correction"]["errors"].is_object() {
            dialog::note(
                ui,
                format!(
                    "After the correction: {}",
                    errors(&v["correction"]["errors"])
                ),
            );
        }
        let unmatched: Vec<&str> = w["unmatched"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if unmatched.is_empty() {
            return;
        }
        let Some(p) = self.project_shared() else {
            return;
        };
        egui::CollapsingHeader::new(format!("{} didn't match", unmatched.len()))
            .id_salt("retime-unmatched")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for id in unmatched.iter().take(LISTED) {
                            let Some(c) = p.cue(id) else { continue };
                            let text = vidcut_core::subtitles::plain(&c.text).replace('\n', " ");
                            ui.label(format!("{}  {text}", self.tc(c.start)));
                        }
                        if unmatched.len() > LISTED {
                            ui.label(dim(format!("and {} more", unmatched.len() - LISTED)));
                        }
                    });
            });
    }

    pub(super) fn retime_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.retime.take() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(t) = p.subtitle_track(&d.track) else {
            return;
        };
        if let Some((key, ask)) = &d.asked
            && let Some(result) = ask.take()
        {
            match result {
                Ok(r) => {
                    d.found = Some((key.clone(), r.result));
                    d.error = None;
                }
                Err(e) => {
                    d.found = None;
                    d.error = Some((key.clone(), e.message));
                }
            }
            d.asked = None;
        }
        let file = &transcribe::PARAKEET_ULTRA;
        let dir = crate::paths::models_dir();
        let needs = transcribe::needed(
            &dir,
            &d.words.then_some(file).into_iter().collect::<Vec<_>>(),
        );
        let name = t.display_name();
        let mut open = true;
        let (mut find, mut apply, mut cancel) = (false, false, false);
        dialog::window(ctx, "Re-time Subtitles to Speech", &mut open, |ui| {
            ui.add_enabled_ui(d.asked.is_none(), |ui| {
                dialog::form(ui, "retime-form", |ui| {
                    dialog::row(ui, "Track", |ui| ui.label(&name));
                    d.heard = dialog::row(ui, "Hear", |ui| d.hear.ui(ui, &p));
                    dialog::row(ui, "Words", |ui| {
                        ui.checkbox(&mut d.words, "Match Words Too")
                            .on_hover_text(format!(
                                "More exact: reads the words with {}",
                                file.title
                            ));
                    });
                });
            });
            if d.heard.is_empty() {
                dialog::warn(ui, "Tick a track to hear");
            }
            if !needs.is_empty() {
                dialog::note(
                    ui,
                    format!("Downloads {} first", transcribe::listed(&needs)),
                );
            }
            match (&d.error, d.current(), d.asked.is_some()) {
                (_, _, true) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(dim("Finding… Jobs shows its progress"));
                    });
                }
                (Some((key, e)), _, false) if *key == d.key() => {
                    ui.add(egui::Label::new(RichText::new(e).color(theme::colors().error)).wrap());
                }
                (_, Some(v), false) => {
                    let v = v.clone();
                    self.result(ui, &v);
                }
                _ => {}
            }
            let idle = d.asked.is_none();
            let ready = idle && d.current().is_some_and(|v| v["moved"].as_u64() > Some(0));
            let can_find = idle && !d.heard.is_empty();
            let find_label = match needs.is_empty() {
                true => "Find",
                false => "Download and Find",
            };
            dialog::footer(ui, |ui| {
                if ready {
                    apply = dialog::main_button(ui, "Apply", true);
                    find = ui
                        .add_enabled(can_find, egui::Button::new(find_label))
                        .clicked();
                } else {
                    find = dialog::main_button(ui, find_label, can_find);
                }
                cancel = ui.button("Cancel").clicked();
            });
        });
        if find {
            let mut dry = d.params(!needs.is_empty());
            dry["dry_run"] = json!(true);
            d.found = None;
            d.error = None;
            d.asked = Some((d.key(), Ask::send(&self.live, ctx, "subtitles.retime", dry)));
        }
        if apply {
            self.send("subtitles.retime", d.params(false));
        } else if open && !cancel {
            self.timeline.retime = Some(d);
        }
    }
}
