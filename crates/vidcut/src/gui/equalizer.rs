use std::ops::RangeInclusive;
use std::time::Instant;

use eframe::egui::{self, Align, Align2, FontId, Layout, Rect, Sense, Shape, Stroke, pos2, vec2};
use serde_json::{Value, json};
use vidcut_core::equalizer::{self, FREQUENCY, GAIN, Q, kinds, label, response_db};
use vidcut_core::model::{EqBand, Equalizer, Project};

use super::inspector::switched_fold;
use super::theme;
use super::timeline::tools::landing;
use super::workspace::{Workspace, dim};

const GRAPH_HEIGHT: f32 = 120.0;
const GRAPH_DB: f64 = 24.0;
const GRID_HZ: [f64; 3] = [100.0, 1000.0, 10000.0];
const MENU_WIDTH: f32 = 72.0;
const HANDLE: f32 = 8.0;

#[derive(Debug, Clone)]
pub struct Draft {
    clips: Vec<String>,
    from: Equalizer,
    to: Equalizer,
    since: Option<Instant>,
}

fn across(hz: f64) -> f32 {
    let (low, high) = (*FREQUENCY.start(), *FREQUENCY.end());
    ((hz / low).log10() / (high / low).log10()) as f32
}

fn hertz(share: f32) -> f64 {
    let (low, high) = (*FREQUENCY.start(), *FREQUENCY.end());
    low * (high / low).powf(f64::from(share))
}

fn graph(ui: &mut egui::Ui, eq: &mut Equalizer, rate: u32) -> bool {
    let size = vec2(ui.available_width(), GRAPH_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let c = theme::colors();
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let line = Stroke::new(1.0, c.line);
    for hz in GRID_HZ {
        let x = rect.left() + across(hz) * rect.width();
        painter.vline(x, rect.y_range(), line);
    }
    painter.hline(rect.x_range(), rect.center().y, line);
    let half = rect.height() / 2.0;
    let height_of = |db: f64| rect.center().y - (db / GRAPH_DB) as f32 * half;
    let mut held = false;
    for (i, band) in eq.bands.iter_mut().enumerate().filter(|(_, b)| b.on) {
        let lifts = equalizer::takes_gain(band.kind);
        let place = |b: &EqBand| {
            let db = if lifts {
                b.gain.clamp(-GRAPH_DB, GRAPH_DB)
            } else {
                0.0
            };
            pos2(rect.left() + across(b.freq) * rect.width(), height_of(db))
        };
        let hit = Rect::from_center_size(place(band), vec2(HANDLE, HANDLE) * 2.0);
        let id = ui.scope_id().with(("equalizer-handle", i));
        let response = ui.interact(hit, id, Sense::drag());
        held |= response.dragged();
        if let Some(p) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            let share = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            band.freq = hertz(share).round();
            if lifts {
                let db = f64::from((rect.center().y - p.y) / half) * GRAPH_DB;
                band.gain = ((db * 10.0).round() / 10.0).clamp(*GAIN.start(), *GAIN.end());
            }
        }
        let at = place(band);
        painter.circle_filled(at, HANDLE, c.selection);
        let font = FontId::proportional(11.0);
        let number = (i + 1).to_string();
        painter.text(at, Align2::CENTER_CENTER, number, font, c.selected_text);
    }
    let shape = Equalizer {
        on: true,
        ..eq.clone()
    };
    let steps = rect.width().max(1.0) as usize;
    let points = (0..=steps)
        .map(|i| {
            let share = i as f32 / steps as f32;
            let db = response_db(&shape, hertz(share), rate).clamp(-GRAPH_DB, GRAPH_DB);
            pos2(rect.left() + share * rect.width(), height_of(db))
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(1.5, c.selection)));
    held
}

fn number(
    ui: &mut egui::Ui,
    free: bool,
    value: &mut f64,
    range: RangeInclusive<f64>,
    speed: f64,
    decimals: usize,
    unit: &str,
) -> bool {
    ui.add_enabled(
        free,
        egui::DragValue::new(value)
            .update_while_editing(false)
            .range(range)
            .speed(speed)
            .fixed_decimals(decimals)
            .suffix(unit),
    )
    .dragged()
}

