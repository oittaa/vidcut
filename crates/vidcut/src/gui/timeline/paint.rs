//! Drawing the timelines (spec §5): rulers, clips with filmstrips and
//! waveforms, marks, markers, the playhead and gesture previews.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, pos2,
    vec2,
};
use vidcut_av::waveform::Waveform;
use vidcut_core::model::{Clip, Project, Track};
use vidcut_core::time::{Rational, format_timecode};

use super::geometry::{Rows, Scale, TrackRect, clips_in};
use super::gesture::{Preview, Target};
use crate::gui::pool::elide;
use crate::gui::theme;
use crate::gui::workspace::Thumbs;

/// What clips show (Timeline Options, p. 645).
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub names: bool,
    pub durations: bool,
    pub status: bool,
    pub filmstrips: bool,
    pub waveforms: bool,
}

/// Media pictures and sounds to draw clips with.
pub struct Assets<'a> {
    pub thumbs: &'a HashMap<String, Thumbs>,
    pub waveforms: &'a HashMap<String, Arc<Waveform>>,
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
                format_timecode(frame, fps, p.settings.start_tc),
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
        // The Smart Indicator (p. 696).
        let x = scale.xf(e);
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x, rect.bottom()),
                pos2(x - 5.0, rect.bottom() - 7.0),
                pos2(x + 5.0, rect.bottom() - 7.0),
            ],
            theme::OK,
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
    painter.rect_filled(rect, CornerRadius::ZERO, theme::TRACK);
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
    if t.locked {
        // Stippled: clips can't be changed (p. 717).
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
    let fill = if enabled {
        theme::CLIP
    } else {
        theme::CLIP_DISABLED
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
    if look.filmstrips && enabled && has_picture && video.height() >= 24.0 && width > 24.0 {
        filmstrip(
            &painter,
            video.shrink2(vec2(1.0, 1.0)),
            scale,
            p,
            c,
            assets,
            fps,
        );
    }
    // Audio, offset by J/L cuts.
    if let Some((ra, rb)) = c.audio_range() {
        let audio =
            Rect::from_x_y_ranges(scale.xf(ra)..=scale.xf(rb), tr.audio_top..=rect.bottom());
        let muted = c.audio.as_ref().is_some_and(|x| x.muted) || t.muted;
        let color = if muted {
            theme::CLIP_DISABLED
        } else {
            theme::AUDIO
        };
        let ap = track_painter.with_clip_rect(rect.intersect(audio.expand(1.0)));
        ap.rect_filled(audio.shrink2(vec2(0.5, 0.0)), CornerRadius::same(2), color);
        if look.waveforms && !muted && audio.height() >= 8.0 {
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
    }
    if look.names && width > 30.0 {
        let name = c
            .name
            .clone()
            .or_else(|| p.media(&c.media).map(|m| m.name.clone()))
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
        // Seconds:frames, bottom left (p. 689).
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
    if look.status && !c.enabled && has_picture && width > 40.0 {
        painter.text(
            video.center(),
            Align2::CENTER_CENTER,
            "disabled",
            FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
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
    let Some(t) = assets.thumbs.get(&c.media) else {
        return;
    };
    let Some(first) = t.textures.first() else {
        return;
    };
    let size = first.size_vec2();
    let tile_w = (rect.height() * size.x / size.y.max(1.0)).max(8.0);
    let media_secs = p
        .media(&c.media)
        .and_then(|m| m.info.video.as_ref().map(|v| v.duration))
        .unwrap_or(1.0)
        .max(1e-3);
    let n = t.textures.len();
    let clip_rect = painter.clip_rect().intersect(rect);
    let mut x = rect.left();
    // Start at the first tile that is visible.
    if clip_rect.left() > x {
        x += ((clip_rect.left() - x) / tile_w).floor() * tile_w;
    }
    while x < rect.right().min(clip_rect.right()) {
        let frame = scale.frame(x + tile_w / 2.0) - c.start as f64 + c.source_in as f64;
        let secs = frame / fps.as_f64();
        let i = ((secs / media_secs) * n as f64)
            .floor()
            .clamp(0.0, (n - 1) as f64) as usize;
        let tile = Rect::from_min_size(pos2(x, rect.top()), vec2(tile_w, rect.height()));
        painter.image(
            t.textures[i].id(),
            tile,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::from_white_alpha(225),
        );
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
        let t0 = scale.frame(x) - c.start as f64 + c.source_in as f64;
        let t1 = scale.frame(x + 1.0) - c.start as f64 + c.source_in as f64;
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
    if rows.get(track).is_none() && (pv.ghost.is_some() || pv.request.is_some()) {
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
            // The edit point turns purple (p. 723).
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
