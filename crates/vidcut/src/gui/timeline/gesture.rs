//! Timeline gestures (spec §7.3, §8). A press starts one; while the pointer
//! moves, [`Drag::update`] says what the release would do (drawn as a
//! preview); the release turns it into exactly one request, or none.
//! Scrubbing only moves the playhead.
//!
//! Pure: driven by pointer positions, times and the project, so gestures are
//! tested without a window.

use std::time::{Duration, Instant};

use eframe::egui::{Pos2, Rect};
use serde_json::{Value, json};
use vidcut_core::model::{Clip, Project, Track, TrackKind};
use vidcut_core::ops::{Edge, MAIN};

use super::geometry::{Hit, Rows, SNAP_PX, Scale, Snap, clips_in, edit_near, hit_rows};

/// Hovering over a clip this long turns a drop from Replace into Overwrite
/// (decision D11).
pub const HOVER: Duration = Duration::from_millis(800);

/// Pushing against a neighbor this long overrides Safe Edit.
pub const SAFE_EDIT_HOLD: Duration = Duration::from_millis(600);

/// Pixels around an edit point that make a drop an insert.
const DROP_EDIT_PX: f32 = 6.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    /// Moving the playhead. With `drag_content` (the lower timeline with a
    /// fixed playhead) dragging moves the clips under the playhead instead
    ///.
    Scrub { drag_content: bool, from: f64 },
    /// Dragging clips.
    Move {
        clips: Vec<String>,
        /// The track of the clip grabbed.
        from: String,
        /// Frame under the pointer at the press.
        grab: f64,
        /// The block's first frame and length.
        start: i64,
        len: i64,
    },
    /// Resizing a clip edge. `ripple` is false with Alt held.
    Trim {
        clip: String,
        edge: Edge,
        from: i64,
        ripple: bool,
    },
    /// Rolling an edit.
    Roll { outgoing: String, from: i64 },
    /// Slipping clips' content.
    Slip { clips: Vec<String> },
    /// Moving only the audio edit: a J/L cut.
    AudioRoll { outgoing: String, from: i64 },
    /// Selecting clips with a rectangle.
    Marquee { additive: bool },
    /// Dragging a marker on the ruler.
    Marker { id: String, from: i64 },
    /// Dragging the timeline In or Out on the ruler.
    Mark { is_in: bool, from: i64 },
    /// Dragging media from the pool (spec §7.3).
    Media {
        media: String,
        range: (i64, i64),
        video_only: bool,
        audio_only: bool,
    },
    /// Dragging a bin onto another bin in the pool (spec §3). The release
    /// is decided in the pool, not as a timeline edit.
    Bin { path: String },
}

/// Where a drop lands.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Target {
    #[default]
    None,
    /// Insert at an edit point (it highlights).
    Insert(i64),
    /// Replace a clip (the whole clip highlights).
    Replace(String),
    /// Write over `[a, b)` (the footprint highlights).
    Overwrite(i64, i64),
    /// After the last clip.
    Append,
}

/// What releasing now would do, for drawing while dragging.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Preview {
    /// `{"method", "params"}`, or `None` if the release changes nothing.
    pub request: Option<Value>,
    pub target: Target,
    /// Frames the moved or incoming block would cover.
    pub ghost: Option<(i64, i64)>,
    /// A frame being snapped to (the white line).
    pub snapped: Option<i64>,
    /// Frames moved and the new duration, for the tooltip.
    pub delta: Option<(i64, Option<i64>)>,
    /// Where the playhead should be (scrubbing).
    pub seek: Option<i64>,
    pub marquee: Option<Rect>,
    /// Safe Edit stopped the trim at a neighbor.
    pub blocked: bool,
    /// The track whose row shows the target and ghost (maybe a new one).
    pub row: Option<String>,
}

/// What a gesture needs to know about the timeline.
pub struct Ctx<'a> {
    pub project: &'a Project,
    pub scale: Scale,
    /// The timeline's track rows.
    pub rows: &'a Rows,
    /// Snap targets, or `None` with snapping off.
    pub snap: Option<&'a Snap>,
    pub safe_edit: bool,
    /// Clips selected before the gesture (marquee with Ctrl adds to them).
    pub selected: &'a [String],
}

impl Ctx<'_> {
    fn v1(&self) -> Option<&Track> {
        self.project.track(MAIN)
    }

    fn clip(&self, id: &str) -> Option<&Clip> {
        self.project.clip(id)
    }

    /// The track a clip is on.
    fn track_of(&self, id: &str) -> Option<&Track> {
        self.project
            .find_clip(id)
            .map(|(ti, _)| &self.project.tracks[ti])
    }

    fn media_frames(&self, c: &Clip) -> i64 {
        self.project
            .media(&c.media)
            .map_or(i64::MAX / 4, |m| self.project.media_frames(m))
    }

    fn snapped(&self, frame: i64) -> (i64, Option<i64>) {
        match self.snap.and_then(|s| s.snap(frame, &self.scale, SNAP_PX)) {
            Some(t) => (t, Some(t)),
            None => (frame, None),
        }
    }
}

fn request(method: &str, params: Value) -> Option<Value> {
    Some(json!({ "method": method, "params": params }))
}

/// A gesture in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub gesture: Gesture,
    pub press: Pos2,
    /// The clip under the pointer and since when (drop targets, D11).
    hover: Option<(String, Instant)>,
    /// When Safe Edit first stopped the trim.
    blocked_since: Option<Instant>,
}

impl Drag {
    pub fn new(gesture: Gesture, press: Pos2) -> Self {
        Self {
            gesture,
            press,
            hover: None,
            blocked_since: None,
        }
    }

