use std::ops::Range;
use std::time::{Duration, Instant};

use eframe::egui::{Pos2, Rect};
use serde_json::{Value, json};
use vidcut_core::model::{
    Clip, Handle, Keyframe, Marks, PictureTransition, Project, Track, TrackKind,
};
use vidcut_core::ops::{Edge, MAIN};
use vidcut_core::validate::GAIN;

use super::EditSel;
use super::edits::{Plan, Trim, applied};
use super::geometry::{
    Hit, Rows, SNAP_PX, Scale, Snap, clip_edge_near, clips_in, edit_near, hit_rows, level_at,
    sound_band, transition_over, transition_span, volume_gain, volume_level, volume_span, volume_y,
};

pub const HOVER: Duration = Duration::from_millis(800);

const VOLUME_FINE: f64 = 0.1;

const AXIS_PX: f32 = 3.0;

pub const SAFE_EDIT_HOLD: Duration = Duration::from_millis(600);

pub const HELD_FRAME: Duration = Duration::from_millis(16);

const DROP_EDIT_PX: f32 = 6.0;

const TRANSITION_EDGE_PX: f32 = 24.0;

const TRANSITION_CENTER_PX: f32 = 6.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    Scrub {
        drag_content: bool,
        from: f64,
    },
    Move {
        clips: Vec<String>,
        from: String,
        grab: f64,
        start: i64,
        len: i64,
    },
    Trim {
        clip: String,
        edge: Edge,
        from: i64,
        ripple: bool,
    },
    Roll {
        outgoing: String,
        from: i64,
    },
    Edits {
        edits: Vec<EditSel>,
        lead: EditSel,
        clips: Vec<String>,
    },
    Slip {
        clips: Vec<String>,
    },
    Fade {
        track: String,
        at: i64,
        end: bool,
        before: i64,
        after: i64,
        sound: bool,
    },
    AudioRoll {
        outgoing: String,
        from: i64,
    },
    Marquee {
        additive: bool,
    },
    Marker {
        id: String,
        from: i64,
    },
    Mark {
        is_in: bool,
        from: i64,
    },
    Media {
        media: String,
        range: (i64, i64),
        video_only: bool,
        audio_only: bool,
    },
    Bin {
        path: String,
    },
    Subtitles {
        media: String,
        cues: Vec<(i64, i64)>,
    },
    Title {
        template: Box<crate::gui::title_browser::Template>,
        len: i64,
    },
    Transition {
        kind: PictureTransition,
        label: &'static str,
    },
    CueMove {
        cues: Vec<String>,
        from: String,
        grab: f64,
        start: i64,
        len: i64,
    },
    CueTrim {
        cue: String,
        edge: Edge,
        from: i64,
    },
    Volume {
        clips: Vec<String>,
        clip: String,
        level: f64,
        last: f32,
        span: f32,
        at: f64,
    },
    Key {
        clip: String,
        frame: i64,
        keys: Vec<(String, i64)>,
        across: Option<bool>,
    },
    KeyHandle {
        clip: String,
        frame: i64,
        out: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Target {
    #[default]
    None,
    Insert(i64),
    Replace(String),
    Overwrite(i64, i64),
    Append,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Preview {
    pub request: Option<Value>,
    pub target: Target,
    pub ghost: Option<(i64, i64)>,
    pub snapped: Option<i64>,
    pub delta: Option<(i64, Option<i64>)>,
    pub seek: Option<i64>,
    pub marquee: Option<Rect>,
    pub blocked: bool,
    pub row: Option<String>,
    pub cues: Vec<(i64, i64)>,
    pub marks: Option<Marks>,
    pub marker: Option<(String, i64)>,
    pub gains: Option<Gains>,
    pub trims: Vec<Trim>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gains {
    pub clips: Vec<(String, f64)>,
    pub keys: Vec<(String, Vec<Keyframe>)>,
    pub picked: Vec<(String, i64)>,
    pub shown: f64,
    pub y: Option<f32>,
}

impl Preview {
    pub fn moved(&self, p: &Project) -> Option<Project> {
        if !self.trims.is_empty() {
            return applied(p, &self.trims).ok();
        }
        if self.marks.is_none() && self.marker.is_none() && self.gains.is_none() {
            return None;
        }
        let mut moved = p.clone();
        if let Some(m) = &self.marks {
            moved.marks = m.clone();
        }
        if let Some((id, to)) = &self.marker
            && let Some(k) = moved.markers.iter_mut().find(|k| &k.id == id)
        {
            k.at = *to;
        }
        for (id, gain) in self.gains.iter().flat_map(|g| &g.clips) {
            if let Some((ti, ci)) = moved.find_clip(id)
                && let Some(a) = moved.tracks[ti].clips[ci].audio.as_mut()
            {
                a.gain_db = *gain;
            }
        }
        for (id, keys) in self.gains.iter().flat_map(|g| &g.keys) {
            if let Some((ti, ci)) = moved.find_clip(id)
                && let Some(a) = moved.tracks[ti].clips[ci].audio.as_mut()
            {
                a.gain_keys.clone_from(keys);
            }
        }
        Some(moved)
    }
}

pub struct Ctx<'a> {
    pub project: &'a Project,
    pub scale: Scale,
    pub rows: &'a Rows,
    pub snap: Option<&'a Snap>,
    pub safe_edit: bool,
    pub selected: &'a [String],
}

impl Ctx<'_> {
    fn v1(&self) -> Option<&Track> {
        self.project.track(MAIN)
    }

    fn clip(&self, id: &str) -> Option<&Clip> {
        self.project.clip(id)
    }

    fn track_of(&self, id: &str) -> Option<&Track> {
        self.project
            .find_clip(id)
            .map(|(ti, _)| &self.project.tracks[ti])
    }

    fn source_range(&self, c: &Clip) -> Range<i64> {
        self.project
            .media(&c.media)
            .map_or(0..i64::MAX / 4, |m| self.project.media_range(m, c))
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

pub fn swipe(from: f64, dx: f32, ppf: f64, drag_content: bool, end: i64) -> f64 {
    let frames = f64::from(dx) / ppf;
    let to = if drag_content {
        from - frames
    } else {
        from + frames
    };
    to.clamp(0.0, end as f64)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub gesture: Gesture,
    pub press: Pos2,
    pub fine: bool,
    hover: Option<(String, Instant)>,
    blocked_since: Option<Instant>,
}

impl Drag {
    pub fn new(gesture: Gesture, press: Pos2) -> Self {
        Self {
            gesture,
            press,
            fine: false,
            hover: None,
            blocked_since: None,
        }
    }

    pub fn update(&mut self, pos: Pos2, now: Instant, ctx: &Ctx) -> Preview {
        match &mut self.gesture {
            Gesture::Volume {
                clips,
                clip,
                level,
                last,
                span,
                at,
            } => {
                let step = if self.fine { VOLUME_FINE } else { 1.0 };
                let moved = f64::from(*last - pos.y) / f64::from(span.max(1.0)) * step;
                *level = (*level + moved).clamp(0.0, 1.0);
                *last = pos.y;
                return volume_preview(ctx, clips, clip, *level, *at);
            }
            Gesture::Key {
                clip,
                frame,
                keys,
                across,
            } => {
                let d = pos - self.press;
                if self.fine && across.is_none() && d.length() >= AXIS_PX {
                    *across = Some(d.x.abs() >= d.y.abs());
                }
                return key_preview(ctx, (self.press, pos), *across, clip, *frame, keys);
            }
            Gesture::KeyHandle { clip, frame, out } => {
                return handle_preview(ctx, pos, clip, *frame, *out);
            }
            _ => {}
        }
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
            Gesture::Bin { .. }
            | Gesture::Volume { .. }
            | Gesture::Key { .. }
            | Gesture::KeyHandle { .. } => Preview::default(),

            Gesture::Subtitles { media, cues } => subtitles_preview(ctx, pos, media, cues),
            Gesture::Title { template, len } => title_preview(ctx, pos, template, *len),
            Gesture::Transition { kind, .. } => transition_preview(ctx, pos, *kind),
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
            Gesture::Edits { edits, lead, .. } => {
                edits_preview(ctx, lead.at + moved(ctx, self.press, pos), edits, *lead)
            }
            Gesture::Slip { clips } => slip_preview(ctx, pos, self.press, clips),
            Gesture::Fade {
                track,
                at,
                end,
                before,
                after,
                sound,
            } => {
                let d = moved(ctx, self.press, pos);
                let grow = if *end { d } else { -d };
                let room = vidcut_core::ops::dissolve_room(ctx.project, track, *at, false)
                    .unwrap_or((*before, *after));
                let (b, a) = crate::gui::trim_strips::grown(*before, *after, grow, room);
                let edge = if *end { a - after } else { before - b };
                let kind = ctx
                    .project
                    .track(track)
                    .and_then(|t| super::geometry::picture_transition(ctx.project, t, *at))
                    .map(super::fades::kind)
                    .unwrap_or_default();
                Preview {
                    request: ((b, a) != (*before, *after))
                        .then(|| {
                            request(
                                "edit.dissolve",
                                json!({
                                    "at": at,
                                    "track": track,
                                    "before": format!("{b}f"),
                                    "after": format!("{a}f"),
                                    "sound": sound,
                                    "transition": kind,
                                }),
                            )
                        })
                        .flatten(),
                    delta: Some((edge, Some(b + a))),
                    ghost: Some((at - b, at + a)),
                    row: Some(track.clone()),
                    ..Preview::default()
                }
            }
            Gesture::AudioRoll { outgoing, from } => {
                audio_roll_preview(ctx, *from + moved(ctx, self.press, pos), outgoing, *from)
            }
            Gesture::Marquee { additive } => marquee_preview(ctx, self.press, pos, *additive),
            Gesture::CueMove {
                cues,
                from,
                grab,
                start,
                len,
            } => cue_move_preview(ctx, pos, cues, from, *grab, (*start, *len)),
            Gesture::CueTrim { cue, edge, from } => {
                cue_trim_preview(ctx, *from + moved(ctx, self.press, pos), cue, *edge)
            }
            Gesture::Marker { id, from } => {
                let (to, snapped) = ctx.snapped(ctx.scale.boundary(pos.x).max(0));
                Preview {
                    request: (to != *from)
                        .then(|| request("markers.move", json!({ "marker": id, "to": to })))
                        .flatten(),
                    snapped,
                    delta: Some((to - from, None)),
                    marker: Some((id.clone(), to)),
                    ..Preview::default()
                }
            }
            Gesture::Mark { is_in, from } => {
                let (f, snapped) = ctx.snapped(ctx.scale.boundary(pos.x).max(0));
                let marks = &ctx.project.marks;
                let f = if *is_in {
                    marks.mark_out.map_or(f, |o| f.min(o - 1))
                } else {
                    marks.mark_in.map_or(f.max(1), |i| f.max(i + 1))
                };
                let key = if *is_in { "in" } else { "out" };
                let mut moved = marks.clone();
                if *is_in {
                    moved.mark_in = Some(f);
                } else {
                    moved.mark_out = Some(f);
                }
                Preview {
                    request: (f != *from)
                        .then(|| request("marks.set", json!({ key: f })))
                        .flatten(),
                    snapped,
                    marks: Some(moved),
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
        let have = ctx.source_range(c);
        let (lo, hi) = match edge {
            Edge::Head => (-c.room(false, have), c.duration - 1),
            Edge::Tail => (1 - c.duration, c.room(true, have)),
        };
        delta = delta.clamp(lo, hi);
        if edge == Edge::Head && !ripple {
            delta = delta.max(-c.start);
        }
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
    let Some(dest) = ctx
        .rows
        .target(ctx.project, pos.y)
        .filter(|_| ctx.rows.reaches(pos))
    else {
        return Preview::default();
    };
    let one_track = clips
        .iter()
        .all(|c| ctx.track_of(c).is_some_and(|t| t.id == from));
    let dest = if one_track { dest } else { from.to_string() };
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
        return overwrite_move(clips, ghost, len, snapped, &dest, &elsewhere);
    }
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

fn subtitles_preview(ctx: &Ctx, pos: Pos2, media: &str, cues: &[(i64, i64)]) -> Preview {
    if !ctx.rows.reaches(pos) {
        return Preview::default();
    }
    let dest = ctx.rows.subtitle_target(ctx.project, pos.y);
    let len = cues.iter().map(|c| c.1).max().unwrap_or(1).max(1);
    let raw = ctx.scale.boundary(pos.x).max(0);
    let (at, snapped) = match ctx.snap {
        Some(s) => s.snap_block(raw, len, &ctx.scale, SNAP_PX),
        None => (raw, None),
    };
    let at = at.max(0);
    let mut params = json!({ "media": media, "at": format!("{at}f") });
    if ctx.project.subtitle_index(&dest).is_some() {
        params["track"] = json!(dest);
    }
    Preview {
        request: request("subtitles.import", params),
        ghost: Some((at, at + len)),
        cues: cues.iter().map(|&(a, b)| (at + a, at + b)).collect(),
        snapped,
        row: Some(dest),
        ..Preview::default()
    }
}

fn title_preview(
    ctx: &Ctx,
    pos: Pos2,
    template: &crate::gui::title_browser::Template,
    len: i64,
) -> Preview {
    let Some(dest) = ctx
        .rows
        .target(ctx.project, pos.y)
        .filter(|_| ctx.rows.reaches(pos))
        .filter(|d| d.starts_with('V'))
    else {
        return Preview::default();
    };
    let raw = ctx.scale.boundary(pos.x).max(0);
    let (at, snapped) = match ctx.snap {
        Some(s) => s.snap_block(raw, len, &ctx.scale, SNAP_PX),
        None => (raw, None),
    };
    let at = at.max(0);
    let mut params = template.params();
    params["at"] = json!(at);
    params["duration"] = json!(len);
    if ctx.project.track(&dest).is_some() {
        params["track"] = json!(dest);
    }
    Preview {
        request: request("titles.add", params),
        target: Target::Overwrite(at, at + len),
        ghost: Some((at, at + len)),
        snapped,
        row: Some(dest),
        ..Preview::default()
    }
}

fn transition_preview(ctx: &Ctx, pos: Pos2, kind: PictureTransition) -> Preview {
    let Some(row) = ctx
        .rows
        .at(pos.y)
        .filter(|r| r.track.starts_with('V') && ctx.rows.area.contains(pos))
    else {
        return Preview::default();
    };
    let Some(track) = ctx.project.track(&row.track) else {
        return Preview::default();
    };
    let id = row.track.clone();
    if let Some(at) = transition_over(ctx.project, track, &ctx.scale, pos.x) {
        return Preview {
            request: request(
                "edit.transition",
                json!({ "at": at, "track": id, "transition": kind }),
            ),
            ghost: transition_span(ctx.project, track, at),
            row: Some(id),
            ..Preview::default()
        };
    }
    let Some(at) = clip_edge_near(track, &ctx.scale, pos.x, TRANSITION_EDGE_PX) else {
        return Preview::default();
    };
    let dx = pos.x - ctx.scale.xf(at);
    let align = if dx.abs() <= TRANSITION_CENTER_PX {
        vidcut_core::ops::Align::Center
    } else if dx < 0.0 {
        vidcut_core::ops::Align::End
    } else {
        vidcut_core::ops::Align::Start
    };
    let length = ctx.project.settings.fps.as_f64().round() as i64;
    let shape = vidcut_core::ops::Shape::Length(length, align);
    let Some((before, after)) = vidcut_core::ops::dissolve_sides(ctx.project, &id, at, shape)
    else {
        return Preview::default();
    };
    Preview {
        request: request(
            "edit.dissolve",
            json!({
                "at": at,
                "track": id,
                "align": align,
                "transition": kind,
                "sound": false,
            }),
        ),
        ghost: Some((at - before, at + after)),
        row: Some(id),
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
    if t.clips.is_empty() || pos.x > ctx.scale.xf(end) + DROP_EDIT_PX {
        return Preview {
            request: request("edit.append", source),
            target: Target::Append,
            ghost: Some((end, end + len)),
            row,
            ..Preview::default()
        };
    }
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

fn tenth(gain_db: f64) -> f64 {
    let g = (gain_db * 10.0).round() / 10.0;
    if g == 0.0 { 0.0 } else { g }
}

fn kept(gain_db: f64) -> f64 {
    let g = (gain_db * 1e6).round() / 1e6;
    if g == 0.0 { 0.0 } else { g }
}

fn band_of(ctx: &Ctx, clip: &str) -> Option<Rect> {
    let c = ctx.clip(clip)?;
    let r = ctx.rows.get(&ctx.track_of(clip)?.id)?;
    sound_band(&r.rect, &ctx.scale, c)
}

fn command(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn one_or_all(commands: Vec<Value>) -> Option<Value> {
    match commands.as_slice() {
        [] => None,
        [only] => Some(only.clone()),
        _ => request("apply", json!({ "commands": commands })),
    }
}

fn segment(c: &Clip, at: f64) -> Vec<i64> {
    let keys = c.audio.as_ref().map_or(&[][..], |a| &a.gain_keys[..]);
    let mut timed: Vec<(f64, i64)> = keys
        .iter()
        .map(|k| (c.key_time(k.frame), k.frame))
        .collect();
    timed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let i = timed.partition_point(|(t, _)| *t <= at);
    match i {
        0 => timed.first().map(|k| vec![k.1]).unwrap_or_default(),
        i if i == timed.len() => vec![timed[i - 1].1],
        i => vec![timed[i - 1].1, timed[i].1],
    }
}

fn raised(keys: &[Keyframe], frames: Option<&[i64]>, by: f64) -> Vec<Keyframe> {
    keys.iter()
        .map(|k| {
            let picked = frames.is_none_or(|f| f.contains(&k.frame));
            Keyframe {
                value: if picked {
                    kept((k.value + by).clamp(*GAIN.start(), *GAIN.end()))
                } else {
                    k.value
                },
                ..*k
            }
        })
        .collect()
}

fn volume_preview(ctx: &Ctx, clips: &[String], clip: &str, level: f64, at: f64) -> Preview {
    let Some(c) = ctx.clip(clip) else {
        return Preview::default();
    };
    let Some(base) = c.gain_at(at) else {
        return Preview::default();
    };
    let to = Some(tenth(volume_gain(level)))
        .filter(|&g| g != tenth(base))
        .unwrap_or(base);
    let by = to - base;
    let (mut flat, mut keys, mut commands) = (Vec::new(), Vec::new(), Vec::new());
    let keyed = c.audio.as_ref().is_some_and(|a| a.is_keyed());
    for id in clips.iter().filter(|id| *id == clip || !keyed) {
        let Some(a) = ctx.clip(id).and_then(|c| c.audio.as_ref()) else {
            continue;
        };
        if a.is_keyed() {
            if by == 0.0 {
                continue;
            }
            let frames = (id == clip).then(|| segment(c, at));
            keys.push((id.clone(), raised(&a.gain_keys, frames.as_deref(), by)));
            let mut params = json!({ "clips": [id], "by": kept(by) });
            if let Some(f) = frames {
                params["frames"] = json!(f);
            }
            commands.push(command("keyframes.set", params));
            continue;
        }
        let g = match a.gain_db {
            _ if id == clip => to,
            g if by == 0.0 => g,
            g => kept((g + by).clamp(*GAIN.start(), *GAIN.end())),
        };
        flat.push((id.clone(), g));
        if g != a.gain_db {
            commands.push(command("clip.set", json!({ "clips": [id], "gain_db": g })));
        }
    }
    let y = band_of(ctx, clip).and_then(|band| volume_y(band, to));
    Preview {
        request: one_or_all(commands),
        gains: Some(Gains {
            clips: flat,
            keys,
            picked: Vec::new(),
            shown: to,
            y,
        }),
        ..Preview::default()
    }
}

fn key_preview(
    ctx: &Ctx,
    (press, pos): (Pos2, Pos2),
    across: Option<bool>,
    clip: &str,
    frame: i64,
    keys: &[(String, i64)],
) -> Preview {
    let (Some(c), Some(band)) = (ctx.clip(clip), band_of(ctx, clip)) else {
        return Preview::default();
    };
    let Some(k) = c
        .audio
        .as_ref()
        .and_then(|a| a.gain_keys.iter().find(|k| k.frame == frame))
        .copied()
    else {
        return Preview::default();
    };
    let (mut dx, mut dy) = (pos.x - press.x, pos.y - press.y);
    match across {
        Some(true) => dy = 0.0,
        Some(false) => dx = 0.0,
        None => {}
    }
    let level =
        (volume_level(k.value) - f64::from(dy) / f64::from(volume_span(band))).clamp(0.0, 1.0);
    let to = Some(tenth(volume_gain(level)))
        .filter(|&g| g != tenth(k.value))
        .unwrap_or(k.value);
    let dv = to - k.value;
    let mut per: Vec<(&Clip, Vec<i64>)> = Vec::new();
    for (id, f) in keys {
        let Some(c) = ctx.clip(id) else {
            continue;
        };
        match per.iter_mut().find(|(x, _)| x.id == *id) {
            Some((_, v)) => v.push(*f),
            None => per.push((c, vec![*f])),
        }
    }
    let fits = |dt: i64| {
        per.iter().all(|(c, moving)| {
            let all = c.audio.as_ref().map_or(&[][..], |a| &a.gain_keys[..]);
            let still: Vec<i64> = all
                .iter()
                .map(|k| k.frame)
                .filter(|f| !moving.contains(f))
                .collect();
            let mut new: Vec<i64> = moving.iter().map(|&f| f + dt).collect();
            let kept_order = moving.iter().zip(&new).all(|(&f, &n)| {
                let lo = still.iter().filter(|&&s| s < f).max();
                let hi = still.iter().filter(|&&s| s > f).min();
                lo.is_none_or(|&l| l < n) && hi.is_none_or(|&h| n < h)
            });
            new.sort_unstable();
            kept_order && new.windows(2).all(|w| w[0] < w[1])
        })
    };
    let mut dt = (f64::from(dx) / ctx.scale.ppf).round() as i64;
    while dt != 0 && !fits(dt) {
        dt -= dt.signum();
    }
    let (mut commands, mut shown, mut picked) = (Vec::new(), Vec::new(), Vec::new());
    for (c, moving) in &per {
        picked.extend(moving.iter().map(|&f| (c.id.clone(), f + dt)));
        let Some(a) = c.audio.as_ref() else {
            continue;
        };
        let mut next = a.gain_keys.clone();
        vidcut_core::keyframes::reshape(&mut next, |marked| {
            for (k, _) in marked.iter_mut().filter(|(k, _)| moving.contains(&k.frame)) {
                k.value = kept((k.value + dv).clamp(*GAIN.start(), *GAIN.end()));
                k.frame += dt;
            }
        });
        if dv != 0.0 {
            let params = json!({ "clips": [c.id], "frames": moving, "by": kept(dv) });
            commands.push(command("keyframes.set", params));
        }
        if dt != 0 {
            let params = json!({ "clips": [c.id], "frames": moving, "by": dt });
            commands.push(command("keyframes.move", params));
        }
        shown.push((c.id.clone(), next));
    }
    Preview {
        request: one_or_all(commands),
        gains: Some(Gains {
            clips: Vec::new(),
            keys: shown,
            picked,
            shown: to,
            y: volume_y(band, to),
        }),
        ..Preview::default()
    }
}

fn handle_preview(ctx: &Ctx, pos: Pos2, clip: &str, frame: i64, out: bool) -> Preview {
    let (Some(c), Some(band)) = (ctx.clip(clip), band_of(ctx, clip)) else {
        return Preview::default();
    };
    let keys = c.audio.as_ref().map_or(&[][..], |a| &a.gain_keys[..]);
    let Some(i) = keys.iter().position(|k| k.frame == frame) else {
        return Preview::default();
    };
    let other = if out {
        keys.get(i + 1)
    } else {
        i.checked_sub(1).and_then(|j| keys.get(j))
    };
    let Some(other) = other else {
        return Preview::default();
    };
    let k = keys[i];
    let len = (other.frame - k.frame).abs() as f64;
    let along = c.key_at(ctx.scale.frame(pos.x)) - k.frame as f64;
    let frames = if out {
        along.clamp(0.0, len)
    } else {
        along.clamp(-len, 0.0)
    };
    let value = tenth(volume_gain(level_at(band, pos.y)) - k.value);
    let h = Handle {
        frames: (frames * 100.0).round() / 100.0,
        value,
    };
    let mut next = keys.to_vec();
    let side = if out {
        &mut next[i].out_handle
    } else {
        &mut next[i].in_handle
    };
    let changed = *side != Some(h);
    *side = Some(h);
    let mut params = json!({ "clips": [clip], "frames": [frame] });
    params[if out { "out" } else { "in" }] = json!(h);
    Preview {
        request: changed.then(|| command("keyframes.set", params)),
        gains: Some(Gains {
            clips: Vec::new(),
            keys: vec![(clip.to_string(), next)],
            picked: Vec::new(),
            shown: k.value + value,
            y: Some(pos.y),
        }),

        ..Preview::default()
    }
}

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
    let lo = (1 - a.duration).max(-b.room(false, ctx.source_range(b)));
    let hi = (b.duration - 1).min(a.room(true, ctx.source_range(a)));
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

fn edits_preview(ctx: &Ctx, target: i64, edits: &[EditSel], lead: EditSel) -> Preview {
    let Ok(plan) = Plan::new(ctx.project, edits, lead) else {
        return Preview::default();
    };
    let (target, snapped) = ctx.snapped(target);
    let want = target - lead.at;
    let fits = |by: i64| {
        by.abs() <= want.abs() && applied(ctx.project, &plan.shared(ctx.project, by)).is_ok()
    };
    let by = match want {
        0 => 0,
        w if fits(w) => w,
        w => super::tools::furthest(0, w.signum(), fits),
    };
    let trims = match by {
        0 => Vec::new(),
        _ => plan.shared(ctx.project, by),
    };
    Preview {
        request: super::edits::request(&trims),
        snapped,
        delta: Some((by, None)),
        row: Some(lead.track.id()),
        trims,
        ..Preview::default()
    }
}

fn slip_preview(ctx: &Ctx, pos: Pos2, press: Pos2, clips: &[String]) -> Preview {
    let moved = (f64::from(pos.x - press.x) / ctx.scale.ppf).round() as i64;
    let mut by = -moved;
    for id in clips {
        if let Some(c) = ctx.clip(id) {
            let have = ctx.source_range(c);
            let lo = -c.room(false, have.clone());
            by = by.clamp(lo, c.room(true, have).max(lo));
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

fn cue_move_preview(
    ctx: &Ctx,
    pos: Pos2,
    cues: &[String],
    from: &str,
    grab: f64,
    (start, len): (i64, i64),
) -> Preview {
    let p = ctx.project;
    let one_track = cues.iter().all(|c| {
        p.find_cue(c)
            .is_some_and(|(t, _)| p.subtitles[t].id == from)
    });
    let dest = ctx
        .rows
        .at(pos.y)
        .map(|r| r.track.as_str())
        .filter(|t| one_track && p.subtitle_track(t).is_some_and(|s| !s.locked))
        .unwrap_or(from)
        .to_string();
    let raw = (start + (ctx.scale.frame(pos.x) - grab).round() as i64).max(0);
    let (at, snapped) = match ctx.snap {
        Some(s) => s.snap_block(raw, len, &ctx.scale, SNAP_PX),
        None => (raw, None),
    };
    let by = at.max(0) - start;
    let mut params = json!({ "cues": cues, "by": by });
    if dest != from {
        params["track"] = json!(dest);
    }
    Preview {
        request: (by != 0 || dest != from)
            .then(|| request("subtitles.move", params))
            .flatten(),
        ghost: Some((start + by, start + by + len)),
        snapped,
        delta: Some((by, None)),
        row: Some(dest),
        ..Preview::default()
    }
}

fn cue_trim_preview(ctx: &Ctx, target: i64, cue: &str, edge: Edge) -> Preview {
    let Some((ti, ci)) = ctx.project.find_cue(cue) else {
        return Preview::default();
    };
    let c = &ctx.project.subtitles[ti].cues[ci];
    let (target, snapped) = ctx.snapped(target);
    let (start, end) = match edge {
        Edge::Head => (target.clamp(0, c.end() - 1), c.end()),
        Edge::Tail => (c.start, target.max(c.start + 1)),
    };
    let (key, value, delta) = match edge {
        Edge::Head => ("start", start, start - c.start),
        Edge::Tail => ("end", end, end - c.end()),
    };
    Preview {
        request: (delta != 0)
            .then(|| request("subtitles.set", json!({ "cue": cue, key: value })))
            .flatten(),
        ghost: Some((start, end)),
        snapped,
        delta: Some((delta, Some(end - start))),
        row: Some(ctx.project.subtitles[ti].id.clone()),
        ..Preview::default()
    }
}

fn marquee_preview(ctx: &Ctx, press: Pos2, pos: Pos2, additive: bool) -> Preview {
    let rect = Rect::from_two_pos(press, pos);
    let (a, b) = ctx.scale.visible(rect.left(), rect.right());
    let (mut clips, mut cues): (Vec<String>, Vec<String>) = if additive {
        ctx.selected
            .iter()
            .cloned()
            .partition(|id| ctx.project.cue(id).is_none())
    } else {
        (Vec::new(), Vec::new())
    };
    for row in &ctx.rows.rows {
        if let Some(t) = ctx.project.subtitle_track(&row.track) {
            for (c, r) in super::cues::placed(t, &ctx.scale, row.rect.rect) {
                if r.intersects(rect) && !cues.contains(&c.id) {
                    cues.push(c.id.clone());
                }
            }
            continue;
        }
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
        request: request("selection.set", json!({ "clips": clips, "cues": cues })),
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
                    alpha: false,
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
            color: None,
            color_space: Default::default(),
            super_scale: Default::default(),
            left_out: Vec::new(),
            words: Vec::new(),
            fingerprint: None,
            subtitles: None,
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
                gain_keys: Vec::new(),
                muted: false,
                effects: None,
                voice_isolation: None,
            }),
            enabled: true,
            name: None,
            effects: None,
            generator: None,
            title: None,
            picture: None,
            compound: None,
            speed: None,
            retime: vidcut_core::model::ClipRetime::Project,
            color: None,
        }
    }

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
    fn a_transition_card_on_the_cut_of_a_transition_changes_its_type() {
        let mut w = World::new();
        let shape = vidcut_core::ops::Shape::Length(20, vidcut_core::ops::Align::Center);
        vidcut_core::ops::dissolve(
            &mut w.p,
            "V1",
            100,
            shape,
            true,
            false,
            false,
            PictureTransition::Dissolve,
        )
        .unwrap();
        let wipe = PictureTransition::Wipe {
            direction: vidcut_core::model::WipeDirection::Down,
        };
        let r = w.drag(
            Gesture::Transition {
                kind: wipe,
                label: "Edge Wipe",
            },
            pos2(200.0, 20.0),
            &[pos2(200.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(json!({"method": "edit.transition", "params": {
                "at": 100,
                "track": "V1",
                "transition": {"type": "wipe", "direction": "down"},
            }}))
        );
    }

    #[test]
    fn dragging_the_edge_of_a_wipe_keeps_it_a_wipe() {
        let mut w = World::new();
        let wipe = PictureTransition::Wipe {
            direction: vidcut_core::model::WipeDirection::Left,
        };
        let shape = vidcut_core::ops::Shape::Length(20, vidcut_core::ops::Align::Center);
        vidcut_core::ops::dissolve(&mut w.p, "V1", 100, shape, true, false, false, wipe).unwrap();
        let r = w.drag(
            Gesture::Fade {
                track: "V1".into(),
                at: 100,
                end: true,
                before: 10,
                after: 10,
                sound: false,
            },
            pos2(220.0, 150.0),
            &[pos2(240.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r.map(|r| r["params"]["transition"].clone()),
            Some(json!({"type": "wipe", "direction": "left"}))
        );
    }

    #[test]
    fn a_transition_card_left_of_an_edit_ends_on_it() {
        let w = World::new();
        let r = w.drag(
            Gesture::Transition {
                kind: PictureTransition::Dissolve,
                label: "Cross Dissolve",
            },
            pos2(185.0, 20.0),
            &[pos2(185.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(
            r,
            Some(json!({"method": "edit.dissolve", "params": {
                "at": 100,
                "track": "V1",
                "align": "end",
                "transition": {"type": "dissolve"},
                "sound": false,
            }}))
        );
    }

    #[test]
    fn a_transition_card_left_of_the_lanes_adds_nothing() {
        let w = World::new();
        let r = w.drag(
            Gesture::Transition {
                kind: PictureTransition::Dissolve,
                label: "Cross Dissolve",
            },
            pos2(-3.0, 20.0),
            &[pos2(-3.0, 150.0)],
            Duration::ZERO,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn dropping_a_clip_on_an_edit_inserts_there() {
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
        assert_eq!(
            slow,
            Some(
                json!({"method": "edit.move", "params": {"clips": ["c3"], "to": 10, "mode": "overwrite"}})
            )
        );
    }

    #[test]
    fn moves_snap_to_edges() {
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
    fn safe_edit_stops_at_the_neighbor_until_pushed() {
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
    fn media_drops_follow_the_cut_page_rules() {
        let w = World::new();
        let g = || Gesture::Media {
            media: "m2".into(),
            range: (0, 40),
            video_only: false,
            audio_only: false,
        };
        let at = |x: f32, pause: Duration| w.drag(g(), pos2(x, 150.0), &[pos2(x, 150.0)], pause);
        let method = |v: Option<Value>| v.map(|v| (v["method"].clone(), v["params"].clone()));
        assert_eq!(
            method(at(700.0, Duration::ZERO)).unwrap().0,
            json!("edit.append")
        );
        let (m, p) = method(at(299.0, Duration::ZERO)).unwrap();
        assert_eq!((m, p["at"].clone()), (json!("edit.insert"), json!(150)));
        let (m, p) = method(at(400.0, Duration::ZERO)).unwrap();
        assert_eq!(m, json!("edit.ripple_overwrite"));
        assert_eq!(
            (p["at"].clone(), p["use_marks"].clone()),
            (json!(150), json!(false))
        );
        let (m, p) = method(at(400.0, HOVER)).unwrap();
        assert_eq!((m, p["at"].clone()), (json!("edit.overwrite"), json!(200)));
    }

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
    fn sound_alone_goes_on_an_audio_track() {
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

    #[test]
    fn swiping_left_under_a_fixed_playhead_moves_on() {
        assert_eq!(swipe(100.0, -20.0, 4.0, true, 1000), 105.0);
    }

    #[test]
    fn swiping_left_on_the_upper_timeline_moves_back() {
        assert_eq!(swipe(100.0, -20.0, 4.0, false, 1000), 95.0);
    }

    #[test]
    fn a_swipe_keeps_less_than_a_frame() {
        assert_eq!(swipe(100.0, -1.0, 4.0, true, 1000), 100.25);
    }

    #[test]
    fn a_swipe_stops_at_the_end() {
        assert_eq!(swipe(998.0, -20.0, 4.0, true, 1000), 1000.0);
    }

    #[test]
    fn a_swipe_stops_at_the_start() {
        assert_eq!(swipe(2.0, 20.0, 4.0, true, 1000), 0.0);
    }
}
