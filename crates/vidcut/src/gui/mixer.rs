//! The mixer and the audio meters: a channel strip
//! per track with a meter, a fader, mute and solo, and the viewer's meter
//! with the monitoring mute.
//!
//! Each meter is one bar per project channel (spec D28). The bars read the
//! cached waveform peaks at the playhead, playing or not, with clip gain,
//! faders, mute and solo applied: they show the edit's levels rather than
//! the player's output, which mpv doesn't expose to the session.

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
        let gained = map_channels(&source_peaks(w, s, s + 1.0 / fps), n);
        let g = linear(audio.gain_db + t.volume_db);
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
        timeline_meters(p, self.snap.timeline.position, &self.snap.waveforms)
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

    fn meters_live(&self, levels: &[f32], playing: bool) -> bool {
        playing
            || levels.iter().any(|l| *l > 0.0)
            || self.mixer.holds.values().any(|(h, _)| *h > 0.0)
    }

    /// The Mixer panel: a strip per track and the mix.
    pub(super) fn mixer_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project().cloned() else {
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
            ui.ctx().request_repaint();
        }
    }

    fn strip(&mut self, ui: &mut egui::Ui, t: &Track, level: &[f32], height: f32) {
        let n = level.len().max(1);
        ui.vertical(|ui| {
            ui.set_width(64.0 + meter_column_width(n));
            let name = t.name.clone().unwrap_or_else(|| t.id.clone());
            ui.label(RichText::new(name).strong()).on_hover_text(&t.id);
            let held = self.hold_channels(&t.id, level);
            ui.horizontal(|ui| {
                self.channel_bars(ui, level, &held, height - 90.0, "track meter");
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

    fn master_strip(&mut self, ui: &mut egui::Ui, level: &[f32], height: f32) {
        ui.vertical(|ui| {
            ui.label(RichText::new("Mix").strong());
            let held = self.hold_channels("mix", level);
            self.channel_bars(ui, level, &held, height - 90.0, "mix meter");
            let peak = db(loudest(&held));
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

    /// One bar per channel. `label` is how tests count the bars.
    fn channel_bars(
        &mut self,
        ui: &mut egui::Ui,
        levels: &[f32],
        held: &[f32],
        height: f32,
        label: &str,
    ) {
        ui.horizontal(|ui| {
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
            ui.ctx().request_repaint();
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
    use std::sync::Arc;

    use vidcut_av::waveform::Waveform;
    use vidcut_core::model::{Clip, ClipAudio, Settings, TrackKind};
    use vidcut_core::time::Rational;

    use super::*;

    fn wave(channels: &[(i8, i8)]) -> Arc<Waveform> {
        let ch = channels.len().max(1) as u16;
        Arc::new(Waveform {
            per_second: 100,
            start: 0.0,
            peaks: vec![(0, 0)],
            channels: ch,
            channel_peaks: channels.to_vec(),
        })
    }

    fn project(channels: u16) -> Project {
        let settings = Settings {
            channels,
            fps: Rational::new(30, 1).expect("valid"),
            ..Settings::default()
        };
        Project::new(settings)
    }

    fn clip(id: &str, media: &str, gain_db: f64, muted: bool) -> Clip {
        Clip {
            id: id.into(),
            media: media.into(),
            start: 0,
            duration: 30,
            source_in: 0,
            video: true,
            audio: Some(ClipAudio {
                stream: 0,
                head: 0,
                tail: 0,
                gain_db,
                muted,
            }),
            enabled: true,
            name: None,
        }
    }

    /// i8 peak as the linear level `channel_peak` returns.
    fn lin(peak: i8) -> f32 {
        f32::from(peak) / 127.0
    }

    #[test]
    fn meter_scale() {
        assert_eq!(position(1.0), 1.0);
        assert_eq!(position(0.0), 0.0);
        assert!((position(linear(-30.0)) - 0.5).abs() < 1e-3);
    }

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
    fn each_channel_keeps_its_own_peak() {
        let mut p = project(2);
        p.tracks[0].clips.push(clip("c1", "m1", 0.0, false));
        let mut waves = Waves::new();
        // Left is loud, right is quiet: the old meter would show only the left.
        waves.insert("m1".into(), wave(&[(-80, 80), (0, 10)]));
        let m = timeline_meters(&p, 0, &waves);
        let bars = &m.tracks[0].1;
        assert!((bars[0] - lin(80)).abs() < 1e-6);
        assert!((bars[1] - lin(10)).abs() < 1e-6);
        assert!(bars[0] > bars[1]);
        assert!((loudest(bars) - bars[0]).abs() < 1e-6);
        assert_eq!(m.mix, *bars);
    }

    #[test]
    fn mute_solo_and_gain_apply_per_channel() {
        let mut p = project(2);
        p.tracks[0].clips.push(clip("c1", "m1", 6.0, false));
        p.tracks[0].volume_db = -6.0;
        p.tracks.push(Track::new("A1", TrackKind::Audio));
        p.tracks[1].clips.push(clip("c2", "m2", 0.0, false));
        let mut waves = Waves::new();
        waves.insert("m1".into(), wave(&[(-40, 40), (0, 20)]));
        waves.insert("m2".into(), wave(&[(0, 50), (-60, 60)]));
        let open = timeline_meters(&p, 0, &waves);
        // +6 dB of clip gain and −6 dB of fader cancel.
        assert!((open.tracks[0].1[0] - lin(40)).abs() < 1e-5);
        assert!((open.tracks[0].1[1] - lin(20)).abs() < 1e-5);
        assert!((open.mix[0] - (lin(40) + lin(50))).abs() < 1e-5);
        assert!((open.mix[1] - (lin(20) + lin(60))).abs() < 1e-5);

        p.tracks[1].muted = true;
        let muted = timeline_meters(&p, 0, &waves);
        assert_eq!(muted.tracks[1].1, vec![0.0, 0.0]);
        assert_eq!(muted.mix, muted.tracks[0].1);

        p.tracks[1].muted = false;
        p.tracks[1].solo = true;
        let solo = timeline_meters(&p, 0, &waves);
        assert_eq!(
            solo.tracks[0].1,
            vec![0.0, 0.0],
            "an unsoloed track is silent"
        );
        assert_eq!(solo.mix, solo.tracks[1].1);
        assert!(solo.mix[1] > solo.mix[0]);

        p.tracks[0].clips[0].audio.as_mut().unwrap().muted = true;
        p.tracks[1].solo = false;
        let clip_muted = timeline_meters(&p, 0, &waves);
        assert_eq!(clip_muted.tracks[0].1, vec![0.0, 0.0]);
        assert_eq!(clip_muted.mix, clip_muted.tracks[1].1);
    }

    #[test]
    fn mono_source_lights_every_project_channel() {
        let mut p = project(2);
        p.tracks[0].clips.push(clip("c1", "m1", 0.0, false));
        let mut waves = Waves::new();
        waves.insert("m1".into(), wave(&[(0, 64)]));
        let m = timeline_meters(&p, 0, &waves);
        assert_eq!(m.mix.len(), 2);
        assert!((m.mix[0] - lin(64)).abs() < 1e-6);
        assert_eq!(m.mix[0], m.mix[1]);
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
