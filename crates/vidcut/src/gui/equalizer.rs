use eframe::egui::{self, Align2, FontId, Rect, Sense, Shape, Stroke, pos2, vec2};
use serde_json::{Value, json};
use vidcut_core::equalizer::{self, FREQUENCY, GAIN, Q, kinds, label, response_db};
use vidcut_core::model::{EqBand, Equalizer};

use super::dialog;
use super::theme;
use super::workspace::Workspace;

const WIDTH: f32 = 520.0;
const GRAPH_HEIGHT: f32 = 120.0;
const GRAPH_DB: f64 = 24.0;
const GRID_HZ: [f64; 3] = [100.0, 1000.0, 10000.0];
const COLUMN: f32 = 76.0;
const HANDLE: f32 = 8.0;

#[derive(Debug, Clone)]
pub struct EqualizerDialog {
    clips: Vec<String>,
    eq: Equalizer,
}

impl EqualizerDialog {
    fn params(&self) -> Value {
        let bands: Vec<Value> = self
            .eq
            .bands
            .iter()
            .enumerate()
            .map(|(i, b)| {
                json!({
                    "band": i + 1, "on": b.on, "kind": b.kind,
                    "freq": b.freq, "gain": b.gain, "q": b.q,
                })
            })
            .collect();
        json!({ "clips": self.clips, "equalizer": { "on": self.eq.on, "bands": bands } })
    }
}

fn across(hz: f64) -> f32 {
    let (low, high) = (*FREQUENCY.start(), *FREQUENCY.end());
    ((hz / low).log10() / (high / low).log10()) as f32
}

fn hertz(share: f32) -> f64 {
    let (low, high) = (*FREQUENCY.start(), *FREQUENCY.end());
    low * (high / low).powf(f64::from(share))
}

fn graph(ui: &mut egui::Ui, eq: &mut Equalizer, rate: u32) {
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
    let steps = rect.width().max(1.0) as usize;
    let points = (0..=steps)
        .map(|i| {
            let share = i as f32 / steps as f32;
            let db = response_db(eq, hertz(share), rate).clamp(-GRAPH_DB, GRAPH_DB);
            pos2(rect.left() + share * rect.width(), height_of(db))
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(1.5, c.selection)));
}

fn number(
    ui: &mut egui::Ui,
    free: bool,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
    decimals: usize,
    unit: &str,
) {
    ui.add_enabled(
        free,
        egui::DragValue::new(value)
            .update_while_editing(false)
            .range(range)
            .speed(speed)
            .fixed_decimals(decimals)
            .suffix(unit),
    );
}

fn band_row(ui: &mut egui::Ui, i: usize, band: &mut EqBand) {
    ui.checkbox(&mut band.on, (i + 1).to_string());
    egui::ComboBox::from_id_salt(("equalizer-kind", i))
        .selected_text(label(band.kind))
        .width(COLUMN)
        .show_ui(ui, |ui| {
            for kind in kinds(i) {
                ui.selectable_value(&mut band.kind, *kind, label(*kind));
            }
        });
    let speed = band.freq * 0.01;
    number(ui, true, &mut band.freq, FREQUENCY, speed, 0, " Hz");
    let gain = equalizer::takes_gain(band.kind);
    number(ui, gain, &mut band.gain, GAIN, 0.1, 1, " dB");
    let q = equalizer::takes_q(band.kind);
    number(ui, q, &mut band.q, Q, 0.02, 1, "");
    ui.end_row();
}

impl Workspace {
    pub(super) fn open_equalizer(&mut self, clips: Vec<String>) {
        let stored = self.project_shared().and_then(|p| {
            let clip = p.clip(clips.first()?)?;
            clip.audio.as_ref()?.equalizer.clone()
        });
        let eq = stored.unwrap_or_else(equalizer::first);
        self.timeline.equalizer = Some(EqualizerDialog { clips, eq });
    }

    pub(super) fn equalizer_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.equalizer.take() else {
            return;
        };
        let rate = self
            .project_shared()
            .map_or(48000, |p| p.settings.sample_rate);
        let (mut open, mut done, mut apply) = (true, false, false);
        dialog::window_of(ctx, "Equalizer", &mut open, WIDTH, |ui| {
            graph(ui, &mut d.eq, rate);
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.eq.on, "On");
                if ui.button("Reset").clicked() {
                    d.eq = equalizer::first();
                }
            });
            egui::Grid::new("equalizer-bands")
                .num_columns(5)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    for name in ["Band", "Filter", "Frequency", "Gain", "Q"] {
                        ui.label(egui::RichText::new(name).color(theme::colors().text_dim));
                    }
                    ui.end_row();
                    for (i, band) in d.eq.bands.iter_mut().enumerate() {
                        band_row(ui, i, band);
                    }
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
            self.timeline.equalizer = Some(d);
        }
    }
}
