//! Drawing the timelines (spec §5): rulers, clips with filmstrips and
//! waveforms, marks, markers, the playhead and gesture previews.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, pos2,
    vec2,
};
use vidcut_av::waveform::Waveform;
use vidcut_core::model::{Clip, Effects, Project, Speed, Track, Transition};
use vidcut_core::ops;
use vidcut_core::otio::transitions;
use vidcut_core::time::Rational;

use super::geometry::{Rows, Scale, TrackRect, clips_in};
use super::gesture::{Preview, Target};
use crate::gui::pool::elide;
use crate::gui::theme;
use crate::gui::tiles::{self, TileKey, Tiles};
use crate::gui::workspace::Thumbs;

/// What clips show (Timeline Options).
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub names: bool,
    pub durations: bool,
    pub status: bool,
    pub filmstrips: bool,
    pub waveforms: bool,
    /// The edit functions' target track: its row is lighter.
    pub target: bool,
}

/// Media pictures and sounds to draw clips with.
pub struct Assets<'a> {
    pub thumbs: &'a HashMap<String, Thumbs>,
    pub waveforms: &'a HashMap<String, Arc<Waveform>>,
    /// Each tile's own frame (`None` in windows without them).
    pub tiles: Option<&'a Tiles>,
    /// Media whose files are missing: their clips are red.
    pub missing: &'a HashSet<String>,
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

/// A "nice" ruler step in seconds with labels at least `min_px` apart.
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

