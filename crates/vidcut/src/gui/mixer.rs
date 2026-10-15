//! The mixer and the audio meters: a channel strip
//! per track with a meter, a fader, mute and solo, and the viewer's meter
//! with the monitoring mute.
//!
//! Each meter is one bar per project channel (spec D28). While the viewer
//! plays, the bars show the player's sound as it is heard: the mix, and
//! each track's lane. Paused, they read the cached waveform peaks at the
//! playhead, with clip gain, faders, mute and solo applied, so they move
//! while paused and scrubbing too.

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
    /// The highest level lately and when it was reached, by meter bar.
    holds: HashMap<String, (f32, Instant)>,
    /// Fader values being dragged (sent on release: one undo step).
    dragging: HashMap<String, f64>,
}

/// How long a peak hold stays up.
const HOLD_SECS: f32 = 1.5;

/// Width of one channel bar.
const BAR_W: f32 = 10.0;

/// A fader's range, dB.
const FADER_DB: std::ops::RangeInclusive<f64> = -60.0..=24.0;

/// The dB marks beside a fader, in the order they are kept when there
/// isn't room for all: 0 dB first.
const FADER_MARKS: [f32; 7] = [0.0, 12.0, -12.0, -24.0, 24.0, -60.0, -40.0];

/// Width of the numbers' column beside a fader.
const FADER_SCALE_W: f32 = 26.0;

/// Height a strip keeps for its name, value, and mute and solo buttons.
const STRIP_ROOM: f32 = 106.0;

/// Font of the scales' numbers.
fn scale_font() -> egui::FontId {
    egui::FontId::proportional(10.0)
}

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

/// The marks that fit along a scale, as `(dB, y)`: each at least `gap`
/// from the ones kept before it, taken in `marks` order (the first always
/// fits).
fn fitting_marks(marks: &[f32], y: impl Fn(f32) -> f32, gap: f32) -> Vec<(f32, f32)> {
    let mut kept: Vec<(f32, f32)> = Vec::new();
    for &m in marks {
        let at = y(m);
        if kept.iter().all(|&(_, k)| (k - at).abs() >= gap) {
            kept.push((m, at));
        }
    }
    kept
}

/// A scale's number: `+12`, `0`, `−12`.
fn mark_text(db: f32) -> String {
    match db {
        d if d > 0.0 => format!("+{d}"),
        d if d < 0.0 => format!("−{}", -d),
        _ => "0".to_string(),
    }
}

/// Where `db` sits on a vertical slider over [`FADER_DB`] in `rect`, as
/// egui places its handle: the travel is the rect less the handle's half
/// length at each end, the top end the most.
fn fader_y(db: f32, rect: Rect, handle: egui::style::HandleShape) -> f32 {
    let mut radius = rect.width() / 2.5;
    if let egui::style::HandleShape::Rect { aspect_ratio } = handle {
        radius *= aspect_ratio;
    }
    let (lo, hi) = (*FADER_DB.start() as f32, *FADER_DB.end() as f32);
    let t = (db.clamp(lo, hi) - lo) / (hi - lo);
    let (bottom, top) = (rect.bottom() - radius, rect.top() + radius);
    bottom + (top - bottom) * t
}

