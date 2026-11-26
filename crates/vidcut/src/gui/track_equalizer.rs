use std::time::Instant;

use eframe::egui::{self, Sense, Shape, Stroke, vec2};
use serde_json::{Value, json};
use vidcut_core::equalizer::track::{
    self, GAIN, Q, SLOPES, TYPES, kinds, label_of, takes_q, takes_slope,
};
use vidcut_core::model::{EqKind, Project, Track, TrackEqualizer};

use super::dialog;
use super::equalizer::{Draft, Limits, band_changes, band_row, curve, graph};
use super::theme;
use super::timeline::tools::landing;
use super::workspace::{Workspace, dim};

const WIDTH: f32 = 560.0;
const GRAPH_DB: f64 = 20.0;
const MENU_WIDTH: f32 = 64.0;

#[derive(Debug, Clone, Default)]
pub struct TrackEqualizerUi {
    open: Option<String>,
    draft: Option<Draft<TrackEqualizer>>,
}

fn change(from: &TrackEqualizer, to: &TrackEqualizer) -> Option<Value> {
    let bands = band_changes(&from.bands, &to.bands);
    let mut set = json!({ "on": to.on });
    if from.r#type != to.r#type {
        set["type"] = json!(to.r#type);
    }
    if from.hi_pass_slope != to.hi_pass_slope {
        set["hi_pass_slope"] = json!(to.hi_pass_slope);
    }
    if from.lo_pass_slope != to.lo_pass_slope {
        set["lo_pass_slope"] = json!(to.lo_pass_slope);
    }
    if !bands.is_empty() {
        set["bands"] = json!(bands);
    }
    (from != to).then_some(set)
}

fn slope_menu(ui: &mut egui::Ui, i: usize, slope: &mut u32) {
    egui::ComboBox::from_id_salt(("track-equalizer-slope", i))
        .selected_text(format!("{slope} dB"))
        .width(MENU_WIDTH)
        .show_ui(ui, |ui| {
            for s in SLOPES {
                ui.selectable_value(slope, s, format!("{s} dB"));
            }
        });
}

fn stored(p: &Project, id: &str) -> Option<TrackEqualizer> {
    p.track(id)
        .map(|t| t.equalizer.clone().unwrap_or_else(track::usual))
}

impl Workspace {
    pub(super) fn open_track_equalizer(&mut self, id: &str) -> Result<(), String> {
        let p = self.project_shared().ok_or("Open a project first")?;
        let t = p.track(id).ok_or_else(|| format!("no track {id}"))?;
        if !t.has_sound() {
            return Err(format!("{id} has no sound"));
        }
        self.track_equalizer = TrackEqualizerUi {
            open: Some(id.to_string()),
            draft: None,
        };
        Ok(())
    }

    pub(super) fn close_track_equalizer(&mut self) {
        self.track_equalizer = TrackEqualizerUi::default();
    }

    pub(super) fn track_equalizer_open(&self) -> bool {
        self.track_equalizer.open.is_some()
    }

    pub(super) fn equalizer_controls(&mut self, ui: &mut egui::Ui, p: &Project, t: &Track) {
        let eq = t.equalizer.clone().unwrap_or_else(track::usual);
        ui.horizontal(|ui| {
            ui.label("EQ");
            let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
            let (rect, r) = ui.allocate_exact_size(size, Sense::click());
            let c = theme::colors();
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
            painter.hline(rect.x_range(), rect.center().y, Stroke::new(1.0, c.line));
            let color = if eq.on { c.selection } else { c.text_dim };
            let points = curve(&eq, p.settings.sample_rate, rect.shrink(1.0), GRAPH_DB);
            painter.add(Shape::line(points, Stroke::new(1.0, color)));
            let r = r.on_hover_text("Click To Bypass, Double-Click To Open");
            if r.double_clicked() {
                let _ = self.open_track_equalizer(&t.id);
            } else if r.clicked() {
                self.send(
                    "track.set",
                    json!({ "track": t.id, "equalizer": { "on": !eq.on } }),
                );
            }
        });
    }

