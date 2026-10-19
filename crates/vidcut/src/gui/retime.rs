use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_av::transcribe;

use super::export::{Ask, near_center};
use super::speech::{Hear, listed};
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

    fn correction_text(&self, c: &Value) -> String {
        let sections = c["sections"].as_array().cloned().unwrap_or_default();
        let fps = self.fps().as_f64().max(1.0);
        let mut text = match sections.as_slice() {
            [one] => format!("Moves them by {}", secs(&one["offset"])),
            many => format!(
                "Moves them by section: {}",
                many.iter()
                    .map(|s| format!(
                        "{} from {}",
                        secs(&s["offset"]),
                        self.tc((s["from"].as_f64().unwrap_or_default() * fps).round() as i64)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        if let Some(s) = speed(c) {
            text.push_str(&format!(", at {s}"));
        }
        if let Some(n) = c["snapped"].as_u64().filter(|&n| n > 0) {
            text = format!("{n} matched subtitle(s) go to their words. The others: {text}");
        }
        text
    }

    fn preview(&self, ui: &mut egui::Ui, v: &Value) {
        let s = &v["speech"];
        if s.is_object() {
            let mut line = format!("Speech: best offset {}", secs(&s["offset"]));
            if let Some(sp) = speed(s) {
                line.push_str(&format!(" at {sp}"));
            }
            let clarity = s["clarity"]
                .as_f64()
                .map_or("far above".into(), |c| format!("{c:.2} times"));
            ui.label(line);
            ui.label(dim(format!(
                "Its peak, {:.3}, is {clarity} the next best ({}, {:.3}); {} times is needed",
                s["peak"].as_f64().unwrap_or_default(),
                secs(&s["rival_offset"]),
                s["rival"].as_f64().unwrap_or_default(),
                s["needed"]
            )));
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
                ui.label(dim(format!(
                    "After the speech's offset: {}",
                    errors(&w["speech_errors"])
                )));
            }
        }
        ui.label(RichText::new(self.correction_text(&v["correction"])).strong());
        if v["correction"]["errors"].is_object() {
            ui.label(dim(format!(
                "Matched subtitles after it: {}",
                errors(&v["correction"]["errors"])
            )));
        }
        ui.label(dim(format!(
            "{} of {} subtitles move",
            v["moved"], v["cues"]
        )));
        for warning in v["warnings"].as_array().into_iter().flatten() {
            ui.label(
                RichText::new(format!("⚠ {}", warning.as_str().unwrap_or_default()))
                    .color(ui.visuals().warn_fg_color),
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
        let needs = transcribe::needed(&dir, d.words.then_some(file));
        let name = t.display_name();
        let mut open = true;
        let (mut find, mut apply, mut cancel) = (false, false, false);
        egui::Window::new("Re-time Subtitles to Speech")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 460.0, 360.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label(dim(format!(
                    "Lines up where speech is heard on the tracks it hears with where {name}'s \
                     subtitles show, and moves them to fit: an offset, and a speed change where \
                     one fits better. It needs no words, so any language works, translations too."
                )));
                ui.add_space(4.0);
                ui.add_enabled_ui(d.asked.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Hear");
                        d.heard = d.hear.ui(ui, &p);
                    });
                });
                match d.heard.is_empty() {
                    true => ui.label(
                        RichText::new("Tick a track to hear").color(ui.visuals().warn_fg_color),
                    ),
                    false => ui.label(dim(format!(
                        "Hears {} over the whole timeline",
                        listed(&d.heard)
                    ))),
                };
                ui.add_enabled_ui(d.asked.is_none(), |ui| {
                    ui.checkbox(&mut d.words, format!("Also match words ({})", file.title));
                });
                ui.indent("retime-words-note", |ui| {
                    ui.label(dim(
                        "Reads the words too, those kept for each clip's file, transcribing and \
                         keeping a file's that aren't: checks the result on the subtitles whose \
                         words it finds, finds where the offset jumps, and can move each of them \
                         to its words",
                    ));
                    if !transcribe::present(&dir, file) {
                        ui.label(dim(format!(
                            "{}, downloaded from Hugging Face when first used",
                            transcribe::size(file.bytes)
                        )));
                    }
                });
                ui.separator();
                match (&d.error, d.current(), d.asked.is_some()) {
                    (_, _, true) => {
                        ui.label(dim("Finding… Jobs shows the progress, and can cancel it"));
                    }
                    (Some((key, e)), _, false) if *key == d.key() => {
                        ui.label(RichText::new(e).color(theme::colors().error));
                    }
                    (_, Some(v), false) => {
                        let v = v.clone();
                        self.preview(ui, &v);
                    }
                    _ => {
                        ui.label(dim("Find shows what it would do before anything moves."));
                    }
                }
                if !needs.is_empty() {
                    ui.label(dim(format!(
                        "Find first downloads {}, checked against SHA-256",
                        transcribe::listed(&needs)
                    )));
                }
                ui.horizontal(|ui| {
                    let label = match needs.is_empty() {
                        true => "Find".to_string(),
                        false => "Download and Find".to_string(),
                    };
                    find = ui
                        .add_enabled(
                            d.asked.is_none() && !d.heard.is_empty(),
                            egui::Button::new(label),
                        )
                        .clicked();
                    let ready = d.asked.is_none()
                        && d.current().is_some_and(|v| v["moved"].as_u64() > Some(0));
                    apply = ui.add_enabled(ready, egui::Button::new("Apply")).clicked();
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
