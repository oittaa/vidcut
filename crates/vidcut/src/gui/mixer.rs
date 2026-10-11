//! The mixer (pp. 649–650) and the audio meters (p. 642): a channel strip
//! per track with a meter, a fader, mute and solo, and the viewer's meter
//! with the monitoring mute.
//!
//! Meters read the media waveforms (peaks every 10 ms) at the playhead
//! rather than tapping the player's audio, so they show the edit's levels
//! with clip gain, faders, mute and solo applied, playing or not.

use std::collections::HashMap;
use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;
use vidcut_core::model::{Project, Track, TrackKind};
use vidcut_core::request::ViewerMode;

use super::theme;
use super::workspace::{Workspace, dim};

/// Meter state: peak holds, and fader positions while they are dragged.
#[derive(Default)]
pub struct Mixer {
    /// The highest level lately and when it was reached, by meter.
    holds: HashMap<String, (f32, Instant)>,
    /// Fader values being dragged (sent on release: one undo step).
    dragging: HashMap<String, f64>,
}

/// How long a peak hold stays up.
const HOLD_SECS: f32 = 1.5;

fn linear(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// dBFS of a linear level (−∞ for silence).
fn db(level: f32) -> f32 {
    if level <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * level.log10()
    }
}

/// Where a level sits on a meter, 0..1 over −60..0 dBFS.
fn position(level: f32) -> f32 {
    ((db(level) + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// The media level of `t` at timeline frame `at` (linear peak), from its
/// clips' waveforms, with clip gain and the fader.
fn track_level(p: &Project, t: &Track, at: i64, waveforms: &Waves) -> f32 {
    let fps = p.settings.fps.as_f64();
    let mut level = 0f32;
    for c in &t.clips {
        let (Some((a, b)), Some(audio)) = (c.audio_range(), &c.audio) else {
            continue;
        };
        if at < a || at >= b || audio.muted {
            continue;
        }
        let Some(w) = waveforms.get(&c.media) else {
            continue;
        };
        let origin = p.media(&c.media).map_or(0.0, |m| {
            m.info
                .video
                .as_ref()
                .map(|v| v.first_pts)
                .or_else(|| m.info.audio.first().map(|a| a.first_pts))
                .unwrap_or(0.0)
        });
        let s = c.source_at(at) as f64 / fps + origin;
        if let Some((lo, hi)) = w.range(s, s + 1.0 / fps) {
            let peak = lo.abs().max(hi.abs());
            level = level.max(peak * linear(audio.gain_db + t.volume_db));
        }
    }
    level
}

type Waves = HashMap<String, std::sync::Arc<vidcut_av::waveform::Waveform>>;

impl Workspace {
    /// Levels at the timeline playhead: each track's, and the mix's.
    fn timeline_levels(&self) -> (Vec<(String, f32)>, f32) {
        let Some(p) = self.project() else {
            return (Vec::new(), 0.0);
        };
        let at = self.snap.timeline.position;
        let any_solo = p.any_solo();
        let mut master = 0f32;
        let levels = p
            .tracks
            .iter()
            .map(|t| {
                let level = track_level(p, t, at, &self.snap.waveforms);
                if t.audible(any_solo) {
                    master += level;
                }
                (t.id.clone(), level)
            })
            .collect();
        (levels, master)
    }

    /// The level the viewer plays: the mix in Timeline mode, the source
    /// media at the source playhead otherwise.
    fn viewer_level(&self) -> f32 {
        if self.snap.player(crate::live::PlayerId::Timeline).muted {
            return 0.0;
        }
        if self.snap.viewer == ViewerMode::Timeline {
            return self.timeline_levels().1;
        }
        let (Some(p), Some((media, frame))) = (self.project(), self.snap.source_at()) else {
            return 0.0;
        };
        let Some(w) = self.snap.waveforms.get(media) else {
            return 0.0;
        };
        let fps = p.settings.fps.as_f64();
        let origin = p.media(media).map_or(0.0, |m| {
            m.info
                .video
                .as_ref()
                .map(|v| v.first_pts)
                .or_else(|| m.info.audio.first().map(|a| a.first_pts))
                .unwrap_or(0.0)
        });
        let s = frame as f64 / fps + origin;
        w.range(s, s + 1.0 / fps)
            .map_or(0.0, |(lo, hi)| lo.abs().max(hi.abs()))
    }

    /// Updates a meter's peak hold; returns the level to hold.
    fn hold(&mut self, key: &str, level: f32) -> f32 {
        let now = Instant::now();
        let held = self.mixer.holds.get(key).copied();
        let keep = held.filter(|(h, at)| *h >= level && at.elapsed().as_secs_f32() < HOLD_SECS);
        match keep {
            Some((h, _)) => h,
            None => {
                self.mixer.holds.insert(key.to_string(), (level, now));
                level
            }
        }
    }

    /// The Mixer panel (p. 649): a strip per track and the mix.
    pub(super) fn mixer_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project().cloned() else {
            return;
        };
        ui.horizontal(|ui| {
            ui.heading("Mixer");
            ui.label(dim("Mute and solo apply to export too (p. 650)"));
        });
        ui.separator();
        let (levels, master) = self.timeline_levels();
        let any_solo = p.any_solo();
        let height = ui.available_height().clamp(160.0, 320.0);
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for t in p.tracks.iter() {
                    // A strip for every audio track and every video track
                    // with sound.
                    let has_sound =
                        t.kind == TrackKind::Audio || t.clips.iter().any(|c| c.audio.is_some());
                    if !has_sound {
                        continue;
                    }
                    let level = levels
                        .iter()
                        .find(|(id, _)| id == &t.id)
                        .map_or(0.0, |l| l.1);
                    let heard = t.audible(any_solo);
                    self.strip(ui, t, if heard { level } else { 0.0 }, height);
                    ui.separator();
                }
                self.master_strip(ui, master, height);
            });
        });
        if master > 0.0 || self.snap.timeline.playing {
            ui.ctx().request_repaint();
        }
    }

    fn strip(&mut self, ui: &mut egui::Ui, t: &Track, level: f32, height: f32) {
        ui.vertical(|ui| {
            ui.set_width(74.0);
            let name = t.name.clone().unwrap_or_else(|| t.id.clone());
            ui.label(RichText::new(name).strong()).on_hover_text(&t.id);
            let held = self.hold(&t.id, level);
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(12.0, height - 90.0), egui::Sense::hover());
                meter(ui.painter(), rect, level, held);
                // The fader: dB, sent when it is let go.
                let mut value = self
                    .mixer
                    .dragging
                    .get(&t.id)
                    .copied()
                    .unwrap_or(t.volume_db);
                // A vertical slider is `slider_width` long.
                ui.spacing_mut().slider_width = height - 90.0;
                let r = ui
                    .add(
                        egui::Slider::new(&mut value, -60.0..=24.0)
                            .vertical()
                            .show_value(false)
                            .step_by(0.5),
                    )
                    .on_hover_text("Track volume; double-click for 0 dB");
                if r.double_clicked() {
                    self.mixer.dragging.remove(&t.id);
                    self.send("track.set", json!({ "track": t.id, "volume_db": 0.0 }));
                } else if r.dragged() || r.changed() {
                    self.mixer.dragging.insert(t.id.clone(), value);
                }
                if (r.drag_stopped() || (r.changed() && !r.dragged()))
                    && let Some(v) = self.mixer.dragging.remove(&t.id)
                    && v != t.volume_db
                {
                    self.send("track.set", json!({ "track": t.id, "volume_db": v }));
                }
                ui.label(RichText::new(format!("{value:+.1}")).small().monospace());
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if ui
                    .selectable_label(t.muted, "M")
                    .on_hover_text("Mute (export too)")
                    .clicked()
                {
                    self.send("track.set", json!({ "track": t.id, "muted": !t.muted }));
                }
                if ui
                    .selectable_label(t.solo, "S")
                    .on_hover_text("Solo: only soloed tracks are heard")
                    .clicked()
                {
                    self.send("track.set", json!({ "track": t.id, "solo": !t.solo }));
                }
            });
        });
    }

    fn master_strip(&mut self, ui: &mut egui::Ui, level: f32, height: f32) {
        ui.vertical(|ui| {
            ui.label(RichText::new("Mix").strong());
            let held = self.hold("mix", level);
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, height - 90.0), egui::Sense::hover());
            meter(ui.painter(), rect, level, held);
            let peak = db(held);
            ui.label(
                RichText::new(if peak.is_finite() {
                    format!("{peak:.1}")
                } else {
                    "-∞".into()
                })
                .small()
                .monospace(),
            );
        });
    }

    /// The meter beside the viewer with the monitoring mute (p. 642).
    pub(super) fn viewer_meter(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let level = self.viewer_level();
        let held = self.hold("viewer", level);
        let bar = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 22.0));
        meter(ui.painter(), bar, level, held);
        let muted = self.snap.player(crate::live::PlayerId::Timeline).muted;
        let button = Rect::from_min_max(
            pos2(rect.left() - 4.0, rect.bottom() - 20.0),
            rect.max + vec2(4.0, 0.0),
        );
        let r = ui.put(
            button,
            egui::Button::selectable(muted, if muted { "🔇" } else { "🔊" }).small(),
        );
        if r.on_hover_text("Mute what the viewer plays (not the export)")
            .clicked()
        {
            self.send("player.mute", json!({ "mute": !muted }));
        }
        if level > 0.0 || self.snap.active().playing {
            ui.ctx().request_repaint();
        }
    }
}