/// A tick across a fader's slider at each mark, to go under its rail and
/// handle: 0 dB is longer and thicker, in the accent color.
fn fader_ticks(slider: Rect, marks: &[(f32, f32)]) -> egui::Shape {
    let ticks = marks
        .iter()
        .map(|&(db, y)| {
            let (reach, stroke) = if db == 0.0 {
                (3.0, Stroke::new(2.0, theme::ACCENT))
            } else {
                (-2.0, Stroke::new(1.0, theme::TEXT_DIM))
            };
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

/// The marks' numbers in `column`, right of a fader, each centered on its
/// level; 0 in the accent color.
fn fader_numbers(painter: &egui::Painter, column: Rect, marks: &[(f32, f32)]) {
    for &(db, y) in marks {
        let color = if db == 0.0 {
            theme::ACCENT
        } else {
            theme::TEXT_DIM
        };
        // The ends' numbers stay whole inside the column.
        let y = y.clamp(column.top() + 5.0, column.bottom() - 5.0);
        painter.text(
            pos2(column.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            mark_text(db),
            scale_font(),
            color,
        );
    }
}

/// The project's channel count, at least one. Stereo is the usual case.
fn project_channels(p: &Project) -> usize {
    usize::from(p.settings.channels.max(1))
}

/// Width of a column of `n` channel bars, including the gaps.
fn meter_column_width(n: usize) -> f32 {
    let n = n.max(1);
    BAR_W * n as f32 + 2.0 * (n - 1) as f32
}

/// Place a waveform's channel peaks onto `n` project channels.
///
/// Waveforms are measured mixed to the project's channels, as the export
/// mixes (D28), so the counts match. Until a waveform measured for another
/// count is measured again, a mono one fills every channel and any other
/// keeps channel `i`.
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

/// Linear peaks of `w` over `[a, b)`, one per stored channel. A waveform
/// with no per-channel data contributes its mono peak as a single channel.
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

/// The media level of `t` at timeline frame `at`, one linear peak per
/// project channel, from its clips' waveforms, with clip gain and the fader.
/// Clips on a track don't overlap, so a frame has one clip's peaks.
fn track_channels(p: &Project, t: &Track, at: i64, waveforms: &Waves) -> Vec<f32> {
    let n = project_channels(p);
    let fps = p.settings.fps.as_f64();
    let mut level = vec![0.0; n];
    for c in &t.clips {
        let Some(audio) = c.audio.as_ref().filter(|a| !a.muted) else {
            continue;
        };
        // Through a cross fade, both clips' sounds at their shares.
        let Some(share) = vidcut_core::otio::transitions::heard_at(p, t, c, at) else {
            continue;
        };
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
        let gained = map_channels(&source_peaks(w, s, s + 1.0 / fps), n);
        let g = linear(audio.gain_db + t.volume_db) * share as f32;
        for (dst, src) in level.iter_mut().zip(gained) {
            *dst = f32::max(*dst, src * g);
        }
    }
    level
}

/// Sum per channel, as the preview's `amix=normalize=0` sums lanes.
fn sum_channels(tracks: &[Vec<f32>], n: usize) -> Vec<f32> {
    let mut out = vec![0.0; n];
    for t in tracks {
        for (o, s) in out.iter_mut().zip(t) {
            *o += *s;
        }
    }
    out
}

/// One strip's bars and the monitored mix.
struct TimelineMeters {
    tracks: Vec<(String, Vec<f32>)>,
    mix: Vec<f32>,
}

/// Levels at timeline frame `at`. Track strips show that track's channels,
/// silent when mute or solo says it is not heard. The mix sums the audible
/// tracks per channel, as the preview does.
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

/// The meters from the sound heard while playing: each track shows its
/// lanes' peaks (`lanes` names each lane's track), silent without one (not
/// heard), and the mix the mix's.
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

/// The loudest channel, for the number under the mix meter.
fn loudest(levels: &[f32]) -> f32 {
    levels.iter().copied().fold(0.0, f32::max)
}

/// Peak hold for one bar. A level at least as high as the held one replaces
/// it; a lower one waits until `age_secs` reaches [`HOLD_SECS`].
fn held_level(prev: Option<(f32, f32)>, level: f32) -> f32 {
    match prev {
        Some((h, age)) if h >= level && age < HOLD_SECS => h,
        _ => level,
    }
}

impl Workspace {
    /// Levels at the timeline playhead: each track's channels, and the mix.
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
        // A fader being dragged is heard at its new level; so it shows.
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

    /// The channels the viewer plays: the monitored mix in Timeline mode,
    /// the source media at the source playhead otherwise (no fader).
    /// `player.mute` silences it.
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
        let origin = p.media(media).map_or(0.0, |m| {
            m.info
                .video
                .as_ref()
                .map(|v| v.first_pts)
                .or_else(|| m.info.audio.first().map(|a| a.first_pts))
                .unwrap_or(0.0)
        });
        let s = frame as f64 / fps + origin;
        map_channels(&source_peaks(w, s, s + 1.0 / fps), n)
    }

    /// How wide the viewer's meter column is.
    pub(super) fn viewer_meter_width(&self) -> f32 {
        let n = self
            .project()
            .map_or(2, |p| usize::from(p.settings.channels.max(1)));
        meter_column_width(n)
    }

    /// Updates one bar's peak hold; returns the level to hold.
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

    /// How often live meters ask for a frame: they move smoothly at 30 a
    /// second. Asking for every frame instead redraws as fast as the CPU
    /// allows, since vsync stays off.
    const METER_FRAME: std::time::Duration = std::time::Duration::from_millis(33);

    fn meters_live(&self, levels: &[f32], playing: bool) -> bool {
        playing
            || levels.iter().any(|l| *l > 0.0)
            || self.mixer.holds.values().any(|(h, _)| *h > 0.0)
    }

    /// The Mixer panel: a strip per track and the mix.
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
                    let level = meters
                        .tracks
                        .iter()
                        .find(|(id, _)| id == &t.id)
                        .map(|l| l.1.clone())
                        .unwrap_or_else(|| vec![0.0; project_channels(&p)]);
                    self.strip(ui, t, &level, height);
                    ui.separator();
                }
                self.master_strip(ui, &meters.mix, height);
            });
        });
        if self.meters_live(&meters.mix, self.snap.timeline.playing) {
            ui.ctx().request_repaint_after(Self::METER_FRAME);
        }
    }

    fn strip(&mut self, ui: &mut egui::Ui, t: &Track, level: &[f32], height: f32) {
        let n = level.len().max(1);
        ui.vertical(|ui| {
            ui.set_width(meter_column_width(n) + FADER_SCALE_W + 40.0);
            let name = t.name.clone().unwrap_or_else(|| t.id.clone());
            ui.label(RichText::new(name).strong()).on_hover_text(&t.id);
            let held = self.hold_channels(&t.id, level);
            let bars = height - STRIP_ROOM;
            // Top-aligned: the bars, fader and scale share one top and height.
            let value = ui.horizontal_top(|ui| {
                self.channel_bars(ui, level, &held, bars, "track meter");
                // The fader: heard at once while dragged (`mixer.preview`),
                // kept when it is let go (one `track.set`).
                let mut value = self
                    .mixer
                    .dragging
                    .get(&t.id)
                    .copied()
                    .unwrap_or(t.volume_db);
                // A vertical slider is `slider_width` long.
                ui.spacing_mut().slider_width = bars;
                // The ticks go under the slider's rail and handle.
                let under = ui.painter().add(egui::Shape::Noop);
                let r = ui
                    .add(
                        egui::Slider::new(&mut value, FADER_DB)
                            .vertical()
                            .show_value(false)
                            .step_by(0.5),
                    )
                    .on_hover_text("Track volume; double-click for 0 dB");
                let handle = ui.style().visuals.handle_shape;
                let marks = fitting_marks(
                    &FADER_MARKS,
                    |db| fader_y(db, r.rect, handle),
                    scale_font().size + 4.0,
                );
                ui.painter().set(under, fader_ticks(r.rect, &marks));
                let (column, _) = ui.allocate_exact_size(
                    vec2(FADER_SCALE_W, r.rect.height()),
                    egui::Sense::hover(),
                );
                fader_numbers(ui.painter(), column, &marks);
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
                value
            });
            ui.label(
                RichText::new(format!("{:+.1} dB", value.inner))
                    .small()
                    .monospace(),
            );
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

    fn master_strip(&mut self, ui: &mut egui::Ui, level: &[f32], height: f32) {
        ui.vertical(|ui| {
            // Its own width, as the track strips have: last in the row, it
            // would otherwise get what is left, a letter wide.
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

    /// One bar per channel. `label` is how tests count the bars.
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
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label)
                });
                meter(ui.painter(), rect, level, hold);
            }
        });
    }

    /// The meter beside the viewer with the monitoring mute.
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
        if r.on_hover_text("Mute what the viewer plays (not the export)")
            .clicked()
        {
            self.send("player.mute", json!({ "mute": !muted }));
        }
        let playing = self.snap.active().playing;
        if self.meters_live(&level, playing) {
            ui.ctx().request_repaint_after(Self::METER_FRAME);
        }
    }
}

/// A vertical meter: green, yellow from −12 dBFS, red from −3 dBFS,
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

/// Several channel bars side by side in `rect` (the viewer meter).
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
        // Channels don't share a hold: the right can fall while the left stays.
        let left = held_level(Some((0.9, 0.2)), 0.1);
        let right = held_level(Some((0.2, 0.2)), 0.4);
        assert_eq!((left, right), (0.9, 0.4));
    }
}