    pub(super) fn track_equalizer_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.track_equalizer.open.clone() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let (Some(stored), Some(t)) = (stored(&p, &id), p.track(&id)) else {
            self.track_equalizer = TrackEqualizerUi::default();
            return;
        };
        let draft = self.track_equalizer.draft.take().filter(|d| {
            d.owners == [id.clone()]
                && d.since
                    .is_none_or(|s| landing(&d.to, s.elapsed(), &d.from, &stored).is_some())
        });
        let shown = draft
            .as_ref()
            .map_or_else(|| stored.clone(), |d| d.to.clone());
        let mut eq = shown.clone();
        let name = t.name.clone().unwrap_or_else(|| t.id.clone());
        let rate = p.settings.sample_rate;
        let mut open = true;
        let (mut held, mut reset, mut close) = (false, false, false);
        let title = format!("Equalizer - {name}");
        dialog::window_of(ctx, &title, &mut open, WIDTH, |ui| {
            dialog::form(ui, "track-equalizer-form", |ui| {
                dialog::row(ui, "Equalizer", |ui| {
                    dialog::switch(ui, |ui| {
                        ui.selectable_value(&mut eq.on, false, "Off");
                        ui.selectable_value(&mut eq.on, true, "On");
                    });
                });
                dialog::row(ui, "Type", |ui| {
                    egui::ComboBox::from_id_salt("track-equalizer-type")
                        .selected_text(label_of(eq.r#type))
                        .show_ui(ui, |ui| {
                            for kind in TYPES {
                                ui.selectable_value(&mut eq.r#type, kind, label_of(kind));
                            }
                        });
                });
            });
            ui.add_enabled_ui(eq.on, |ui| {
                held |= graph(ui, &mut eq, rate, GAIN, GRAPH_DB);
                let of = eq.r#type;
                let takes = move |kind: EqKind| takes_q(kind, of);
                egui::Grid::new("track-equalizer-bands")
                    .num_columns(6)
                    .spacing([6.0, 6.0])
                    .show(ui, |ui| {
                        for name in ["Band", "Filter", "Frequency", "Gain", "Q", "Slope"] {
                            ui.label(dim(name));
                        }
                        ui.end_row();
                        for i in 0..eq.bands.len() {
                            let limits = Limits {
                                kinds: kinds(i),
                                gain: GAIN,
                                q: Q,
                                takes_q: &takes,
                            };
                            held |= band_row(ui, i, &mut eq.bands[i], &limits);
                            let slopes = takes_slope(eq.bands[i].kind, of);
                            match i {
                                0 if slopes => slope_menu(ui, i, &mut eq.hi_pass_slope),
                                5 if slopes => slope_menu(ui, i, &mut eq.lo_pass_slope),
                                _ => {
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
            dialog::footer(ui, |ui| {
                close = dialog::main_button(ui, "Close", true);
                let changed = stored != track::usual();
                reset = ui
                    .add_enabled(changed, egui::Button::new("Reset"))
                    .clicked();
            });
        });
        for (band, was) in eq.bands.iter_mut().zip(&shown.bands) {
            if band.q != was.q {
                band.q = track::stepped(band.q);
            }
        }
        let now = Some(Instant::now());
        let draft_of = |from: TrackEqualizer, to: TrackEqualizer, since| Draft {
            owners: vec![id.clone()],
            from,
            to,
            since,
        };
        self.track_equalizer.draft = if reset {
            self.send("track.set", json!({ "track": id, "equalizer": null }));
            Some(draft_of(stored, track::usual(), now))
        } else if held {
            Some(draft_of(stored, eq, None))
        } else if eq != shown || draft.as_ref().is_some_and(|d| d.since.is_none()) {
            match change(&stored, &eq) {
                Some(set) => {
                    self.send("track.set", json!({ "track": id, "equalizer": set }));
                    Some(draft_of(stored, eq, now))
                }
                None => None,
            }
        } else {
            draft
        };
        if !open || close {
            self.track_equalizer = TrackEqualizerUi::default();
        }
    }
}