/// The ruler with timecodes, the In/Out range, markers and the Smart
/// Indicator.
#[allow(clippy::too_many_arguments)]
pub fn ruler(
    painter: &Painter,
    rect: Rect,
    scale: &Scale,
    p: &Project,
    smart: Option<i64>,
    compact: bool,
) {
    painter.rect_filled(rect, CornerRadius::ZERO, theme::PANEL_DARK);
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
                Stroke::new(1.0, theme::TEXT_DIM),
            );
            painter.text(
                pos2(x + 3.0, rect.top() + 1.0),
                Align2::LEFT_TOP,
                p.settings.timecode(frame),
                FontId::monospace(if compact { 9.0 } else { 10.0 }),
                theme::TEXT_DIM,
            );
        }
        t += s;
    }
    // In/Out.
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
                let w = if left { 6.0 } else { -6.0 };
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        pos2(x, rect.top() + 2.0),
                        pos2(x + w, rect.top() + 2.0),
                        pos2(x, rect.bottom()),
                    ],
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
        // The Smart Indicator: red where a clip has no media past
        // the edit. It holds still (deviation X15).
        let x = scale.xf(e);
        let color = if ops::edit_has_handles(p, e) {
            theme::OK
        } else {
            theme::ERROR
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

/// A track's clips.
#[allow(clippy::too_many_arguments)]
pub fn track(
    painter: &Painter,
    tr: &TrackRect,
    scale: &Scale,
    p: &Project,
    t: &Track,
    selected: &[String],
    assets: &Assets,
    look: Look,
) {
    let rect = tr.rect;
    let fill = if look.target {
        theme::TRACK_TARGET
    } else {
        theme::TRACK
    };
    painter.rect_filled(rect, CornerRadius::ZERO, fill);
    let (a, b) = scale.visible(rect.left(), rect.right());
    let fps = p.settings.fps;
    for c in &t.clips[clips_in(t, a - 1, b + 1)] {
        clip(
            painter,
            tr,
            scale,
            p,
            t,
            c,
            selected.contains(&c.id),
            assets,
            look,
            fps,
        );
    }
    let cuts = transitions::Cuts::of(p, t);
    for x in &t.transitions {
        if let Some(at) = cuts.place(p, x)
            && at + x.after >= a
            && at - x.before <= b
        {
            transition(painter, tr, scale, at, x);
        }
    }
    if t.locked {
        // Stippled: clips can't be changed.
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
    let (x0, x1) = (
        scale.xf(c.start),
        scale.xf(c.end()).max(scale.xf(c.start) + 1.0),
    );
    let video = Rect::from_x_y_ranges(x0..=x1, rect.top()..=tr.audio_top - 1.0);
    // The audio band may reach into a neighbor (J/L cuts), so it is clipped
    // to its own span rather than the clip's.
    let track_painter = painter;
    let painter = painter.with_clip_rect(rect.intersect(Rect::from_x_y_ranges(
        (x0 - 1.0)..=(x1 + 1.0),
        rect.y_range(),
    )));
    let enabled = c.enabled && t.video_enabled;
    // Offline clips are red, as in the Media Pool, with nothing drawn of
    // their file.
    let offline = assets.missing.contains(&c.media);
    let tint = p.clip_color(c).and_then(theme::clip_color);
    let fill = if offline {
        theme::OFFLINE
    } else if !enabled {
        theme::CLIP_DISABLED
    } else if let Some(t) = tint {
        t
    } else if c.is_generator() {
        theme::GENERATOR
    } else if c.compound.is_some() {
        theme::COMPOUND
    } else {
        theme::CLIP
    };
    // Audio-only clips (and audio tracks) have no picture band.
    let has_picture = c.video && video.height() > 2.0;
    if has_picture {
        painter.rect_filled(video.shrink2(vec2(0.5, 0.0)), CornerRadius::same(2), fill);
    } else if video.height() > 2.0 {
        painter.rect_stroke(
            video.shrink(1.0),
            CornerRadius::same(2),
            Stroke::new(1.0, theme::CLIP_DISABLED),
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
        // Its Clip Color over the pictures.
        if let Some(t) = tint {
            painter.rect_filled(
                video.shrink2(vec2(0.5, 0.0)),
                CornerRadius::same(2),
                t.gamma_multiply(0.35),
            );
        }
    }
    // A compound clip's sound: a block on the audio band.
    if c.compound.is_some() && !c.video {
        let band = Rect::from_x_y_ranges(x0..=x1, tr.audio_top..=rect.bottom());
        painter.rect_filled(band.shrink2(vec2(0.5, 0.0)), CornerRadius::same(2), fill);
    }
    // Audio, offset by J/L cuts.
    if let Some((ra, rb)) = c.audio_range() {
        let audio =
            Rect::from_x_y_ranges(scale.xf(ra)..=scale.xf(rb), tr.audio_top..=rect.bottom());
        let muted = c.audio.as_ref().is_some_and(|x| x.muted) || t.muted;
        let color = if offline {
            theme::OFFLINE
        } else if muted {
            theme::CLIP_DISABLED
        } else if let Some(t) = tint {
            let dim = |v: u8| (f32::from(v) * 0.8) as u8;
            Color32::from_rgb(dim(t.r()), dim(t.g()), dim(t.b()))
        } else {
            theme::AUDIO
        };
        let ap = track_painter.with_clip_rect(rect.intersect(audio.expand(1.0)));
        ap.rect_filled(audio.shrink2(vec2(0.5, 0.0)), CornerRadius::same(2), color);
        if look.waveforms && !muted && !offline && audio.height() >= 8.0 {
            waveform(&ap, audio, scale, p, c, assets, fps);
        }
        if muted && look.status {
            ap.text(
                audio.left_center() + vec2(4.0, 0.0),
                Align2::LEFT_CENTER,
                "muted",
                FontId::proportional(9.0),
                theme::TEXT_DIM,
            );
        }
        if look.status
            && let Some(e) = c.audio.as_ref().and_then(|a| a.effects.as_ref())
        {
            effects_badge(&ap, audio, e);
        }
        if look.status
            && !c.video
            && let Some(speed) = &c.speed
        {
            speed_badge(&ap, audio, speed);
        }
    }
    if look.names && width > 30.0 {
        let name = c
            .name
            .clone()
            .or_else(|| p.media(&c.media).map(|m| m.name.clone()))
            .or_else(|| c.generator.as_ref().map(vidcut_core::otio::generator_kind))
            .or_else(|| c.compound.as_ref().map(vidcut_core::otio::compound_kind))
            .unwrap_or_default();
        // Pinned to the visible part of the clip.
        let left = x0.max(painter.clip_rect().left());
        let room = x1 - left;
        let label = elide(&name, ((room - 8.0) / 6.5).max(3.0) as usize);
        // On audio rows the name sits on the audio band.
        let pos = pos2(left + 4.0, video.top().min(tr.audio_top) + 2.0);
        let galley = painter.layout_no_wrap(label, FontId::proportional(11.0), Color32::WHITE);
        painter.rect_filled(
            Rect::from_min_size(pos - vec2(2.0, 0.0), galley.size() + vec2(4.0, 1.0)),
            CornerRadius::same(2),
            Color32::from_black_alpha(110),
        );
        painter.galley(pos, galley, Color32::WHITE);
    }
    if look.durations && width > 70.0 && video.height() > 30.0 {
        // Seconds:frames, bottom left.
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
        && let Some(speed) = &c.speed
    {
        speed_badge(&painter, video, speed);
    }
    if look.status && width > 40.0 {
        let status = if offline {
            Some(("offline", Color32::WHITE))
        } else {
            (!c.enabled).then_some(("disabled", theme::TEXT_DIM))
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
            Stroke::new(2.0, theme::ACCENT),
            StrokeKind::Inside,
        );
    }
}

/// A transition carried from another editor on the cut at `at` (deviation
/// X9), played as a cut: a box over the frames it covers, across the
/// picture or sound band, with a line rising through it.
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

/// A badge at the bottom right of a clip's picture or sound band naming
/// the effects it carries from another editor, kept but not shown
/// (deviation X9): `fx Transform, Cropping`, or `fx` where that won't fit.
fn effects_badge(painter: &Painter, band: Rect, effects: &Effects) {
    let right = band.right().min(painter.clip_rect().right());
    // Clear of the duration at the bottom left.
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

/// A badge at the top right of a clip's picture or sound band with the
/// speed change it carries from another editor (deviation X9), which it
/// plays at (decision D48): `200%`.
fn speed_badge(painter: &Painter, band: Rect, speed: &Speed) {
    let right = band.right().min(painter.clip_rect().right());
    if right - band.left().max(painter.clip_rect().left()) < 40.0 || band.height() < 13.0 {
        return;
    }
    tag(painter, speed.label(), |size| {
        pos2(right - size.x - 4.0, band.top() + 2.0)
    });
}

/// `text` small on a dark tag, where `at` puts it for its size.
fn tag(painter: &Painter, text: String, at: impl FnOnce(egui::Vec2) -> Pos2) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(9.0), theme::ACCENT);
    let pos = at(galley.size());
    painter.rect_filled(
        Rect::from_min_size(pos - vec2(2.0, 0.0), galley.size() + vec2(4.0, 1.0)),
        CornerRadius::same(2),
        Color32::from_black_alpha(140),
    );
    painter.galley(pos, galley, theme::ACCENT);
}

/// Pool thumbnails tiled along a clip, each from the source time at its place.
fn filmstrip(
    painter: &Painter,
    rect: Rect,
    scale: &Scale,
    p: &Project,
    c: &Clip,
    assets: &Assets,
    fps: Rational,
) {
    let Some(m) = p.media(&c.media) else {
        return;
    };
    let pool = assets.thumbs.get(&c.media);
    // Tiles keep the picture's shape: the pool's pictures have it, else
    // the media's display size.
    let aspect = pool
        .and_then(|t| t.textures.first())
        .map(|t| t.size_vec2().x / t.size_vec2().y.max(1.0))
        .or_else(|| {
            let v = m.info.video.as_ref()?;
            let sar = if v.sar[0] > 0 && v.sar[1] > 0 {
                v.sar[0] as f32 / v.sar[1] as f32
            } else {
                1.0
            };
            let (w, h) = (v.width as f32 * sar, v.height.max(1) as f32);
            Some(if v.rotation % 180 == 90 { h / w } else { w / h })
        })
        .unwrap_or(16.0 / 9.0)
        .clamp(0.1, 10.0);
    let tile_w = (rect.height() * aspect).max(8.0);
    let height = tiles::tile_height(rect.height() * painter.ctx().pixels_per_point());
    let width = (height as f32 * aspect).round().max(1.0) as u32;
    let step = tiles::tile_step(f64::from(tile_w) / scale.ppf);
    let speed = c.speed();
    let last = c.source_at(c.start + c.duration - 1);
    let (lo, hi) = (c.source_in.min(last), c.source_in.max(last));
    let clip_rect = painter.clip_rect().intersect(rect);
    let mut x = rect.left();
    // Start at the first tile that is visible.
    if clip_rect.left() > x {
        x += ((clip_rect.left() - x) / tile_w).floor() * tile_w;
    }
    while x < rect.right().min(clip_rect.right()) {
        let offset = f64::from(x - rect.left()) / scale.ppf;
        let first = x <= rect.left();
        let frame = tiles::tile_frame(c.source_in, offset, step, first, speed).clamp(lo, hi);
        let key = TileKey {
            media: c.media.clone(),
            frame,
            height,
        };
        // Its own frame, else the nearest kept one within a tile or two,
        // else the pool's nearest.
        let texture = assets
            .tiles
            .and_then(|t| {
                t.want(&key, || t.job(p, &key, width))
                    .or_else(|| t.nearest(&c.media, height, frame, 2 * step.max(1)))
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

/// The clip's audio peaks, one line per pixel column.
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
    // At its speed (decision D48): a held frame is silent.
    if c.speed() == 0.0 {
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
        if let Some((lo, hi)) = w.range(origin + t0 / f, origin + t1 / f) {
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

/// The playhead line (and its handle on the ruler).
pub fn playhead(painter: &Painter, x: f32, ruler: Rect, bottom: f32, color: Color32) {
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

/// What a gesture would do: drop targets, the moved block, snap lines. A
/// target on a track that doesn't exist yet shows where it will be made.
pub fn preview(painter: &Painter, rows: &Rows, scale: &Scale, p: &Project, pv: &Preview) {
    let track = pv.row.as_deref().unwrap_or(vidcut_core::ops::MAIN);
    let tr = rows.rect_of(track);
    let rect = tr.rect;
    let new_track = rows.get(track).is_none();
    // A new audio track shows below the timeline, where it is dropped.
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
            theme::TEXT_DIM,
        );
    }
    let span = |a: i64, b: i64| Rect::from_x_y_ranges(scale.xf(a)..=scale.xf(b), rect.y_range());
    match &pv.target {
        Target::Insert(e) => {
            // The edit point turns purple.
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

/// What a drag at the pointer would trim, for the timeline's own pointer:
/// the end of the clip on the left, the start of the clip on the right, or
/// the cut between them (a roll).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimPointer {
    End,
    Start,
    Roll,
}

/// Draws the trim pointer at `at`: a bracket for each clip end that moves
/// (`]` an end, `[` a start), with an arrow pointing into that clip, so the
/// side that changes reads at a glance. Outlined, so it shows on any
/// picture.
pub fn trim_pointer(painter: &Painter, at: Pos2, kind: TrimPointer) {
    let (h, serif, reach, head) = (9.0, 4.0, 11.0, 4.0);
    // A bracket at `x`; its serifs point toward `side` (-1 left, 1 right).
    let bracket = |x: f32, side: f32| {
        vec![
            pos2(x + side * serif, at.y - h),
            pos2(x, at.y - h),
            pos2(x, at.y + h),
            pos2(x + side * serif, at.y + h),
        ]
    };
    // An arrow from `x` outward by `reach`, toward `side`.
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
