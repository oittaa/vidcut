use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, pos2,
    vec2,
};
use vidcut_av::waveform::Waveform;
use vidcut_core::model::{Clip, Effects, Media, Project, Speed, Track, Transition};
use vidcut_core::ops;
use vidcut_core::otio::transitions;
use vidcut_core::render_cache::Part;
use vidcut_core::time::Rational;

use super::geometry::{
    Rows, Scale, TrackRect, chosen_on, clips_in, covered, curve_y, handle_point, key_point,
    sound_band, speed_keyframes, speed_labels, speed_strip, volume_y,
};
use super::gesture::{Preview, Target};
use crate::gui::pool::elide;
use crate::gui::theme;
use crate::gui::tiles::{self, TileKey, Tiles};
use crate::gui::workspace::Thumbs;

#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub names: bool,
    pub durations: bool,
    pub status: bool,
    pub filmstrips: bool,
    pub waveforms: bool,
    pub target: bool,
}

pub struct Assets<'a> {
    pub thumbs: &'a HashMap<String, Thumbs>,
    pub waveforms: &'a HashMap<String, Arc<Waveform>>,
    pub tiles: Option<&'a Tiles>,
    pub missing: &'a HashSet<String>,
    pub transcribed: &'a HashSet<String>,
    pub pool: &'a HashMap<&'a str, &'a Media>,
    pub titles: &'a HashMap<String, egui::TextureId>,
    pub keys: &'a [(String, i64)],
    pub audio_trim: &'a [String],
}

pub type Placed = HashMap<String, Vec<(i64, usize)>>;

pub const PULSE: f64 = 1.0;
pub const PULSE_STEP: std::time::Duration = std::time::Duration::from_millis(40);

pub fn pulse(time: f64) -> f32 {
    (0.66 + 0.06 * (time * std::f64::consts::TAU / PULSE).cos()) as f32
}

pub fn placed(p: &Project) -> Placed {
    p.tracks
        .iter()
        .filter(|t| !t.transitions.is_empty())
        .map(|t| {
            let cuts = transitions::Cuts::of(p, t);
            let mut placed: Vec<(i64, usize)> = t
                .transitions
                .iter()
                .enumerate()
                .filter_map(|(i, x)| Some((cuts.place(p, x)?, i)))
                .collect();
            placed.sort_by_key(|&(at, i)| (t.transitions[i].sound, at));
            (t.id.clone(), placed)
        })
        .collect()
}

pub fn marker_color(name: &str) -> Color32 {
    match name {
        "red" => Color32::from_rgb(220, 70, 60),
        "green" => Color32::from_rgb(90, 190, 90),
        "yellow" => Color32::from_rgb(230, 200, 60),
        "purple" => Color32::from_rgb(160, 100, 220),
        "cyan" => Color32::from_rgb(80, 200, 210),
        "pink" => Color32::from_rgb(230, 120, 180),
        "fuchsia" => Color32::from_rgb(200, 70, 200),
        "rose" => Color32::from_rgb(230, 110, 130),
        "lavender" => Color32::from_rgb(170, 150, 230),
        "sky" => Color32::from_rgb(120, 190, 240),
        "mint" => Color32::from_rgb(120, 220, 170),
        "lemon" => Color32::from_rgb(240, 240, 120),
        "sand" => Color32::from_rgb(210, 180, 130),
        "cocoa" => Color32::from_rgb(150, 100, 70),
        "cream" => Color32::from_rgb(240, 230, 200),
        _ => Color32::from_rgb(80, 140, 230),
    }
}

fn step(scale: &Scale, fps: f64, min_px: f64) -> f64 {
    let px_per_s = scale.ppf * fps;
    for s in [
        1.0 / fps,
        2.0 / fps,
        5.0 / fps,
        10.0 / fps,
        0.5,
        1.0,
        2.0,
        5.0,
        10.0,
        15.0,
        30.0,
        60.0,
        120.0,
        300.0,
        600.0,
        1800.0,
        3600.0,
    ] {
        if s * px_per_s >= min_px {
            return s;
        }
    }
    7200.0
}

const CACHE_LINE: f32 = 3.0;
const TO_CACHE: Color32 = Color32::from_rgb(225, 60, 60);
const CACHED: Color32 = Color32::from_rgb(60, 130, 235);

