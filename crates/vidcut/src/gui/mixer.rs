use std::collections::HashMap;
use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;
use vidcut_core::lru::Lru;
use vidcut_core::model::{Project, Track};
use vidcut_core::request::ViewerMode;

use super::theme;
use super::workspace::{Workspace, dim};

const HOLDS_KEPT: usize = 1024;

#[derive(Default)]
pub struct Mixer {
    holds: Lru<String, (f32, Instant), HOLDS_KEPT>,
    dragging: HashMap<String, f64>,
}

const HOLD_SECS: f32 = 1.5;

const BAR_W: f32 = 10.0;

const FADER_TRAVEL: f64 = 66.0;

const FADER_LAW: [(f64, f64); 23] = [
    (0.0, 10.0),
    (3.0, 8.5),
    (6.0, 6.6),
    (9.0, 4.8),
    (12.0, 3.1),
    (15.0, 1.3),
    (18.0, -0.3),
    (21.0, -2.1),
    (24.0, -3.8),
    (27.0, -5.6),
    (30.0, -7.4),
    (33.0, -9.1),
    (36.0, -12.0),
    (39.0, -16.0),
    (42.0, -19.0),
    (45.0, -24.0),
    (48.0, -31.0),
    (51.0, -38.0),
    (54.0, -45.0),
    (57.0, -54.0),
    (60.0, -68.0),
    (63.0, -95.0),
    (66.0, -100.0),
];

const FADER_ZERO_TICK: f64 = 17.0;

const FADER_TICKS: [f64; 9] = [
    0.0,
    FADER_ZERO_TICK,
    35.0,
    42.7,
    47.7,
    51.7,
    57.0,
    60.0,
    63.0,
];

const STRIP_SPARE_W: f32 = 66.0;

const STRIP_ROOM: f32 = 106.0;

const DUCK_ROW: f32 = 24.0;

fn ducking_room(p: &Project) -> f32 {
    let ducked = p
        .tracks
        .iter()
        .any(|t| t.duck.as_ref().is_some_and(|d| d.on));
    DUCK_ROW + if ducked { 3.0 * DUCK_ROW } else { 0.0 }
}

fn linear(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

fn db(level: f32) -> f32 {
    if level <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * level.log10()
    }
}

