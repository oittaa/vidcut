use std::ops::RangeInclusive;

use eframe::egui;
use serde_json::{Value, json};
use vidcut_core::duck;
use vidcut_core::model::{Duck, DuckDetect, Project, Track};
use vidcut_core::request::DuckSet;

use super::dialog;
use super::export::Ask;
use super::theme;
use super::workspace::Workspace;

const GRAPH_HEIGHT: f32 = 110.0;
const SHOWN_SECS: f64 = 8.0;
const ASKED_SECS: f64 = 60.0;
const FLOOR_DB: f64 = -60.0;

#[derive(Default)]
pub(super) struct DuckingUi {
    asking: Option<(String, Value)>,
    amount: Option<(String, f64)>,
    advanced: Option<Advanced>,
    graph: GraphUi,
}

#[derive(Default)]
struct GraphUi {
    ask: Option<Ask>,
    asked: Option<GraphAsk>,
    curve: Option<Curve>,
    failed: Option<String>,
}

struct GraphAsk {
    set: Value,
    rev: u64,
    from: i64,
    to: i64,
}

struct Curve {
    from: i64,
    per_second: f64,
    source_db: Vec<Option<f64>>,
    duck_db: Vec<f64>,
}

impl Curve {
    fn of(v: &Value) -> Self {
        let list = |k: &str| v[k].as_array().cloned().unwrap_or_default();
        Self {
            from: v["from"].as_i64().unwrap_or(0),
            per_second: v["per_second"].as_f64().unwrap_or(1.0),
            source_db: list("source_db").iter().map(Value::as_f64).collect(),
            duck_db: list("duck_db").iter().filter_map(Value::as_f64).collect(),
        }
    }
}

fn points_within<T>(v: &[T], i: i64, j: i64) -> &[T] {
    let n = v.len() as i64;
    &v[i.clamp(0, n) as usize..j.clamp(0, n) as usize]
}

fn paint_graph(ui: &mut egui::Ui, g: &GraphUi, threshold: Option<f64>, at: f64, fps: f64) {
    let c = theme::colors();
    let size = egui::vec2(ui.available_width(), GRAPH_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, c.track);
    let y = |db: f64| rect.top() + rect.height() * (db / FLOOR_DB).clamp(0.0, 1.0) as f32;
    if let Some(t) = threshold {
        painter.hline(rect.x_range(), y(t), egui::Stroke::new(1.0, c.text_dim));
    }
    if let Some(curve) = &g.curve {
        let secs_per_pixel = SHOWN_SECS / f64::from(rect.width());
        let shown_from = at / fps - SHOWN_SECS / 2.0 - curve.from as f64 / fps;
        let point =
            |px: i64| ((shown_from + px as f64 * secs_per_pixel) * curve.per_second).floor() as i64;
        let mut dip = Vec::new();
        for px in 0..rect.width() as i64 {
            let (i, j) = (point(px), point(px + 1));
            let j = j.max(i + 1);
            let x = rect.left() + px as f32;
            let level = points_within(&curve.source_db, i, j)
                .iter()
                .flatten()
                .copied()
                .reduce(f64::max);
            if let Some(level) = level {
                let top = y(level);
                painter.vline(x, top..=rect.bottom(), egui::Stroke::new(1.0, c.audio));
            }
            let deepest = points_within(&curve.duck_db, i, j)
                .iter()
                .copied()
                .reduce(f64::min);
            if let Some(d) = deepest {
                dip.push(egui::pos2(x, y(d)));
            }
        }
        painter.add(egui::Shape::line(dip, egui::Stroke::new(1.5, c.accent)));
    }
    painter.vline(
        rect.center().x,
        rect.y_range(),
        egui::Stroke::new(1.0, c.playhead),
    );
    let font = egui::FontId::proportional(11.0);
    let inset = egui::vec2(4.0, 2.0);
    painter.text(
        rect.left_top() + inset,
        egui::Align2::LEFT_TOP,
        "0 dB",
        font.clone(),
        c.text_dim,
    );
    painter.text(
        rect.left_bottom() + egui::vec2(inset.x, -inset.y),
        egui::Align2::LEFT_BOTTOM,
        format!("{FLOOR_DB} dB"),
        font.clone(),
        c.text_dim,
    );
    let right = rect.right_top() + egui::vec2(-inset.x, inset.y);
    painter.text(
        right,
        egui::Align2::RIGHT_TOP,
        "Ducking",
        font.clone(),
        c.accent,
    );
    painter.text(
        right + egui::vec2(0.0, 14.0),
        egui::Align2::RIGHT_TOP,
        "Source",
        font,
        c.audio,
    );
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
            self.ducking.graph = GraphUi::default();
        }
    }

    fn ask_graph(&mut self, ctx: &egui::Context, a: &Advanced, playhead: i64) {
        let Some((rev, end, fps)) = self
            .project()
            .map(|p| (p.rev, p.end(), p.settings.fps.as_f64()))
        else {
            return;
        };
        let g = &mut self.ducking.graph;
        if let Some(result) = g.ask.as_ref().and_then(Ask::take) {
            g.ask = None;
            match result {
                Ok(r) => {
                    g.curve = Some(Curve::of(&r.result));
                    g.failed = None;
                }
                Err(e) => g.failed = Some(e.message),
            }
        }
        if g.ask.is_some() {
            return;
        }
        let frames = |secs: f64| (secs * fps).ceil() as i64;
        let set = a.set();
        let shown = (
            (playhead - frames(SHOWN_SECS / 2.0)).max(0),
            (playhead + frames(SHOWN_SECS / 2.0)).min(end),
        );
        let covered = g
            .asked
            .as_ref()
            .is_some_and(|x| x.set == set && x.rev == rev && x.from <= shown.0 && shown.1 <= x.to);
        let (from, to) = (
            (playhead - frames(ASKED_SECS / 2.0)).max(0),
            (playhead + frames(ASKED_SECS / 2.0)).min(end),
        );
        if covered || to <= from {
            return;
        }
        let params = json!({
            "track": a.track,
            "range": format!("{from}f..{to}f"),
            "duck": set,
        });
        g.asked = Some(GraphAsk { set, rev, from, to });
        g.ask = Some(Ask::send(&self.live, ctx, "analyze.ducking", params));
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
        let playhead = self.drawn_playhead();
        if needs.is_empty() {
            self.ask_graph(ctx, &a, playhead);
        }
        let at = self.glided_playhead(playhead);
        let fps = self.project().map_or(1.0, |p| p.settings.fps.as_f64());
        let graph = &self.ducking.graph;
        dialog::window(ctx, "Ducking", &mut open, |ui| {
            let threshold = (a.detect == DuckDetect::Level).then_some(a.threshold_db);
            paint_graph(ui, graph, threshold, at, fps);
            if let Some(failed) = &graph.failed {
                dialog::warn(ui, failed.as_str());
            }
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