    /// Tracks the pointer and says what releasing at `pos` would do.
    pub fn update(&mut self, pos: Pos2, now: Instant, ctx: &Ctx) -> Preview {
        // Hover bookkeeping for drop targets.
        let under = match hit_rows(ctx.project, ctx.rows, &ctx.scale, pos).1 {
            Hit::Clip(id) | Hit::Slip(id) | Hit::Edge(id, _) => Some(id),
            _ => None,
        };
        match (&self.hover, &under) {
            (Some((h, _)), Some(u)) if h == u => {}
            (_, Some(u)) => self.hover = Some((u.clone(), now)),
            (_, None) => self.hover = None,
        }
        let hovered = |id: &str| {
            self.hover
                .as_ref()
                .filter(|(h, _)| h == id)
                .map_or(Duration::ZERO, |(_, since)| now.duration_since(*since))
        };
        match &self.gesture {
            Gesture::Scrub { drag_content, from } => {
                let end = ctx.project.end();
                let frame = if *drag_content {
                    // Content follows the pointer: dragging left moves on.
                    (from + f64::from(self.press.x - pos.x) / ctx.scale.ppf).round() as i64
                } else {
                    ctx.scale.under(pos.x)
                };
                let (frame, snapped) = ctx.snapped(frame.clamp(0, end));
                Preview {
                    seek: Some(frame.clamp(0, end)),
                    snapped,
                    ..Preview::default()
                }
            }
            Gesture::Move {
                clips,
                from,
                grab,
                start,
                len,
            } => move_preview(ctx, pos, clips, from, *grab, *start, *len, &under, &hovered),
            Gesture::Media {
                media,
                range,
                video_only,
                audio_only,
            } => media_preview(
                ctx,
                pos,
                media,
                *range,
                (*video_only, *audio_only),
                &under,
                &hovered,
            ),
            // The pool finishes this drop (`bins.rename`) before update.
            Gesture::Bin { .. } => Preview::default(),
            Gesture::Trim {
                clip,
                edge,
                from,
                ripple,
            } => {
                let (clip, edge, from, ripple) = (clip.clone(), *edge, *from, *ripple);
                self.trim_preview(ctx, pos, now, &clip, edge, from, ripple)
            }
            Gesture::Roll { outgoing, from } => {
                roll_preview(ctx, *from + moved(ctx, self.press, pos), outgoing, *from)
            }
            Gesture::Slip { clips } => slip_preview(ctx, pos, self.press, clips),
            Gesture::AudioRoll { outgoing, from } => {
                audio_roll_preview(ctx, *from + moved(ctx, self.press, pos), outgoing, *from)
            }
            Gesture::Marquee { additive } => marquee_preview(ctx, self.press, pos, *additive),
            Gesture::Marker { id, from } => {
                let (to, snapped) = ctx.snapped(ctx.scale.boundary(pos.x).max(0));
                Preview {
                    request: (to != *from)
                        .then(|| request("markers.move", json!({ "marker": id, "to": to })))
                        .flatten(),
                    snapped,
                    delta: Some((to - from, None)),
                    ..Preview::default()
                }
            }
            Gesture::Mark { is_in, from } => {
                let (f, snapped) = ctx.snapped(ctx.scale.boundary(pos.x).max(0));
                let marks = &ctx.project.marks;
                // Keep In before Out.
                let f = if *is_in {
                    marks.mark_out.map_or(f, |o| f.min(o - 1))
                } else {
                    marks.mark_in.map_or(f.max(1), |i| f.max(i + 1))
                };
                let key = if *is_in { "in" } else { "out" };
                Preview {
                    request: (f != *from)
                        .then(|| request("marks.set", json!({ key: f })))
                        .flatten(),
                    snapped,
                    ..Preview::default()
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn trim_preview(
        &mut self,
        ctx: &Ctx,
        pos: Pos2,
        now: Instant,
        id: &str,
        edge: Edge,
        from: i64,
        ripple: bool,
    ) -> Preview {
        let (Some(c), Some(t)) = (ctx.clip(id), ctx.track_of(id)) else {
            return Preview::default();
        };
        let row = Some(t.id.clone());
        let (target, snapped) = ctx.snapped(from + moved(ctx, self.press, pos));
        let mut delta = target - from;
        // Media limits.
        let frames = ctx.media_frames(c);
        let (lo, hi) = match edge {
            Edge::Head => (-c.source_in, c.duration - 1),
            Edge::Tail => (1 - c.duration, frames - c.source_in - c.duration),
        };
        delta = delta.clamp(lo, hi);
        if edge == Edge::Head && !ripple {
            delta = delta.max(-c.start);
        }
        // Safe Edit: without ripple, stop at the neighbor until the user has
        // kept pushing for a moment.
        let mut blocked = false;
        if !ripple && ctx.safe_edit {
            let i = t.clips.iter().position(|x| x.id == c.id).unwrap_or(0);
            let limit = match edge {
                Edge::Tail => t
                    .clips
                    .get(i + 1)
                    .map(|n| n.start - c.end())
                    .filter(|&room| delta > room),
                Edge::Head => i
                    .checked_sub(1)
                    .map(|p| t.clips[p].end() - c.start)
                    .filter(|&room| delta < room),
            };
            match limit {
                Some(room) => {
                    let since = *self.blocked_since.get_or_insert(now);
                    if now.duration_since(since) < SAFE_EDIT_HOLD {
                        delta = room;
                        blocked = true;
                    }
                }
                None => self.blocked_since = None,
            }
        }
        let duration = match edge {
            Edge::Head => c.duration - delta,
            Edge::Tail => c.duration + delta,
        };
        let ghost = match (edge, ripple) {
            (Edge::Head, true) => (c.start, c.start + duration),
            (Edge::Head, false) => (c.start + delta, c.end()),
            (Edge::Tail, _) => (c.start, c.start + duration),
        };
        let mut params = json!({
            "clip": id,
            "edge": if edge == Edge::Head { "head" } else { "tail" },
            "by": delta,
        });
        if !ripple && ctx.project.settings.ripple_on {
            params["ripple"] = json!(false);
        }
        Preview {
            request: (delta != 0).then(|| request("edit.trim", params)).flatten(),
            ghost: Some(ghost),
            snapped,
            delta: Some((delta, Some(duration))),
            blocked,
            row,
            ..Preview::default()
        }
    }

    /// The request releasing at `pos` makes.
    pub fn finish(&mut self, pos: Pos2, now: Instant, ctx: &Ctx) -> Option<Value> {
        self.update(pos, now, ctx).request
    }
}

#[allow(clippy::too_many_arguments)]
fn move_preview(
    ctx: &Ctx,
    pos: Pos2,
    clips: &[String],
    from: &str,
    grab: f64,
    start: i64,
    len: i64,
    under: &Option<String>,
    hovered: &dyn Fn(&str) -> Duration,
) -> Preview {
    // Dragged away from the timeline: no change (decision D18).
    let Some(dest) = ctx
        .rows
        .target(ctx.project, pos.y)
        .filter(|_| ctx.rows.reaches(pos))
    else {
        return Preview::default();
    };
    // Clips from several tracks only move sideways, each on its own track.
    let one_track = clips
        .iter()
        .all(|c| ctx.track_of(c).is_some_and(|t| t.id == from));
    let dest = if one_track { dest } else { from.to_string() };
    // Only audio goes on audio tracks.
    if dest.starts_with('A') && clips.iter().any(|c| ctx.clip(c).is_some_and(|c| c.video)) {
        return Preview::default();
    }
    let delta = (ctx.scale.frame(pos.x) - grab).round() as i64;
    let raw = (start + delta).max(0);
    let (ghost, snapped) = match ctx.snap {
        Some(s) => s.snap_block(raw, len, &ctx.scale, SNAP_PX),
        None => (raw, None),
    };
    let ghost = ghost.max(0);
    let row = Some(dest.clone());
    let elsewhere = (dest != from).then(|| dest.clone());
    let with_track = |mut params: Value| {
        if let Some(t) = &elsewhere {
            params["track"] = json!(t);
        }
        params
    };
    let Some(t) = ctx.v1() else {
        return Preview::default();
    };
    if dest == MAIN {
        let own = |f: i64| from == MAIN && (f == start || f == start + len);
        let end = t.end();
        // Dropping on an edit point inserts there.
        if let Some(e) = edit_near(t, &ctx.scale, pos.x, DROP_EDIT_PX).filter(|&e| !own(e)) {
            return Preview {
                request: request(
                    "edit.move",
                    with_track(json!({ "clips": clips, "to": e, "mode": "insert" })),
                ),
                target: Target::Insert(e),
                ghost: Some((e, e + len)),
                row,
                ..Preview::default()
            };
        }
        // Over another Track 1 clip: replace it right away, overwrite after
        // hovering. Clips from other tracks only overwrite.
        let on_main = under
            .as_ref()
            .filter(|u| !clips.contains(u))
            .filter(|u| ctx.track_of(u).is_some_and(|t| t.id == MAIN));
        if let Some(id) = on_main {
            if from == MAIN && clips.len() == 1 && hovered(id) < HOVER {
                let target = ctx
                    .clip(id)
                    .map_or((ghost, ghost + len), |c| (c.start, c.end()));
                return Preview {
                    request: request("edit.replace", json!({ "clip": clips[0], "onto": id })),
                    target: Target::Replace(id.clone()),
                    ghost: Some((target.0, target.0 + len)),
                    row,
                    ..Preview::default()
                };
            }
            return overwrite_move(clips, ghost, len, snapped, &dest, &elsewhere);
        }
        if from == MAIN && ghost == start {
            return Preview {
                ghost: Some((start, start + len)),
                snapped,
                row,
                ..Preview::default()
            };
        }
        // Past the end: move to the end.
        if pos.x > ctx.scale.xf(end) + DROP_EDIT_PX && (from != MAIN || start + len < end) {
            return Preview {
                request: request(
                    "edit.move",
                    with_track(json!({ "clips": clips, "to": end, "mode": "insert" })),
                ),
                target: Target::Append,
                ghost: Some((end, end + len)),
                row,
                ..Preview::default()
            };
        }
        // Elsewhere (over itself, a gap, or a neighbor's edge without the
        // pointer reaching it): overwrite at the new position.
        return overwrite_move(clips, ghost, len, snapped, &dest, &elsewhere);
    }
    // Other tracks don't ripple: the clips go where they are dropped.
    if elsewhere.is_none() && ghost == start {
        return Preview {
            ghost: Some((start, start + len)),
            snapped,
            row,
            ..Preview::default()
        };
    }
    overwrite_move(clips, ghost, len, snapped, &dest, &elsewhere)
}

/// Moving `clips` to `ghost` on `dest`, overwriting; `track` names `dest`
/// when it isn't their own track.
fn overwrite_move(
    clips: &[String],
    ghost: i64,
    len: i64,
    snapped: Option<i64>,
    dest: &str,
    track: &Option<String>,
) -> Preview {
    let mut params = json!({ "clips": clips, "to": ghost, "mode": "overwrite" });
    if let Some(t) = track {
        params["track"] = json!(t);
    }
    Preview {
        request: request("edit.move", params),
        target: Target::Overwrite(ghost, ghost + len),
        ghost: Some((ghost, ghost + len)),
        snapped,
        row: Some(dest.to_string()),
        ..Preview::default()
    }
}

fn media_preview(
    ctx: &Ctx,
    pos: Pos2,
    media: &str,
    range: (i64, i64),
    (video_only, audio_only): (bool, bool),
    under: &Option<String>,
    hovered: &dyn Fn(&str) -> Duration,
) -> Preview {
    let Some(mut dest) = ctx
        .rows
        .target(ctx.project, pos.y)
        .filter(|_| ctx.rows.reaches(pos))
    else {
        return Preview::default();
    };
    let Some(t) = ctx.v1() else {
        return Preview::default();
    };
    let len = range.1 - range.0;
    let has_video = !audio_only
        && ctx
            .project
            .media(media)
            .is_some_and(|m| m.info.video.is_some());
    // Sound alone goes on an audio track.
    if !has_video && dest.starts_with('V') {
        let audio = ctx.project.tracks_of(TrackKind::Audio);
        dest = audio
            .first()
            .map_or("A1".to_string(), |&i| ctx.project.tracks[i].id.clone());
    }
    let mut source = json!({ "media": media, "range": format!("{}f..{}f", range.0, range.1) });
    if video_only {
        source["video_only"] = json!(true);
    }
    if audio_only {
        source["audio_only"] = json!(true);
    }
    let with = |mut v: Value, extra: Value| {
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    };
    let row = Some(dest.clone());
    if dest != MAIN {
        // Other tracks: written where it is dropped (made if new).
        let raw = ctx.scale.boundary(pos.x).max(0);
        let (at, snapped) = match ctx.snap {
            Some(s) => s.snap_block(raw, len, &ctx.scale, SNAP_PX),
            None => (raw, None),
        };
        let at = at.max(0);
        return Preview {
            request: request(
                "edit.overwrite",
                with(
                    source,
                    json!({ "at": at, "use_marks": false, "track": dest }),
                ),
            ),
            target: Target::Overwrite(at, at + len),
            ghost: Some((at, at + len)),
            snapped,
            row,
            ..Preview::default()
        };
    }
    let end = t.end();
    // An empty timeline, or past the last clip: append (spec §7.3).
    if t.clips.is_empty() || pos.x > ctx.scale.xf(end) + DROP_EDIT_PX {
        return Preview {
            request: request("edit.append", source),
            target: Target::Append,
            ghost: Some((end, end + len)),
            row,
            ..Preview::default()
        };
    }
    // Left of the first clip: it becomes the first clip.
    let first = t.clips[0].start;
    if pos.x < ctx.scale.xf(first) - DROP_EDIT_PX {
        return Preview {
            request: request("edit.insert", with(source, json!({ "at": first }))),
            target: Target::Insert(first),
            ghost: Some((first, first + len)),
            row,
            ..Preview::default()
        };
    }
    if let Some(e) = edit_near(t, &ctx.scale, pos.x, DROP_EDIT_PX) {
        return Preview {
            request: request("edit.insert", with(source, json!({ "at": e }))),
            target: Target::Insert(e),
            ghost: Some((e, e + len)),
            row,
            ..Preview::default()
        };
    }
    if let Some(id) = under
        && let Some(c) = ctx
            .clip(id)
            .filter(|_| ctx.track_of(id).is_some_and(|t| t.id == MAIN))
    {
        if hovered(id) < HOVER {
            return Preview {
                request: request(
                    "edit.ripple_overwrite",
                    with(source, json!({ "at": c.start, "use_marks": false })),
                ),
                target: Target::Replace(id.clone()),
                ghost: Some((c.start, c.start + len)),
                row,
                ..Preview::default()
            };
        }
        let raw = ctx.scale.boundary(pos.x).max(0);
        let (at, snapped) = match ctx.snap {
            Some(s) => s.snap_block(raw, len, &ctx.scale, SNAP_PX),
            None => (raw, None),
        };
        return Preview {
            request: request(
                "edit.overwrite",
                with(source, json!({ "at": at, "use_marks": false })),
            ),
            target: Target::Overwrite(at, at + len),
            ghost: Some((at, at + len)),
            snapped,
            row,
            ..Preview::default()
        };
    }
    // In a gap (Ripple Off timelines): write into it.
    let at = ctx.scale.boundary(pos.x).max(0);
    Preview {
        request: request(
            "edit.overwrite",
            with(source, json!({ "at": at, "use_marks": false })),
        ),
        target: Target::Overwrite(at, at + len),
        ghost: Some((at, at + len)),
        row,
        ..Preview::default()
    }
}

/// Frames the pointer has moved since the press (edges follow the pointer
/// rather than jumping to it).
fn moved(ctx: &Ctx, press: Pos2, pos: Pos2) -> i64 {
    (f64::from(pos.x - press.x) / ctx.scale.ppf).round() as i64
}

fn roll_preview(ctx: &Ctx, target: i64, outgoing: &str, from: i64) -> Preview {
    let (Some(t), Some(a)) = (ctx.track_of(outgoing), ctx.clip(outgoing)) else {
        return Preview::default();
    };
    let Some(b) = t.clips.iter().find(|c| c.start == a.end()) else {
        return Preview::default();
    };
    let (target, snapped) = ctx.snapped(target);
    let lo = (1 - a.duration).max(-b.source_in);
    let hi = (b.duration - 1).min(ctx.media_frames(a) - a.source_in - a.duration);
    let delta = (target - from).clamp(lo.min(0), hi.max(0));
    Preview {
        request: (delta != 0)
            .then(|| {
                request(
                    "edit.roll",
                    json!({ "clip": outgoing, "edge": "tail", "by": delta }),
                )
            })
            .flatten(),
        snapped,
        delta: Some((delta, None)),
        ghost: Some((a.start, a.end() + delta)),
        row: Some(t.id.clone()),
        ..Preview::default()
    }
}

fn slip_preview(ctx: &Ctx, pos: Pos2, press: Pos2, clips: &[String]) -> Preview {
    // The content follows the pointer: dragging right shows earlier media.
    let moved = (f64::from(pos.x - press.x) / ctx.scale.ppf).round() as i64;
    let mut by = -moved;
    for id in clips {
        if let Some(c) = ctx.clip(id) {
            let frames = ctx.media_frames(c);
            by = by.clamp(
                -c.source_in,
                (frames - c.source_in - c.duration).max(-c.source_in),
            );
        }
    }
    let req = match clips {
        [] => None,
        _ if by == 0 => None,
        [one] => request("edit.slip", json!({ "clip": one, "by": by })),
        many => request(
            "apply",
            json!({
                "commands": many.iter().map(|c| json!({ "method": "edit.slip", "params": { "clip": c, "by": by } })).collect::<Vec<_>>(),
                "label": format!("slip {} clips", many.len()),
            }),
        ),
    };
    Preview {
        request: req,
        delta: Some((by, None)),
        ..Preview::default()
    }
}

fn audio_roll_preview(ctx: &Ctx, target: i64, outgoing: &str, from: i64) -> Preview {
    let (Some(t), Some(a)) = (ctx.track_of(outgoing), ctx.clip(outgoing)) else {
        return Preview::default();
    };
    let Some(b) = t.clips.iter().find(|c| c.start == a.end()) else {
        return Preview::default();
    };
    let tail = a.audio.as_ref().map_or(0, |x| x.tail);
    let (target, snapped) = ctx.snapped(target);
    // The audio edit stays within the two clips.
    let lo = 1 - a.duration - tail;
    let hi = b.duration - 1 - tail;
    let delta = (target - from).clamp(lo.min(0), hi.max(0));
    Preview {
        request: (delta != 0)
            .then(|| {
                request(
                    "edit.audio_roll",
                    json!({ "clip": outgoing, "edge": "tail", "by": delta }),
                )
            })
            .flatten(),
        snapped,
        delta: Some((delta, None)),
        ..Preview::default()
    }
}

fn marquee_preview(ctx: &Ctx, press: Pos2, pos: Pos2, additive: bool) -> Preview {
    let rect = Rect::from_two_pos(press, pos);
    let (a, b) = ctx.scale.visible(rect.left(), rect.right());
    let mut clips: Vec<String> = if additive {
        ctx.selected.to_vec()
    } else {
        Vec::new()
    };
    for row in &ctx.rows.rows {
        let Some(t) = ctx.project.track(&row.track) else {
            continue;
        };
        if !rect.intersects(row.rect.rect) {
            continue;
        }
        for c in &t.clips[clips_in(t, a, b)] {
            let (x0, x1) = (ctx.scale.xf(c.start), ctx.scale.xf(c.end()));
            if x0 < rect.right() && rect.left() < x1 && !clips.contains(&c.id) {
                clips.push(c.id.clone());
            }
        }
    }
    Preview {
        request: request("selection.set", json!({ "clips": clips })),
        marquee: Some(rect),
        ..Preview::default()
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{pos2, vec2};
    use vidcut_core::model::{
        AudioInfo, ClipAudio, Marks, Media, MediaInfo, Settings, TrackKind, VideoInfo,
    };
    use vidcut_core::time::Rational;

    use super::*;

    fn media(id: &str, frames: i64) -> Media {
        Media {
            id: id.into(),
            path: format!("/{id}.mp4").into(),
            name: id.into(),
            size: 0,
            mtime_ns: 0,
            info: MediaInfo {
                format: "mov".into(),
                duration: frames as f64 / 25.0,
                start_time: 0.0,
                video: Some(VideoInfo {
                    codec: "h264".into(),
                    width: 320,
                    height: 180,
                    sar: [1, 1],
                    pix_fmt: "yuv420p".into(),
                    fps: Rational::new(25, 1).unwrap(),
                    vfr: false,
                    first_pts: 0.0,
                    duration: frames as f64 / 25.0,
                    rotation: 0,
                    fields: None,
                    hdr: None,
                    still: false,
                    mastering: None,
                    light: None,
                    hdr10plus: false,
                    dolby_vision: None,
                }),
                audio: vec![AudioInfo {
                    index: 0,
                    codec: "aac".into(),
                    sample_rate: 48_000,
                    channels: 2,
                    first_pts: 0.0,
                    duration: frames as f64 / 25.0,
                }],
                timecode: None,
            },
            marks: Marks::default(),
            bin: String::new(),
            sequence: None,
            proxy: None,
            fingerprint: None,
        }
    }

    fn clip(id: &str, start: i64, duration: i64, source_in: i64) -> Clip {
        Clip {
            id: id.into(),
            media: "m1".into(),
            start,
            duration,
            source_in,
            video: true,
            audio: Some(ClipAudio {
                stream: 0,
                head: 0,
                tail: 0,
                gain_db: 0.0,
                muted: false,
            }),
            enabled: true,
            name: None,
        }
    }

    /// 25 fps; m1 has 500 frames. c1 0..100 (src 0), c2 100..150 (src 200),
    /// c3 150..250 (src 300). Drawn at 2 px per frame, track y 100..200.
    fn project() -> Project {
        let mut p = Project::new(Settings {
            fps: Rational::new(25, 1).unwrap(),
            auto_format: false,
            ..Settings::default()
        });
        p.media.push(media("m1", 500));
        p.media.push(media("m2", 300));
        p.tracks[0] = Track {
            clips: vec![
                clip("c1", 0, 100, 0),
                clip("c2", 100, 50, 200),
                clip("c3", 150, 100, 300),
            ],
            ..Track::new("V1", TrackKind::Video)
        };
        p
    }

    struct World {
        p: Project,
        snap: Snap,
        snapping: bool,
        safe_edit: bool,
        rows: Rows,
    }

    /// V1 at y 100..200, V2 above it at 40..90 when there is one, and the
    /// strip for a new video track above that.
    fn rows(p: &Project) -> Rows {
        let mut rows = vec![super::super::geometry::Row {
            track: "V1".into(),
            rect: super::super::geometry::TrackRect::new(
                Rect::from_min_size(pos2(0.0, 100.0), vec2(1000.0, 100.0)),
                0.3,
            ),
        }];
        if p.track("V2").is_some() {
            rows.insert(
                0,
                super::super::geometry::Row {
                    track: "V2".into(),
                    rect: super::super::geometry::TrackRect::new(
                        Rect::from_min_size(pos2(0.0, 40.0), vec2(1000.0, 50.0)),
                        0.3,
                    ),
                },
            );
        }
        Rows {
            rows,
            area: Rect::from_min_max(pos2(0.0, 26.0), pos2(1000.0, 260.0)),
        }
    }

    impl World {
        fn new() -> Self {
            let p = project();
            Self {
                snap: Snap::new(&p, None, &[]),
                rows: rows(&p),
                p,
                snapping: false,
                safe_edit: false,
            }
        }

        /// With a V2 track holding `clips`.
        fn with_v2(clips: Vec<Clip>) -> Self {
            let mut p = project();
            p.tracks.insert(
                1,
                Track {
                    clips,
                    ..Track::new("V2", TrackKind::Video)
                },
            );
            Self {
                snap: Snap::new(&p, None, &[]),
                rows: rows(&p),
                p,
                snapping: false,
                safe_edit: false,
            }
        }

        fn ctx(&self) -> Ctx<'_> {
            Ctx {
                project: &self.p,
                scale: Scale::fit(0.0, 1000.0, 500),
                rows: &self.rows,
                snap: self.snapping.then_some(&self.snap),
                safe_edit: self.safe_edit,
                selected: &[],
            }
        }

        /// Presses at `from`, moves through `path` (pausing `pause` at the
        /// last point) and releases there.
        fn drag(
            &self,
            gesture: Gesture,
            from: Pos2,
            path: &[Pos2],
            pause: Duration,
        ) -> Option<Value> {
            let ctx = self.ctx();
            let t0 = Instant::now();
            let mut d = Drag::new(gesture, from);
            let mut now = t0;
            for p in path {
                now += Duration::from_millis(16);
                d.update(*p, now, &ctx);
            }
            let end = *path.last().unwrap_or(&from);
            d.finish(end, now + pause, &ctx)
        }
    }

    fn mv(clips: &[&str], grab_x: f32, start: i64, len: i64) -> Gesture {
        Gesture::Move {
            clips: clips.iter().map(|s| s.to_string()).collect(),
            from: "V1".into(),
            grab: f64::from(grab_x) / 2.0,
            start,
            len,
        }
    }

    #[test]
    fn dropping_a_clip_on_an_edit_inserts_there() {
        // The edit highlights; the clip is inserted there.
        let w = World::new();
        let r = w.drag(
            mv(&["c3"], 400.0, 150, 100),
            pos2(400.0, 150.0),
            &[pos2(250.0, 150.0), pos2(201.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(
                json!({"method": "edit.move", "params": {"clips": ["c3"], "to": 100, "mode": "insert"}})
            )
        );
    }

    #[test]
    fn dropping_on_a_clip_replaces_it_and_hovering_overwrites() {
        // Decision D11.
        let w = World::new();
        let quick = w.drag(
            mv(&["c3"], 400.0, 150, 100),
            pos2(400.0, 150.0),
            &[pos2(120.0, 150.0)],
            Duration::from_millis(100),
        );
        assert_eq!(
            quick,
            Some(json!({"method": "edit.replace", "params": {"clip": "c3", "onto": "c1"}}))
        );
        let slow = w.drag(
            mv(&["c3"], 400.0, 150, 100),
            pos2(400.0, 150.0),
            &[pos2(120.0, 150.0)],
            HOVER,
        );
        // The pointer moved 140 frames left: the block lands at 10.
        assert_eq!(
            slow,
            Some(
                json!({"method": "edit.move", "params": {"clips": ["c3"], "to": 10, "mode": "overwrite"}})
            )
        );
    }

    #[test]
    fn a_small_drag_overwrites_the_neighbors_edge() {
        // C2 dragged 10 frames right, the pointer still over c2.
        let w = World::new();
        let r = w.drag(
            mv(&["c2"], 250.0, 100, 50),
            pos2(250.0, 150.0),
            &[pos2(270.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(
                json!({"method": "edit.move", "params": {"clips": ["c2"], "to": 110, "mode": "overwrite"}})
            )
        );
        // Dragged away and back where it started: nothing.
        let r = w.drag(
            mv(&["c2"], 250.0, 100, 50),
            pos2(250.0, 150.0),
            &[pos2(290.0, 150.0), pos2(250.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(r, None);
        // Dragged off the track: nothing.
        let r = w.drag(
            mv(&["c2"], 250.0, 100, 50),
            pos2(250.0, 150.0),
            &[pos2(270.0, 20.0)],
            Duration::ZERO,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn moves_snap_to_edges() {
        // §8.8: c2's head lands 3 frames from c3's end (250) and snaps.
        let mut w = World::new();
        w.snapping = true;
        w.snap = Snap::new(&w.p, None, &["c2".to_string()]);
        let r = w.drag(
            mv(&["c2"], 250.0, 100, 50),
            pos2(250.0, 150.0),
            &[pos2(546.0, 150.0)],
            HOVER,
        );
        assert_eq!(
            r.as_ref().map(|v| v["params"]["to"].clone()),
            Some(json!(250)),
            "{r:?}"
        );
    }

    #[test]
    fn trims_ripple_unless_alt_and_respect_media() {
        // Dragging c2's tail 10 frames right.
        let w = World::new();
        let tail = Gesture::Trim {
            clip: "c2".into(),
            edge: Edge::Tail,
            from: 150,
            ripple: true,
        };
        let r = w.drag(
            tail.clone(),
            pos2(300.0, 150.0),
            &[pos2(320.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(
                json!({"method": "edit.trim", "params": {"clip": "c2", "edge": "tail", "by": 10}})
            )
        );
        // Alt: no ripple.
        let alt = Gesture::Trim {
            clip: "c2".into(),
            edge: Edge::Tail,
            from: 150,
            ripple: false,
        };
        let r = w.drag(
            alt,
            pos2(300.0, 150.0),
            &[pos2(320.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(r.unwrap()["params"]["ripple"], json!(false));
        // c1's head can't go before the media's first frame.
        let head = Gesture::Trim {
            clip: "c1".into(),
            edge: Edge::Head,
            from: 0,
            ripple: true,
        };
        assert_eq!(
            w.drag(
                head,
                pos2(1.0, 150.0),
                &[pos2(-40.0, 150.0)],
                Duration::ZERO
            ),
            None
        );
    }

    #[test]
    fn safe_edit_stops_at_the_neighbor_until_pushed() {
        // With Ripple Off: c2 (100..150) ends where c3 starts, so
        // extending its tail stops at once; pushing on overrides it.
        let mut w = World::new();
        w.safe_edit = true;
        let g = Gesture::Trim {
            clip: "c2".into(),
            edge: Edge::Tail,
            from: 150,
            ripple: false,
        };
        let ctx = w.ctx();
        let t0 = Instant::now();
        let mut d = Drag::new(g, pos2(300.0, 150.0));
        let p = d.update(pos2(330.0, 150.0), t0, &ctx);
        assert!(p.blocked);
        assert_eq!(p.request, None);
        let p = d.update(pos2(330.0, 150.0), t0 + SAFE_EDIT_HOLD, &ctx);
        assert!(!p.blocked);
        assert_eq!(p.request.unwrap()["params"]["by"], json!(15));
    }

    #[test]
    fn rolls_slips_and_audio_rolls() {
        let w = World::new();
        // Roll the c1|c2 edit 5 frames left.
        let r = w.drag(
            Gesture::Roll {
                outgoing: "c1".into(),
                from: 100,
            },
            pos2(200.0, 150.0),
            &[pos2(190.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(
                json!({"method": "edit.roll", "params": {"clip": "c1", "edge": "tail", "by": -5}})
            )
        );
        // Dragging c2's slip handle 4 frames right shows earlier media.
        let r = w.drag(
            Gesture::Slip {
                clips: vec!["c2".into()],
            },
            pos2(250.0, 110.0),
            &[pos2(258.0, 110.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(json!({"method": "edit.slip", "params": {"clip": "c2", "by": -4}}))
        );
        // An L-cut, c1's audio 6 frames into c2.
        let r = w.drag(
            Gesture::AudioRoll {
                outgoing: "c1".into(),
                from: 100,
            },
            pos2(200.0, 190.0),
            &[pos2(212.0, 190.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(
                json!({"method": "edit.audio_roll", "params": {"clip": "c1", "edge": "tail", "by": 6}})
            )
        );
    }

    #[test]
    fn media_drops_follow_the_cut_page_rules() {
        // Spec §7.3.
        let w = World::new();
        let g = || Gesture::Media {
            media: "m2".into(),
            range: (0, 40),
            video_only: false,
            audio_only: false,
        };
        let at = |x: f32, pause: Duration| w.drag(g(), pos2(x, 150.0), &[pos2(x, 150.0)], pause);
        let method = |v: Option<Value>| v.map(|v| (v["method"].clone(), v["params"].clone()));
        // Past the end: append.
        assert_eq!(
            method(at(700.0, Duration::ZERO)).unwrap().0,
            json!("edit.append")
        );
        // On an edit: insert.
        let (m, p) = method(at(299.0, Duration::ZERO)).unwrap();
        assert_eq!((m, p["at"].clone()), (json!("edit.insert"), json!(150)));
        // On a clip right away: ripple overwrite of that clip.
        let (m, p) = method(at(400.0, Duration::ZERO)).unwrap();
        assert_eq!(m, json!("edit.ripple_overwrite"));
        assert_eq!(
            (p["at"].clone(), p["use_marks"].clone()),
            (json!(150), json!(false))
        );
        // After hovering: overwrite at the pointer.
        let (m, p) = method(at(400.0, HOVER)).unwrap();
        assert_eq!((m, p["at"].clone()), (json!("edit.overwrite"), json!(200)));
    }

    #[test]
    fn scrubbing_and_marquee() {
        let w = World::new();
        let ctx = w.ctx();
        let now = Instant::now();
        // The ruler: the playhead goes under the pointer.
        let mut d = Drag::new(
            Gesture::Scrub {
                drag_content: false,
                from: 0.0,
            },
            pos2(10.0, 50.0),
        );
        assert_eq!(d.update(pos2(101.0, 50.0), now, &ctx).seek, Some(50));
        // Fixed playhead: dragging the content left moves the playhead on.
        let mut d = Drag::new(
            Gesture::Scrub {
                drag_content: true,
                from: 100.0,
            },
            pos2(500.0, 50.0),
        );
        assert_eq!(d.update(pos2(440.0, 50.0), now, &ctx).seek, Some(130));
        // A marquee from empty space over c2 and c3.
        let mut d = Drag::new(Gesture::Marquee { additive: false }, pos2(250.0, 90.0));
        let r = d.finish(pos2(320.0, 160.0), now, &ctx);
        assert_eq!(
            r,
            Some(json!({"method": "selection.set", "params": {"clips": ["c2", "c3"]}}))
        );
    }

    /// A clip moved along a track other than Track 1 shows there while it
    /// is dragged, not on Track 1.
    #[test]
    fn a_moved_clip_shows_on_its_own_track() {
        let w = World::with_v2(vec![clip("b1", 300, 40, 0)]);
        let ctx = w.ctx();
        let mut d = Drag::new(
            Gesture::Move {
                clips: vec!["b1".into()],
                from: "V2".into(),
                grab: 320.0,
                start: 300,
                len: 40,
            },
            pos2(640.0, 60.0),
        );
        let pv = d.update(pos2(700.0, 60.0), Instant::now(), &ctx);
        assert_eq!(
            (pv.row.as_deref(), pv.ghost),
            (Some("V2"), Some((330, 370)))
        );
        assert_eq!(pv.target, Target::Overwrite(330, 370));
        assert!(
            pv.request
                .is_some_and(|r| r["params"].get("track").is_none())
        );
    }

    #[test]
    fn clips_and_media_go_to_the_track_under_the_pointer() {
        // Rows for V2 (y 40..90) over V1 (100..200); a new video track
        // above them, a new audio track below.
        let mut b = clip("b1", 300, 40, 0);
        b.audio = None;
        let w = World::with_v2(vec![b]);
        let method = |v: Option<Value>| v.map(|v| (v["method"].clone(), v["params"].clone()));
        // A Track 1 clip dragged up into V2 overwrites there.
        let (m, p) = method(w.drag(
            mv(&["c2"], 250.0, 100, 50),
            pos2(250.0, 150.0),
            &[pos2(250.0, 120.0), pos2(260.0, 60.0)],
            Duration::ZERO,
        ))
        .unwrap();
        assert_eq!(m, "edit.move");
        assert_eq!(
            (p["track"].clone(), p["mode"].clone(), p["to"].clone()),
            (json!("V2"), json!("overwrite"), json!(105))
        );
        // Above every track: a new one.
        let (_, p) = method(w.drag(
            mv(&["c2"], 250.0, 100, 50),
            pos2(250.0, 150.0),
            &[pos2(250.0, 30.0)],
            Duration::ZERO,
        ))
        .unwrap();
        assert_eq!(p["track"], "V3");
        // The V2 clip moves along its own track without a track change...
        let from_v2 = Gesture::Move {
            clips: vec!["b1".into()],
            from: "V2".into(),
            grab: 320.0,
            start: 300,
            len: 40,
        };
        let (m, p) = method(w.drag(
            from_v2.clone(),
            pos2(640.0, 60.0),
            &[pos2(700.0, 60.0)],
            Duration::ZERO,
        ))
        .unwrap();
        assert_eq!(
            (m, p.get("track").is_none(), p["to"].clone()),
            (json!("edit.move"), true, json!(330))
        );
        // ...and down into a Track 1 edit point it inserts there.
        let (_, p) = method(w.drag(
            from_v2.clone(),
            pos2(640.0, 60.0),
            &[pos2(300.0, 150.0)],
            Duration::ZERO,
        ))
        .unwrap();
        assert_eq!(
            (p["track"].clone(), p["mode"].clone(), p["to"].clone()),
            (json!("V1"), json!("insert"), json!(150))
        );
        // Pictures don't go on audio tracks.
        assert!(
            w.drag(
                mv(&["c2"], 250.0, 100, 50),
                pos2(250.0, 150.0),
                &[pos2(250.0, 230.0)],
                Duration::ZERO
            )
            .is_none()
        );
        // Media dropped on V2 is written there; below the rows it makes an
        // audio track and brings only its sound.
        let media = || Gesture::Media {
            media: "m2".into(),
            range: (0, 40),
            video_only: false,
            audio_only: false,
        };
        let (m, p) = method(w.drag(
            media(),
            pos2(100.0, 60.0),
            &[pos2(100.0, 60.0)],
            Duration::ZERO,
        ))
        .unwrap();
        assert_eq!(
            (m, p["track"].clone(), p["at"].clone()),
            (json!("edit.overwrite"), json!("V2"), json!(50))
        );
        let (_, p) = method(w.drag(
            media(),
            pos2(100.0, 230.0),
            &[pos2(100.0, 230.0)],
            Duration::ZERO,
        ))
        .unwrap();
        assert_eq!(p["track"], "A1");
    }

    #[test]
    fn sound_alone_goes_on_an_audio_track() {
        // Music dropped on a video track goes to an audio track.
        let mut w = World::new();
        let mut music = media("m3", 500);
        music.info.video = None;
        w.p.media.push(music);
        let g = Gesture::Media {
            media: "m3".into(),
            range: (0, 100),
            video_only: false,
            audio_only: false,
        };
        let r = w
            .drag(g, pos2(100.0, 150.0), &[pos2(100.0, 150.0)], Duration::ZERO)
            .unwrap();
        assert_eq!(r["params"]["track"], "A1");
    }

    #[test]
    fn marquee_selects_across_rows() {
        let mut b = clip("b1", 40, 30, 0);
        b.audio = None;
        let w = World::with_v2(vec![b]);
        let r = w
            .drag(
                Gesture::Marquee { additive: false },
                pos2(60.0, 30.0),
                &[pos2(120.0, 180.0)],
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(r["params"]["clips"], json!(["b1", "c1"]));
    }
}