fn position(level: f32) -> f32 {
    ((db(level) + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn fader_db(px: f64) -> f64 {
    let px = px.clamp(0.0, FADER_TRAVEL);
    let at = FADER_LAW
        .partition_point(|&(p, _)| p <= px)
        .clamp(1, FADER_LAW.len() - 1);
    let ((p0, d0), (p1, d1)) = (FADER_LAW[at - 1], FADER_LAW[at]);
    d0 + (d1 - d0) * (px - p0) / (p1 - p0)
}

fn fader_px(db: f64) -> f64 {
    let at = FADER_LAW
        .partition_point(|&(_, d)| d >= db)
        .clamp(1, FADER_LAW.len() - 1);
    let ((p0, d0), (p1, d1)) = (FADER_LAW[at - 1], FADER_LAW[at]);
    (p0 + (p1 - p0) * (db - d0) / (d1 - d0)).clamp(0.0, FADER_TRAVEL)
}

fn fader_text(db: f64) -> String {
    if vidcut_core::validate::fader_silent(db) {
        "-∞ dB".into()
    } else {
        format!("{db:+.1} dB")
    }
}

fn fader_y(px: f64, rect: Rect, handle: egui::style::HandleShape) -> f32 {
    let mut radius = rect.width() / 2.5;
    if let egui::style::HandleShape::Rect { aspect_ratio } = handle {
        radius *= aspect_ratio;
    }
    let (top, bottom) = (rect.top() + radius, rect.bottom() - radius);
    top + (bottom - top) * (px / FADER_TRAVEL) as f32
}

fn fader_ticks(slider: Rect, handle: egui::style::HandleShape) -> egui::Shape {
    let ticks = FADER_TICKS
        .iter()
        .map(|&px| {
            let (reach, stroke) = if px == FADER_ZERO_TICK {
                (3.0, Stroke::new(2.0, theme::colors().text))
            } else {
                (-2.0, Stroke::new(1.0, theme::colors().text_dim))
            };
            let y = fader_y(px, slider, handle);
            egui::Shape::line_segment(
                [
                    pos2(slider.left() - reach, y),
                    pos2(slider.right() + reach, y),
                ],
                stroke,
            )
        })
        .collect();
    egui::Shape::Vec(ticks)
}

fn project_channels(p: &Project) -> usize {
    usize::from(p.settings.channels.max(1))
}

fn meter_column_width(n: usize) -> f32 {
    let n = n.max(1);
    BAR_W * n as f32 + 2.0 * (n - 1) as f32
}

fn map_channels(src: &[f32], n: usize) -> Vec<f32> {
    let mut out = vec![0.0; n.max(1)];
    if src.len() == 1 && out.len() > 1 {
        out.fill(src[0]);
    } else {
        for (o, s) in out.iter_mut().zip(src) {
            *o = *s;
        }
    }
    out
}

type Waves = HashMap<String, std::sync::Arc<vidcut_av::waveform::Waveform>>;

fn source_peaks(w: &vidcut_av::waveform::Waveform, a: f64, b: f64) -> Vec<f32> {
    if w.channels == 0 {
        let peak = w
            .range(a, b)
            .map(|(lo, hi)| lo.abs().max(hi.abs()))
            .unwrap_or(0.0);
        return vec![peak];
    }
    (0..usize::from(w.channels))
        .map(|ch| w.channel_peak(ch, a, b))
        .collect()
}

fn track_channels(p: &Project, t: &Track, at: i64, waveforms: &Waves) -> Vec<f32> {
    let n = project_channels(p);
    let fps = p.settings.fps.as_f64();
    let mut level = vec![0.0; n];
    let fades = vidcut_core::otio::transitions::cross_fades(p, t);
    for c in &t.clips {
        let Some(audio) = c.audio.as_ref().filter(|a| !a.muted) else {
            continue;
        };
        let Some(share) = vidcut_core::otio::transitions::heard_at(&fades, c, at) else {
            continue;
        };
        let Some(w) = waveforms.get(&c.media) else {
            continue;
        };
        let origin = p.media(&c.media).map_or(0.0, |m| m.origin());
        let s = c.source_at(at) as f64 / fps + origin;
        let gained = map_channels(&source_peaks(w, s, s + 1.0 / fps), n);
        let gain = c.gain_at(at as f64).unwrap_or(audio.gain_db);
        let g = linear(gain + t.volume_db) * share as f32;
        for (dst, src) in level.iter_mut().zip(gained) {
            *dst = f32::max(*dst, src * g);
        }
    }
    level
}

fn sum_channels(tracks: &[Vec<f32>], n: usize) -> Vec<f32> {
    let mut out = vec![0.0; n];
    for t in tracks {
        for (o, s) in out.iter_mut().zip(t) {
            *o += *s;
        }
    }
    out
}

struct TimelineMeters {
    tracks: Vec<(String, Vec<f32>)>,
    mix: Vec<f32>,
}

fn timeline_meters(p: &Project, at: i64, waveforms: &Waves) -> TimelineMeters {
    let n = project_channels(p);
    let any_solo = p.any_solo();
    let mut audible = Vec::new();
    let tracks = p
        .tracks
        .iter()
        .map(|t| {
            let raw = track_channels(p, t, at, waveforms);
            if t.audible(any_solo) {
                audible.push(raw.clone());
                (t.id.clone(), raw)
            } else {
                (t.id.clone(), vec![0.0; n])
            }
        })
        .collect();
    let mix = sum_channels(&audible, n);
    TimelineMeters { tracks, mix }
}

fn heard_meters(
    p: &Project,
    levels: &vidcut_av::player::Levels,
    lanes: &[String],
) -> TimelineMeters {
    let n = project_channels(p);
    let tracks = p
        .tracks
        .iter()
        .map(|t| {
            let mut level = vec![0.0; n];
            for (lane, peaks) in lanes.iter().zip(&levels.lanes) {
                if *lane == t.id {
                    for (dst, src) in level.iter_mut().zip(map_channels(peaks, n)) {
                        *dst = f32::max(*dst, src);
                    }
                }
            }
            (t.id.clone(), level)
        })
        .collect();
    TimelineMeters {
        tracks,
        mix: map_channels(&levels.mix, n),
    }
}

fn loudest(levels: &[f32]) -> f32 {
    levels.iter().copied().fold(0.0, f32::max)
}

fn held_level(prev: Option<(f32, f32)>, level: f32) -> f32 {
    match prev {
        Some((h, age)) if h >= level && age < HOLD_SECS => h,
        _ => level,
    }
}

impl Workspace {
    fn timeline_levels(&self) -> TimelineMeters {
        let Some(p) = self.project() else {
            return TimelineMeters {
                tracks: Vec::new(),
                mix: Vec::new(),
            };
        };
        let timeline = &self.snap.timeline;
        if let Some(levels) = self
            .levels
            .as_ref()
            .filter(|_| self.snap.viewer == ViewerMode::Timeline && timeline.playing)
        {
            return heard_meters(p, levels, &timeline.lanes);
        }
        if self.mixer.dragging.is_empty() {
            return timeline_meters(p, self.snap.timeline.position, &self.snap.waveforms);
        }
        let mut p = p.clone();
        for t in &mut p.tracks {
            if let Some(v) = self.mixer.dragging.get(&t.id) {
                t.volume_db = *v;
            }
        }
        timeline_meters(&p, self.snap.timeline.position, &self.snap.waveforms)
    }

    fn viewer_channels(&self) -> Vec<f32> {
        let Some(p) = self.project() else {
            return vec![0.0; 2];
        };
        let n = project_channels(p);
        if self.snap.player(crate::live::PlayerId::Timeline).muted {
            return vec![0.0; n];
        }
        if self.snap.viewer == ViewerMode::Timeline {
            return self.timeline_levels().mix;
        }
        if let Some(levels) = self.levels.as_ref().filter(|_| self.snap.active().playing) {
            return map_channels(&levels.mix, n);
        }
        let Some((media, frame)) = self.snap.source_at() else {
            return vec![0.0; n];
        };
        let Some(w) = self.snap.waveforms.get(media) else {
            return vec![0.0; n];
        };
        let fps = p.settings.fps.as_f64();
        let origin = p.media(media).map_or(0.0, |m| m.origin());
        let s = frame as f64 / fps + origin;
        map_channels(&source_peaks(w, s, s + 1.0 / fps), n)
    }

    pub(super) fn viewer_meter_width(&self) -> f32 {
        let n = self
            .project()
            .map_or(2, |p| usize::from(p.settings.channels.max(1)));
        meter_column_width(n)
    }

    fn hold(&mut self, key: &str, level: f32) -> f32 {
        let now = Instant::now();
        let prev = self.mixer.holds.get(key).copied();
        let age = prev.map(|(_, at)| at.elapsed().as_secs_f32());
        let kept = held_level(prev.map(|(h, _)| (h, age.unwrap_or(0.0))), level);
        let still = prev.is_some_and(|(h, _)| h >= level && age.unwrap_or(0.0) < HOLD_SECS);
        if !still {
            self.mixer.holds.insert(key.to_string(), (kept, now));
        }
        kept
    }

    fn hold_channels(&mut self, key: &str, levels: &[f32]) -> Vec<f32> {
        levels
            .iter()
            .enumerate()
            .map(|(i, level)| self.hold(&format!("{key}:{i}"), *level))
            .collect()
    }

    const METER_FRAME: std::time::Duration = std::time::Duration::from_millis(33);

    fn meters_live(&self, levels: &[f32], playing: bool) -> bool {
        playing
            || levels.iter().any(|l| *l > 0.0)
            || self.mixer.holds.iter().any(|(_, (h, _))| *h > 0.0)
    }

    pub(super) fn mixer_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project_shared() else {
            return;
        };
        ui.horizontal(|ui| {
            ui.heading("Mixer");
            ui.label(dim("Mute and solo apply to export too"));
        });
        ui.separator();
        let meters = self.timeline_levels();
        let height =
            (ui.available_height().clamp(160.0, 320.0) - ducking_room(&p)).max(STRIP_ROOM + 40.0);
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for t in p.tracks.iter() {
                    if !t.has_sound() {
                        continue;
                    }
                    let level = meters
                        .tracks
                        .iter()
                        .find(|(id, _)| id == &t.id)
                        .map(|l| l.1.clone())
                        .unwrap_or_else(|| vec![0.0; project_channels(&p)]);
                    self.strip(ui, &p, t, &level, height);
                    ui.separator();
                }
                self.master_strip(ui, &meters.mix, height);
            });
        });
        if self.meters_live(&meters.mix, self.snap.timeline.playing) {
            ui.ctx().request_repaint_after(Self::METER_FRAME);
        }
    }

    fn strip(&mut self, ui: &mut egui::Ui, p: &Project, t: &Track, level: &[f32], height: f32) {
        let n = level.len().max(1);
        ui.vertical(|ui| {
            ui.set_width(meter_column_width(n) + STRIP_SPARE_W);
            let name = t.name.clone().unwrap_or_else(|| t.id.clone());
            ui.label(RichText::new(name).strong());
            let held = self.hold_channels(&t.id, level);
            let bars = height - STRIP_ROOM;
            let (number, _) = ui.allocate_exact_size(
                vec2(
                    ui.available_width(),
                    ui.text_style_height(&egui::TextStyle::Small),
                ),
                egui::Sense::hover(),
            );
            let row = ui.horizontal_top(|ui| {
                self.channel_bars(ui, level, &held, bars, "track meter");
                let mut value = self
                    .mixer
                    .dragging
                    .get(&t.id)
                    .copied()
                    .unwrap_or(t.volume_db);
                ui.spacing_mut().slider_width = bars;
                let under = ui.painter().add(egui::Shape::Noop);
                let mut up = FADER_TRAVEL - fader_px(value);
                let r = ui.add(
                    egui::Slider::new(&mut up, 0.0..=FADER_TRAVEL)
                        .vertical()
                        .show_value(false)
                        .smart_aim(false),
                );
                if r.changed() {
                    value = (fader_db(FADER_TRAVEL - up) * 10.0).round() / 10.0;
                }
                let handle = ui.style().visuals.handle_shape;
                ui.painter().set(under, fader_ticks(r.rect, handle));
                if r.double_clicked() {
                    self.mixer.dragging.remove(&t.id);
                    self.send("track.set", json!({ "track": t.id, "volume_db": 0.0 }));
                } else if r.dragged() || r.changed() {
                    if r.dragged() && self.mixer.dragging.get(&t.id) != Some(&value) {
                        self.send(
                            "mixer.preview",
                            json!({ "track": t.id, "volume_db": value }),
                        );
                    }
                    self.mixer.dragging.insert(t.id.clone(), value);
                }
                if (r.drag_stopped() || (r.changed() && !r.dragged()))
                    && let Some(v) = self.mixer.dragging.remove(&t.id)
                    && v != t.volume_db
                {
                    self.send("track.set", json!({ "track": t.id, "volume_db": v }));
                }
                (value, r.rect)
            });
            let (value, fader) = row.inner;
            let over =
                Rect::from_center_size(pos2(fader.center().x, number.center().y), number.size());
            ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(over)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            )
            .add(egui::Label::new(RichText::new(fader_text(value)).small().monospace()).extend());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if ui.selectable_label(t.muted, "M").clicked() {
                    self.send("track.set", json!({ "track": t.id, "muted": !t.muted }));
                }
                if ui.selectable_label(t.solo, "S").clicked() {
                    self.send("track.set", json!({ "track": t.id, "solo": !t.solo }));
                }
            });
            let on = super::isolation::Isolate::Track(t.id.clone());
            let amount = t.voice_isolation.filter(|v| v.on).map(|v| v.amount);
            self.isolation_controls(ui, on, amount, "Voice Isolation");
            self.ducking_controls(ui, p, t);
        });
    }

    fn master_strip(&mut self, ui: &mut egui::Ui, level: &[f32], height: f32) {
        ui.vertical(|ui| {
            ui.set_width(meter_column_width(level.len()).max(48.0));
            ui.add(egui::Label::new(RichText::new("Mix").strong()).extend());
            let held = self.hold_channels("mix", level);
            let bars = height - STRIP_ROOM;
            self.channel_bars(ui, level, &held, bars, "mix meter");
            let peak = db(loudest(&held));
            let text = if peak.is_finite() {
                format!("{peak:.1} dB")
            } else {
                "-∞ dB".into()
            };
            ui.add(egui::Label::new(RichText::new(text).small().monospace()).extend());
        });
    }

    fn channel_bars(
        &mut self,
        ui: &mut egui::Ui,
        levels: &[f32],
        held: &[f32],
        height: f32,
        label: &str,
    ) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let n = levels.len().max(1);
            for i in 0..n {
                let level = levels.get(i).copied().unwrap_or(0.0);
                let hold = held.get(i).copied().unwrap_or(level);
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(BAR_W, height.max(1.0)), egui::Sense::hover());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, label));
                meter(ui.painter(), rect, level, hold);
            }
        });
    }

    pub(super) fn viewer_meter(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let level = self.viewer_channels();
        let held = self.hold_channels("viewer", &level);
        let bar = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 22.0));
        paint_meters(ui.painter(), bar, &level, &held);
        let muted = self.snap.player(crate::live::PlayerId::Timeline).muted;
        let button = Rect::from_min_max(
            pos2(rect.left() - 4.0, rect.bottom() - 20.0),
            rect.max + vec2(4.0, 0.0),
        );
        let r = ui.put(
            button,
            egui::Button::selectable(muted, if muted { "🔇" } else { "🔊" }).small(),
        );
        if r.clicked() {
            self.send("player.mute", json!({ "mute": !muted }));
        }
        let playing = self.snap.active().playing;
        if self.meters_live(&level, playing) {
            ui.ctx().request_repaint_after(Self::METER_FRAME);
        }
    }
}