pub fn cache_line(painter: &Painter, rect: Rect, scale: &Scale, line: &[Part]) {
    let band = egui::Rangef::new(rect.bottom() - CACHE_LINE, rect.bottom());
    let (a, b) = scale.visible(rect.left(), rect.right());
    let fill = |from: i64, to: i64, color: Color32| {
        let x0 = scale.xf(from).max(rect.left());
        let x1 = scale.xf(to).min(rect.right());
        if x1 > x0 {
            painter.rect_filled(
                Rect::from_x_y_ranges(x0..=x1, band),
                CornerRadius::ZERO,
                color,
            );
        }
    };
    for p in line
        .iter()
        .filter(|p| p.start < b && p.start + p.frames > a)
    {
        fill(p.start, p.start + p.frames, TO_CACHE);
        for &(from, to) in &p.cached {
            fill(from, to, CACHED);
        }
    }
}

pub fn cache_reasons(rect: Rect, scale: &Scale, line: &[Part], pos: Pos2) -> Option<String> {
    let near = (rect.bottom() - 2.0 * CACHE_LINE..=rect.bottom() + 1.0).contains(&pos.y);
    if !near || !rect.x_range().contains(pos.x) {
        return None;
    }
    let frame = scale.under(pos.x);
    let p = line.iter().find(|p| p.holds(frame))?;
    let why = crate::live::stats::reason_names(&p.reasons);
    Some(match p.has(frame) {
        true => format!("Cached: {}", why.join(", ")),
        false => why.join(", "),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn ruler(
    painter: &Painter,
    rect: Rect,
    scale: &Scale,
    p: &Project,
    smart: Option<i64>,
    compact: bool,
) {
    painter.rect_filled(rect, CornerRadius::ZERO, theme::colors().panel_dark);
    let fps = p.settings.fps;
    let f = fps.as_f64();
    let s = step(scale, f, if compact { 110.0 } else { 90.0 });
    let (a, b) = scale.visible(rect.left(), rect.right());
    let first = ((a.max(0) as f64 / f) / s).floor() * s;
    let mut t = first;
    while t * f <= b as f64 {
        let frame = (t * f).round() as i64;
        let x = scale.xf(frame);
        if x >= rect.left() - 1.0 {
            painter.line_segment(
                [pos2(x, rect.bottom() - 5.0), pos2(x, rect.bottom())],
                Stroke::new(1.0, theme::colors().text_dim),
            );
            painter.text(
                pos2(x + 3.0, rect.top() + 1.0),
                Align2::LEFT_TOP,
                p.settings.timecode(frame),
                FontId::monospace(if compact { 9.0 } else { 10.0 }),
                theme::colors().text_dim,
            );
        }
        t += s;
    }
    if p.marks.mark_in.is_some() || p.marks.mark_out.is_some() {
        let x0 = scale.xf(p.marks.mark_in.unwrap_or(0));
        let x1 = scale.xf(p.marks.mark_out.unwrap_or(p.end()));
        painter.rect_filled(
            Rect::from_x_y_ranges(x0..=x1, (rect.bottom() - 6.0)..=rect.bottom()),
            CornerRadius::ZERO,
            Color32::from_rgb(170, 170, 180),
        );
        for (x, left) in [
            (p.marks.mark_in.map(|_| x0), true),
            (p.marks.mark_out.map(|_| x1), false),
        ] {
            if let Some(x) = x {
                let w = if left { 7.0 } else { -7.0 };
                let top = rect.bottom() - super::geometry::MARK_BAND;
                painter.add(egui::Shape::convex_polygon(
                    vec![pos2(x, top), pos2(x + w, top), pos2(x, rect.bottom())],
                    Color32::WHITE,
                    Stroke::NONE,
                ));
            }
        }
    }
    for k in &p.markers {
        let x = scale.xf(k.at);
        if x < rect.left() - 6.0 || x > rect.right() + 6.0 {
            continue;
        }
        let top = rect.bottom() - if compact { 8.0 } else { 11.0 };
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 5.0, top),
                pos2(x + 5.0, top),
                pos2(x, rect.bottom() - 1.0),
            ],
            marker_color(&k.color),
            Stroke::NONE,
        ));
        if !k.note.is_empty() {
            painter.circle_filled(pos2(x, top - 3.0), 1.5, marker_color(&k.color));
        }
    }
    if let Some(e) = smart {
        let x = scale.xf(e);
        let color = if ops::edit_has_handles(p, e) {
            theme::colors().ok
        } else {
            theme::colors().error
        };
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x, rect.bottom()),
                pos2(x - 5.0, rect.bottom() - 7.0),
                pos2(x + 5.0, rect.bottom() - 7.0),
            ],
            color,
            Stroke::NONE,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn track(
    painter: &Painter,
    tr: &TrackRect,
    scale: &Scale,
    p: &Project,
    t: &Track,
    selected: &HashSet<&str>,
    assets: &Assets,
    look: Look,
    placed: &[(i64, usize)],
) {
    let rect = tr.rect;
    let fill = if look.target {
        theme::colors().track_target
    } else {
        theme::colors().track
    };
    painter.rect_filled(rect, CornerRadius::ZERO, fill);
    let (a, b) = scale.visible(rect.left(), rect.right());
    let fps = p.settings.fps;
    let shown = &t.clips[clips_in(t, a - 1, b + 1)];
    let span = |c: &Clip| (c.start, c.end());
    let picked = |c: &Clip| selected.contains(c.id.as_str());
    for (i, c) in shown.iter().enumerate() {
        let next = shown.get(i + 1);
        if covered(scale, span(c), next.map(span)) && (!picked(c) || next.is_some_and(picked)) {
            continue;
        }
        clip(painter, tr, scale, p, t, c, picked(c), assets, look, fps);
    }
    let reach = |&(at, i): &(i64, usize)| {
        let x = &t.transitions[i];
        (at - x.before, at + x.after)
    };
    for (k, &(at, i)) in placed.iter().enumerate() {
        let x = &t.transitions[i];
        let next = placed
            .get(k + 1)
            .filter(|&&(_, j)| t.transitions[j].sound == x.sound);
        if at + x.after >= a
            && at - x.before <= b
            && !covered(scale, reach(&(at, i)), next.map(reach))
        {
            transition(painter, tr, scale, at, x);
        }
    }
    if t.locked {
        let mut x = rect.left() - rect.height();
        while x < rect.right() {
            painter.line_segment(
                [pos2(x, rect.bottom()), pos2(x + rect.height(), rect.top())],
                Stroke::new(1.0, Color32::from_black_alpha(90)),
            );
            x += 8.0;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn clip(
    painter: &Painter,
    tr: &TrackRect,
    scale: &Scale,
    p: &Project,
    t: &Track,
    c: &Clip,
    selected: bool,
    assets: &Assets,
    look: Look,
    fps: Rational,
) {
    let rect = tr.rect;
    let sound_only = c.audio.is_some() && assets.audio_trim.contains(&c.id);
    let tr = &TrackRect {
        rect,
        audio_top: if sound_only { rect.top() } else { tr.audio_top },
    };
    let (x0, x1) = (
        scale.xf(c.start),
        scale.xf(c.end()).max(scale.xf(c.start) + 1.0),
    );
    let video = Rect::from_x_y_ranges(x0..=x1, rect.top()..=tr.audio_top - 1.0);
    let track_painter = painter;
    let painter = painter.with_clip_rect(rect.intersect(Rect::from_x_y_ranges(
        (x0 - 1.0)..=(x1 + 1.0),
        rect.y_range(),
    )));
    let enabled = c.enabled && t.video_enabled;
    let offline = assets.missing.contains(&c.media);
    let media = assets.pool.get(c.media.as_str()).copied();
    let tint = c.shown_color(media).and_then(theme::clip_color);
    let fill = if offline {
        theme::colors().offline
    } else if !enabled {
        theme::colors().clip_disabled
    } else if let Some(t) = tint {
        t
    } else if c.is_generator() {
        theme::colors().generator
    } else if c.compound.is_some() {
        theme::colors().compound
    } else {
        theme::colors().clip
    };
    let has_picture = c.video && video.height() > 2.0;
    let corners = theme::clip_corners();
    if has_picture {
        painter.rect_filled(parted(video), corners, fill);
    } else if video.height() > 2.0 {
        painter.rect_stroke(
            video.shrink(1.0),
            corners,
            Stroke::new(1.0, theme::colors().clip_disabled),
            StrokeKind::Inside,
        );
    }
    let width = x1 - x0;
    if look.filmstrips
        && enabled
        && !offline
        && has_picture
        && video.height() >= 24.0
        && width > 24.0
    {
        filmstrip(
            &painter,
            video.shrink2(vec2(1.0, 1.0)),
            scale,
            p,
            c,
            assets,
            fps,
        );
        if let Some(t) = tint {
            painter.rect_filled(parted(video), corners, t.gamma_multiply(0.35));
        }
        if let Some(tex) = assets.titles.get(c.id.as_str()) {
            let inner = video.shrink2(vec2(1.0, 1.0));
            let aspect = p.settings.width as f32 / p.settings.height.max(1) as f32;
            let w = (inner.height() * aspect).min(inner.width());
            painter.with_clip_rect(inner).image(
                *tex,
                Rect::from_min_size(inner.min, vec2(w, inner.height())),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    if c.compound.is_some() && !c.video {
        let band = Rect::from_x_y_ranges(x0..=x1, tr.audio_top..=rect.bottom());
        painter.rect_filled(parted(band), corners, fill);
    }
    if let Some(audio) = sound_band(tr, scale, c) {
        let muted = c.audio.as_ref().is_some_and(|x| x.muted) || t.muted;
        let color = if offline {
            theme::colors().offline
        } else if muted {
            theme::colors().clip_disabled
        } else if let Some(t) = tint {
            let dim = |v: u8| (f32::from(v) * 0.8) as u8;
            Color32::from_rgb(dim(t.r()), dim(t.g()), dim(t.b()))
        } else {
            theme::colors().audio
        };
        let ap = track_painter.with_clip_rect(rect.intersect(audio.expand(1.0)));
        ap.rect_filled(parted(audio), corners, color);
        if (look.waveforms || sound_only) && !muted && !offline && audio.height() >= 8.0 {
            waveform(&ap, audio, scale, p, c, assets, fps);
        }
        volume_line(&ap, audio, scale, c, assets.keys);
        if muted && look.status {
            ap.text(
                audio.left_center() + vec2(4.0, 0.0),
                Align2::LEFT_CENTER,
                "muted",
                FontId::proportional(9.0),
                theme::colors().text_dim,
            );
        }
        if look.status
            && let Some(e) = c.audio.as_ref().and_then(|a| a.effects.as_ref())
        {
            effects_badge(&ap, audio, e);
        }
        if look.status
            && !c.video
            && speed_strip(tr, scale, c).is_none()
            && let Some(speed) = &c.speed
        {
            speed_badge(&ap, audio, speed);
        }
    }
    let strip = speed_strip(tr, scale, c);
    if let Some(strip) = strip {
        let bottom = if c.audio.is_some() {
            rect.bottom()
        } else {
            video.bottom()
        };
        speed_marks(&painter, tr, scale, c, strip, bottom);
    }
    if c.audio.as_ref().is_some_and(|a| a.is_keyed())
        || c.picture.as_ref().is_some_and(|p| p.is_keyed())
    {
        let band = if has_picture { video } else { rect };
        let top = strip.map_or(band.top(), |s| s.bottom());
        let at = pos2(x0.max(painter.clip_rect().left()) + 8.0, top + 8.0);
        let diamond = vec![
            at + vec2(0.0, -3.5),
            at + vec2(3.5, 0.0),
            at + vec2(0.0, 3.5),
            at + vec2(-3.5, 0.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            diamond,
            theme::colors().accent,
            Stroke::new(1.0, Color32::from_black_alpha(180)),
        ));
    }
    if look.names && width > 30.0 {
        let name = c
            .name
            .clone()
            .or_else(|| media.map(|m| m.name.clone()))
            .or_else(|| c.title.as_ref().map(vidcut_core::model::Title::label))
            .or_else(|| c.generator.as_ref().map(vidcut_core::otio::generator_kind))
            .or_else(|| c.compound.as_ref().map(vidcut_core::otio::compound_kind))
            .unwrap_or_default();
        let left = x0.max(painter.clip_rect().left());
        let told = c.audio.is_some() && assets.transcribed.contains(&c.media);
        let room = x1 - left - if told { 16.0 } else { 0.0 };
        let mut label = elide(&name, ((room - 8.0) / 6.5).max(3.0) as usize);
        if told {
            label = format!("{} {label}", egui_phosphor::regular::CHAT_TEXT);
        }
        let top = strip.map_or(video.top().min(tr.audio_top), |s| s.bottom());
        let pos = pos2(left + 4.0, top + 2.0);
        let galley = painter.layout_no_wrap(label, FontId::proportional(11.0), Color32::WHITE);
        painter.rect_filled(
            Rect::from_min_size(pos - vec2(2.0, 0.0), galley.size() + vec2(4.0, 1.0)),
            CornerRadius::same(2),
            Color32::from_black_alpha(110),
        );
        painter.galley(pos, galley, Color32::WHITE);
    }
    if look.durations && width > 70.0 && video.height() > 30.0 {
        let f = fps.as_f64().round().max(1.0) as i64;
        let text = format!("{}:{:02}", c.duration / f, c.duration % f);
        painter.text(
            pos2(
                x0.max(painter.clip_rect().left()) + 4.0,
                video.bottom() - 2.0,
            ),
            Align2::LEFT_BOTTOM,
            text,
            FontId::monospace(10.0),
            Color32::from_white_alpha(200),
        );
    }
    if look.status
        && has_picture
        && let Some(e) = &c.effects
    {
        effects_badge(&painter, video, e);
    }
    if look.status
        && has_picture
        && strip.is_none()
        && let Some(speed) = &c.speed
    {
        speed_badge(&painter, video, speed);
    }
    if look.status && width > 40.0 {
        let status = if offline {
            Some(("offline", Color32::WHITE))
        } else {
            (!c.enabled).then_some(("disabled", theme::colors().text_dim))
        };
        let band = if has_picture { Some(video) } else { None };
        if let (Some((text, color)), Some(band)) = (status, band) {
            painter.text(
                band.center(),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional(10.0),
                color,
            );
        }
    }
    if selected {
        painter.rect_stroke(
            Rect::from_x_y_ranges(x0..=x1, rect.y_range()),
            CornerRadius::same(2),
            Stroke::new(2.0, theme::colors().accent),
            StrokeKind::Inside,
        );
    }
}

fn parted(r: Rect) -> Rect {
    if r.width() >= 2.0 {
        r.shrink2(vec2(0.5, 0.0))
    } else {
        r
    }
}

fn transition(painter: &Painter, tr: &TrackRect, scale: &Scale, at: i64, x: &Transition) {
    let rect = tr.rect;
    let band = if x.sound {
        tr.audio_top..=rect.bottom()
    } else {
        rect.top()..=tr.audio_top - 1.0
    };
    let (x0, x1) = (scale.xf(at - x.before), scale.xf(at + x.after));
    let r = Rect::from_x_y_ranges(
        x0.min(scale.xf(at) - 2.0)..=x1.max(scale.xf(at) + 2.0),
        band,
    );
    if r.height() < 4.0 {
        return;
    }
    let painter = painter.with_clip_rect(rect);
    painter.rect_filled(r, CornerRadius::same(2), Color32::from_white_alpha(70));
    painter.line_segment(
        [r.left_bottom(), r.right_top()],
        Stroke::new(1.0, Color32::from_black_alpha(160)),
    );
    painter.rect_stroke(
        r,
        CornerRadius::same(2),
        Stroke::new(1.0, Color32::from_white_alpha(170)),
        StrokeKind::Inside,
    );
}

fn volume_line(painter: &Painter, band: Rect, scale: &Scale, c: &Clip, chosen: &[(String, i64)]) {
    let Some(a) = c.audio.as_ref() else {
        return;
    };
    let shadow = Stroke::new(1.0, Color32::from_black_alpha(140));
    let line = Stroke::new(1.0, Color32::from_white_alpha(230));
    if !a.is_keyed() {
        if let Some(y) = volume_y(band, a.gain_db) {
            let across = |y: f32| [pos2(band.left(), y), pos2(band.right(), y)];
            painter.line_segment(across(y + 1.0), shadow);
            painter.line_segment(across(y), line);
        }
        return;
    }
    let shown = band.intersect(painter.clip_rect());
    let (x0, x1) = (shown.left().floor(), shown.right().ceil());
    let mut points = Vec::new();
    let mut x = x0;
    while x <= x1 {
        if let Some(y) = curve_y(band, c, scale.frame(x)) {
            points.push(pos2(x, y));
        }
        x += 2.0;
    }
    if points.len() < 2 {
        return;
    }
    let lowered: Vec<Pos2> = points.iter().map(|p| *p + vec2(0.0, 1.0)).collect();
    painter.add(egui::Shape::line(lowered, shadow));
    painter.add(egui::Shape::line(points, line));
    let accent = theme::colors().accent;
    for (i, k) in a.gain_keys.iter().enumerate() {
        let Some(at) = key_point(band, scale, c, k) else {
            continue;
        };
        let picked = chosen_on(chosen, c, k.frame);
        if picked {
            for out in [false, true] {
                if let Some(h) = handle_point(band, scale, c, i, out) {
                    painter.line_segment([at, h], Stroke::new(1.0, accent));
                    painter.circle_filled(h, 2.5, accent);
                }
            }
        }
        let diamond = vec![
            at + vec2(0.0, -4.0),
            at + vec2(4.0, 0.0),
            at + vec2(0.0, 4.0),
            at + vec2(-4.0, 0.0),
        ];
        let fill = if picked { accent } else { Color32::WHITE };
        painter.add(egui::Shape::convex_polygon(
            diamond,
            fill,
            Stroke::new(1.0, Color32::from_black_alpha(180)),
        ));
    }
}

fn effects_badge(painter: &Painter, band: Rect, effects: &Effects) {
    let right = band.right().min(painter.clip_rect().right());
    let room = right - band.left().max(painter.clip_rect().left()) - 40.0;
    if room < 16.0 || band.height() < 13.0 {
        return;
    }
    let names = format!("fx {}", effects.names().join(", "));
    let fits = (room / 5.5) as usize;
    let text = if fits >= 6 {
        elide(&names, fits)
    } else {
        "fx".to_string()
    };
    tag(painter, text, |size| {
        pos2(right - size.x - 4.0, band.bottom() - size.y - 2.0)
    });
}

fn speed_badge(painter: &Painter, band: Rect, speed: &Speed) {
    let right = band.right().min(painter.clip_rect().right());
    if right - band.left().max(painter.clip_rect().left()) < 40.0 || band.height() < 13.0 {
        return;
    }
    tag(painter, speed.label(), |size| {
        pos2(right - size.x - 4.0, band.top() + 2.0)
    });
}

fn speed_marks(
    painter: &Painter,
    tr: &TrackRect,
    scale: &Scale,
    c: &Clip,
    strip: Rect,
    bottom: f32,
) {
    painter.rect_filled(strip, CornerRadius::ZERO, Color32::BLACK);
    let line = Stroke::new(2.0, Color32::WHITE);
    for t in speed_keyframes(c) {
        let x = scale.xf(t);
        painter.line_segment([pos2(x, strip.top()), pos2(x, bottom)], line);
        painter.rect_filled(
            Rect::from_center_size(pos2(x, strip.top() + 4.0), vec2(9.0, 8.0)),
            CornerRadius::same(1),
            Color32::WHITE,
        );
    }
    for l in speed_labels(tr, scale, c) {
        let color = if l.speed > 0.0 {
            Color32::WHITE
        } else {
            theme::colors().text_dim
        };
        let text = format!("{}%", (l.speed * 100.0).round());
        for dx in [0.0, 1.0] {
            painter.text(
                l.rect.center() + vec2(dx, 0.0),
                Align2::CENTER_CENTER,
                &text,
                FontId::proportional(12.0),
                color,
            );
        }
    }
}

fn tag(painter: &Painter, text: String, at: impl FnOnce(egui::Vec2) -> Pos2) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(9.0), theme::colors().accent);
    let pos = at(galley.size());
    painter.rect_filled(
        Rect::from_min_size(pos - vec2(2.0, 0.0), galley.size() + vec2(4.0, 1.0)),
        CornerRadius::same(2),
        Color32::from_black_alpha(140),
    );
    painter.galley(pos, galley, theme::colors().accent);
}

fn filmstrip(
    painter: &Painter,
    rect: Rect,
    scale: &Scale,
    p: &Project,
    c: &Clip,
    assets: &Assets,
    fps: Rational,
) {
    let Some(m) = assets.pool.get(c.media.as_str()) else {
        return;
    };
    let pool = assets.thumbs.get(&c.media);
    let aspect = pool
        .and_then(|t| t.textures.first())
        .map(|t| t.size_vec2().x / t.size_vec2().y.max(1.0))
        .or_else(|| tiles::media_aspect(m))
        .unwrap_or(16.0 / 9.0)
        .clamp(0.1, 10.0);
    let tile_w = (rect.height() * aspect).max(8.0);
    let height = tiles::tile_height(rect.height() * painter.ctx().pixels_per_point());
    let width = (height as f32 * aspect).round().max(1.0) as u32;
    let pace = c.pace();
    let fastest = pace.map.iter().map(|s| s.speed.abs()).fold(0.0, f64::max);
    let span = (f64::from(tile_w) / scale.ppf * fastest).ceil().max(1.0) as i64;
    let clip_rect = painter.clip_rect().intersect(rect);
    let mut x = rect.left();
    if clip_rect.left() > x {
        x += ((clip_rect.left() - x) / tile_w).floor() * tile_w;
    }
    while x < rect.right().min(clip_rect.right()) {
        let edge = if x <= rect.left() {
            0.0
        } else {
            scale.frame(x) - c.start as f64
        };
        let frame = tiles::tile_frame(c.source_in, &pace, edge, c.duration);
        let key = TileKey {
            media: c.media.clone(),
            frame,
            height,
        };
        let texture = assets
            .tiles
            .and_then(|t| {
                t.want(&key, || t.job(p, &key, width))
                    .or_else(|| t.nearest(&c.media, height, frame, span))
            })
            .or_else(|| pool?.nearest(fps.frames_to_secs(frame)));
        if let Some(texture) = texture {
            let tile = Rect::from_min_size(pos2(x, rect.top()), vec2(tile_w, rect.height()));
            painter.image(
                texture,
                tile,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::from_white_alpha(225),
            );
        }
        x += tile_w;
    }
}

fn waveform(
    painter: &Painter,
    rect: Rect,
    scale: &Scale,
    p: &Project,
    c: &Clip,
    assets: &Assets,
    fps: Rational,
) {
    let Some(w) = assets.waveforms.get(&c.media) else {
        return;
    };
    if c.is_frozen() {
        return;
    }
    let source = |at: f64| c.sound_at(at);
    let origin = p
        .media(&c.media)
        .map(|m| {
            m.info
                .video
                .as_ref()
                .map(|v| v.first_pts)
                .or_else(|| m.info.audio.first().map(|a| a.first_pts))
                .unwrap_or(0.0)
        })
        .unwrap_or(0.0);
    let visible = painter.clip_rect().intersect(rect);
    if visible.width() <= 0.0 {
        return;
    }
    let (mid, half) = (rect.center().y, rect.height() / 2.0 - 1.0);
    let f = fps.as_f64();
    let color = Color32::from_rgb(170, 230, 190);
    let mut x = visible.left().floor();
    while x < visible.right() {
        let (a, b) = (source(scale.frame(x)), source(scale.frame(x + 1.0)));
        let (t0, t1) = (a.min(b), a.max(b));
        if c.speed_at(scale.frame(x).floor() as i64) != 0.0
            && let Some((lo, hi)) = w.range(origin + t0 / f, origin + t1 / f)
        {
            painter.line_segment(
                [
                    pos2(x + 0.5, mid - hi * half),
                    pos2(x + 0.5, mid - lo * half + 0.5),
                ],
                Stroke::new(1.0, color),
            );
        }
        x += 1.0;
    }
}

pub fn playhead(painter: &Painter, x: f32, ruler: Rect, bottom: f32, color: Color32) {
    theme::playhead_glow(painter, [pos2(x, ruler.top()), pos2(x, bottom)], color);
    painter.line_segment(
        [pos2(x, ruler.top()), pos2(x, bottom)],
        Stroke::new(1.5, color),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(x - 5.0, ruler.top()),
            pos2(x + 5.0, ruler.top()),
            pos2(x, ruler.top() + 7.0),
        ],
        color,
        Stroke::NONE,
    ));
}

pub fn preview(painter: &Painter, rows: &Rows, scale: &Scale, p: &Project, pv: &Preview) {
    let track = pv.row.as_deref().unwrap_or(vidcut_core::ops::MAIN);
    let tr = rows.rect_of(track);
    let rect = tr.rect;
    let new_track = rows.get(track).is_none();
    let mut painter = painter.clone();
    if new_track {
        painter.set_clip_rect(painter.clip_rect().union(rect));
    }
    let painter = &painter;
    if new_track && (pv.ghost.is_some() || pv.request.is_some()) {
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_white_alpha(18));
        painter.text(
            pos2(rows.area.left() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!("new track {track}"),
            FontId::proportional(10.0),
            theme::colors().text_dim,
        );
    }
    let span = |a: i64, b: i64| Rect::from_x_y_ranges(scale.xf(a)..=scale.xf(b), rect.y_range());
    match &pv.target {
        Target::Insert(e) => {
            let x = scale.xf(*e);
            painter.line_segment(
                [pos2(x, rect.top() - 4.0), pos2(x, rect.bottom() + 4.0)],
                Stroke::new(3.0, Color32::from_rgb(170, 90, 230)),
            );
        }
        Target::Replace(id) => {
            if let Some(c) = p.clip(id) {
                painter.rect_stroke(
                    span(c.start, c.end()),
                    CornerRadius::same(2),
                    Stroke::new(2.5, Color32::WHITE),
                    StrokeKind::Inside,
                );
            }
        }
        Target::Overwrite(a, b) => {
            painter.rect_filled(
                span(*a, *b),
                CornerRadius::ZERO,
                Color32::from_white_alpha(40),
            );
            painter.rect_stroke(
                span(*a, *b),
                CornerRadius::ZERO,
                Stroke::new(1.5, Color32::from_rgb(230, 120, 60)),
                StrokeKind::Inside,
            );
        }
        Target::Append | Target::None => {}
    }
    for &(a, b) in &pv.cues {
        painter.rect_filled(
            span(a, b).shrink2(vec2(0.5, rect.height() * 0.2)),
            CornerRadius::same(2),
            theme::colors().accent.gamma_multiply(0.7),
        );
    }
    if let Some((a, b)) = pv.ghost {
        painter.rect_stroke(
            span(a, b).shrink(1.0),
            CornerRadius::same(2),
            Stroke::new(1.0, Color32::from_white_alpha(200)),
            StrokeKind::Inside,
        );
    }
    if let Some(s) = pv.snapped {
        let x = scale.xf(s);
        painter.line_segment(
            [pos2(x, rect.top() - 18.0), pos2(x, rect.bottom())],
            Stroke::new(1.0, Color32::WHITE),
        );
    }
    if let Some(m) = pv.marquee {
        painter.rect_filled(m, CornerRadius::ZERO, Color32::from_white_alpha(20));
        painter.rect_stroke(
            m,
            CornerRadius::ZERO,
            Stroke::new(1.0, Color32::from_white_alpha(120)),
            StrokeKind::Inside,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimPointer {
    End,
    Start,
    Roll,
}

pub fn trim_pointer(painter: &Painter, at: Pos2, kind: TrimPointer) {
    let (h, serif, reach, head) = (9.0, 4.0, 11.0, 4.0);
    let bracket = |x: f32, side: f32| {
        vec![
            pos2(x + side * serif, at.y - h),
            pos2(x, at.y - h),
            pos2(x, at.y + h),
            pos2(x + side * serif, at.y + h),
        ]
    };
    let arrow = |x: f32, side: f32| {
        let tip = x + side * reach;
        vec![
            vec![pos2(x, at.y), pos2(tip, at.y)],
            vec![
                pos2(tip - side * head, at.y - head),
                pos2(tip, at.y),
                pos2(tip - side * head, at.y + head),
            ],
        ]
    };
    let mut lines: Vec<Vec<Pos2>> = Vec::new();
    match kind {
        TrimPointer::End => {
            lines.push(bracket(at.x + 1.0, -1.0));
            lines.extend(arrow(at.x - 2.0, -1.0));
        }
        TrimPointer::Start => {
            lines.push(bracket(at.x - 1.0, 1.0));
            lines.extend(arrow(at.x + 2.0, 1.0));
        }
        TrimPointer::Roll => {
            lines.push(bracket(at.x - 2.0, -1.0));
            lines.push(bracket(at.x + 2.0, 1.0));
            lines.extend(arrow(at.x - 5.0, -1.0));
            lines.extend(arrow(at.x + 5.0, 1.0));
        }
    }
    for (width, color) in [(4.0, Color32::BLACK), (2.0, Color32::WHITE)] {
        for l in &lines {
            painter.add(egui::Shape::line(l.clone(), Stroke::new(width, color)));
        }
    }
}

pub(crate) fn marks(
    painter: &Painter,
    rows: &Rows,
    scale: &Scale,
    p: &Project,
    m: &crate::gui::history::Marks,
) {
    let accent = theme::colors().accent;
    let fill = accent.gamma_multiply(0.22 * m.strength);
    let edge = Stroke::new(1.5, accent.gamma_multiply(m.strength));
    for row in &rows.rows {
        let r = row.rect.rect;
        for s in &m.spans {
            if s.track.as_deref().is_some_and(|t| t != row.track) {
                continue;
            }
            let x = scale.xf(s.from)..=scale.xf(s.to).max(scale.xf(s.from) + 2.0);
            let band = Rect::from_x_y_ranges(x, r.y_range());
            painter.rect_filled(band, CornerRadius::ZERO, fill);
        }
        let Some(t) = p.track(&row.track) else {
            continue;
        };
        for c in t.clips.iter().filter(|c| m.clips.contains(&c.id)) {
            let clip = Rect::from_x_y_ranges(scale.xf(c.start)..=scale.xf(c.end()), r.y_range());
            painter.rect_stroke(
                clip.shrink(1.0),
                3.0,
                Stroke::new(2.0, accent.gamma_multiply(m.strength)),
                StrokeKind::Inside,
            );
        }
        for s in m
            .spans
            .iter()
            .filter(|s| s.track.as_deref() == Some(row.track.as_str()))
        {
            let x = scale.xf(s.from)..=scale.xf(s.to).max(scale.xf(s.from) + 2.0);
            painter.rect_stroke(
                Rect::from_x_y_ranges(x, r.y_range()),
                0.0,
                edge,
                StrokeKind::Inside,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_selected_edits_glow_is_strongest_at_the_start_of_each_pulse() {
        assert_eq!((super::pulse(super::PULSE) * 100.0).round(), 72.0);
    }
}