/// A vertical meter: green, yellow from −12 dBFS, red from −3 dBFS (p. 642),
/// with a peak-hold line.
fn meter(painter: &egui::Painter, rect: Rect, level: f32, hold: f32) {
    painter.rect_filled(rect, CornerRadius::same(2), theme::PANEL_DARK);
    let h = rect.height();
    let y = |pos: f32| rect.bottom() - pos * h;
    let top = y(position(level));
    for (from, to, color) in [
        (0.0, position(linear(-12.0)), Color32::from_rgb(70, 190, 90)),
        (
            position(linear(-12.0)),
            position(linear(-3.0)),
            Color32::from_rgb(220, 200, 60),
        ),
        (position(linear(-3.0)), 1.0, Color32::from_rgb(225, 70, 60)),
    ] {
        let (a, b) = (y(to).max(top), y(from));
        if a < b {
            painter.rect_filled(
                Rect::from_x_y_ranges(rect.x_range(), a..=b),
                CornerRadius::ZERO,
                color,
            );
        }
    }
    if hold > 0.0 {
        let hy = y(position(hold));
        painter.line_segment(
            [pos2(rect.left(), hy), pos2(rect.right(), hy)],
            Stroke::new(1.5, Color32::WHITE),
        );
    }
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, Color32::from_white_alpha(25)),
        StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_scale() {
        assert_eq!(position(1.0), 1.0);
        assert_eq!(position(0.0), 0.0);
        assert!((position(linear(-30.0)) - 0.5).abs() < 1e-3);
    }
}