fn meter(painter: &egui::Painter, rect: Rect, level: f32, hold: f32) {
    painter.rect_filled(rect, CornerRadius::same(2), theme::colors().panel_dark);
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

fn paint_meters(painter: &egui::Painter, rect: Rect, levels: &[f32], held: &[f32]) {
    let n = levels.len().max(1);
    let gap = if n > 1 { 2.0 } else { 0.0 };
    let w = ((rect.width() - gap * (n - 1) as f32) / n as f32).max(1.0);
    for i in 0..n {
        let x = rect.left() + i as f32 * (w + gap);
        let bar = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
        meter(
            painter,
            bar,
            levels.get(i).copied().unwrap_or(0.0),
            held.get(i).copied().unwrap_or(0.0),
        );
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn channels_map_onto_the_project() {
        assert_eq!(map_channels(&[0.5, 0.25], 2), vec![0.5, 0.25]);
        assert_eq!(map_channels(&[0.4], 2), vec![0.4, 0.4], "mono fills stereo");
        assert_eq!(map_channels(&[0.1, 0.2, 0.9], 2), vec![0.1, 0.2]);
        assert_eq!(
            map_channels(&[0.1, 0.2], 4),
            vec![0.1, 0.2, 0.0, 0.0],
            "a stereo source leaves the extra project channels silent"
        );
    }

    #[test]
    fn peak_hold_is_per_channel_for_one_and_a_half_seconds() {
        assert_eq!(held_level(None, 0.2), 0.2);
        assert_eq!(held_level(Some((0.8, 1.0)), 0.3), 0.8);
        assert_eq!(
            held_level(Some((0.8, 1.4)), 0.9),
            0.9,
            "a new peak replaces"
        );
        assert_eq!(held_level(Some((0.8, HOLD_SECS)), 0.3), 0.3);
        let left = held_level(Some((0.9, 0.2)), 0.1);
        let right = held_level(Some((0.2, 0.2)), 0.4);
        assert_eq!((left, right), (0.9, 0.4));
    }
}