fn band_row(ui: &mut egui::Ui, i: usize, band: &mut EqBand) -> bool {
    ui.checkbox(&mut band.on, (i + 1).to_string());
    egui::ComboBox::from_id_salt(("equalizer-kind", i))
        .selected_text(label(band.kind))
        .width(MENU_WIDTH)
        .show_ui(ui, |ui| {
            for kind in kinds(i) {
                ui.selectable_value(&mut band.kind, *kind, label(*kind));
            }
        });
    let speed = band.freq * 0.01;
    let mut held = number(ui, true, &mut band.freq, FREQUENCY, speed, 0, " Hz");
    let gain = equalizer::takes_gain(band.kind);
    held |= number(ui, gain, &mut band.gain, GAIN, 0.1, 1, " dB");
    if equalizer::takes_q(band.kind) {
        held |= number(ui, true, &mut band.q, Q, 0.02, 1, "");
    } else {
        ui.label("");
    }
    ui.end_row();
    held
}

fn change(from: &Equalizer, to: &Equalizer) -> Option<Value> {
    let bands: Vec<Value> = from
        .bands
        .iter()
        .zip(&to.bands)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (a, b))| {
            let mut band = json!({ "band": i + 1 });
            if a.on != b.on {
                band["on"] = json!(b.on);
            }
            if a.kind != b.kind {
                band["kind"] = json!(b.kind);
            }
            if a.freq != b.freq {
                band["freq"] = json!(b.freq);
            }
            if a.gain != b.gain {
                band["gain"] = json!(b.gain);
            }
            if a.q != b.q {
                band["q"] = json!(b.q);
            }
            band
        })
        .collect();
    (from.on != to.on || !bands.is_empty()).then(|| json!({ "on": to.on, "bands": bands }))
}

impl Workspace {
    pub(super) fn equalizer_section(&mut self, ui: &mut egui::Ui, p: &Project, clips: &[String]) {
        let stored_of = |id: &String| {
            p.clip(id)
                .and_then(|c| c.audio.as_ref())
                .and_then(|a| a.equalizer.clone())
                .unwrap_or_else(equalizer::usual)
        };
        let stored = stored_of(&clips[0]);
        let draft = self.timeline.equalizer.take().filter(|d| {
            d.clips == clips
                && d.since
                    .is_none_or(|t| landing(&d.to, t.elapsed(), &d.from, &stored).is_some())
        });
        let shown = draft
            .as_ref()
            .map_or_else(|| stored.clone(), |d| d.to.clone());
        let mut eq = shown.clone();
        let changed = clips.iter().any(|id| stored_of(id) != equalizer::usual());
        let count = clips.len();
        let header = |ui: &mut egui::Ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let reset = ui.add_enabled(changed, egui::Button::new("Reset"));
                if count > 1 {
                    ui.label(dim(format!("{count} clips")));
                }
                reset.clicked()
            })
            .inner
        };
        let rate = p.settings.sample_rate;
        let mut held = false;
        let (flipped, reset) = switched_fold(ui, "Equalizer", Some(shown.on), header, |ui| {
            ui.add_enabled_ui(eq.on, |ui| {
                held |= graph(ui, &mut eq, rate);
                egui::Grid::new("inspector-equalizer")
                    .num_columns(5)
                    .spacing([6.0, 6.0])
                    .show(ui, |ui| {
                        for name in ["Band", "Filter", "Frequency", "Gain", "Q"] {
                            ui.label(dim(name));
                        }
                        ui.end_row();
                        for (i, band) in eq.bands.iter_mut().enumerate() {
                            held |= band_row(ui, i, band);
                        }
                    });
            });
        });
        let draft_of = |from: Equalizer, to: Equalizer, since: Option<Instant>| Draft {
            clips: clips.to_vec(),
            from,
            to,
            since,
        };
        let now = Some(Instant::now());
        if reset {
            let ids: Vec<&String> = clips
                .iter()
                .filter(|id| stored_of(id) != equalizer::usual())
                .collect();
            self.send("clip.set", json!({ "clips": ids, "equalizer": null }));
            self.timeline.equalizer = Some(draft_of(stored, equalizer::usual(), now));
        } else if let Some(on) = flipped {
            let ids: Vec<&String> = clips.iter().filter(|id| stored_of(id).on != on).collect();
            if !ids.is_empty() {
                self.send(
                    "clip.set",
                    json!({ "clips": ids, "equalizer": { "on": on } }),
                );
                let to = Equalizer { on, ..shown };
                self.timeline.equalizer = Some(draft_of(stored, to, now));
            }
        } else if held {
            self.timeline.equalizer = Some(draft_of(stored, eq, None));
        } else if eq != shown || draft.as_ref().is_some_and(|d| d.since.is_none()) {
            if let Some(change) = change(&stored, &eq) {
                self.send("clip.set", json!({ "clips": clips, "equalizer": change }));
                self.timeline.equalizer = Some(draft_of(stored, eq, now));
            }
        } else {
            self.timeline.equalizer = draft;
        }
    }
}
