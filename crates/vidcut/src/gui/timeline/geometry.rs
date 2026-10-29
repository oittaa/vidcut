use eframe::egui::{Pos2, Rect, pos2, vec2};
use vidcut_core::model::{Clip, Keyframe, Project, Track, TrackKind, Transition};
use vidcut_core::ops::{Edge, MAIN};
use vidcut_core::otio::transitions::Cuts;
use vidcut_core::validate::GAIN;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub origin: f64,
    pub ppf: f64,
}

impl Scale {
    pub fn fit(left: f32, width: f32, span: i64) -> Self {
        Self {
            origin: f64::from(left),
            ppf: f64::from(width.max(1.0)) / span.max(1) as f64,
        }
    }

    pub fn anchored(x: f32, frame: f64, ppf: f64) -> Self {
        Self {
            origin: f64::from(x) - frame * ppf,
            ppf,
        }
    }

    pub fn x(&self, frame: f64) -> f32 {
        (self.origin + frame * self.ppf) as f32
    }

    pub fn xf(&self, frame: i64) -> f32 {
        self.x(frame as f64)
    }

    pub fn frame(&self, x: f32) -> f64 {
        (f64::from(x) - self.origin) / self.ppf
    }

    pub fn boundary(&self, x: f32) -> i64 {
        self.frame(x).round() as i64
    }

    pub fn under(&self, x: f32) -> i64 {
        self.frame(x).floor() as i64
    }

    pub fn visible(&self, left: f32, right: f32) -> (i64, i64) {
        (
            self.frame(left).floor() as i64,
            self.frame(right).ceil() as i64,
        )
    }
}

pub fn covered(scale: &Scale, span: (i64, i64), next: Option<(i64, i64)>) -> bool {
    let thin = |(a, b): (i64, i64)| scale.xf(b) - scale.xf(a) < 1.0;
    let column = |(a, _): (i64, i64)| scale.xf(a).floor();
    thin(span) && next.is_some_and(|n| thin(n) && column(n) == column(span))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackRect {
    pub rect: Rect,
    pub audio_top: f32,
}

impl TrackRect {
    pub fn new(rect: Rect, audio_share: f32) -> Self {
        Self {
            rect,
            audio_top: rect.bottom() - (rect.height() * audio_share).round(),
        }
    }

    pub fn in_audio(&self, y: f32) -> bool {
        y >= self.audio_top
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub track: String,
    pub rect: TrackRect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rows {
    pub rows: Vec<Row>,
    pub area: Rect,
}

pub const NEW_TRACK_STRIP: f32 = 14.0;

pub const MARK_BAND: f32 = 12.0;

pub fn is_subtitle(track: &str) -> bool {
    track.starts_with('S')
}

const ENLARGE: f32 = 3.0;
const ENLARGED_MIN: f32 = 180.0;

impl Rows {
    pub fn layout(
        p: &Project,
        area: Rect,
        heights: [f32; 4],
        min: f32,
        strip: f32,
        enlarged: Option<&str>,
        lift: f32,
    ) -> Self {
        let [v1, video, audio, subtitle] = heights;
        let mut order: Vec<(&str, f32, f32)> = p
            .subtitles
            .iter()
            .rev()
            .map(|t| (t.id.as_str(), subtitle, 0.0))
            .collect();
        let track = |t: &'_ Track| match t.kind {
            TrackKind::Audio => audio,
            TrackKind::Video if t.id == MAIN => v1,
            TrackKind::Video => video,
        };
        let share = |t: &'_ Track| match t.kind {
            TrackKind::Audio => 1.0,
            TrackKind::Video if t.id == MAIN => 0.32,
            TrackKind::Video => 0.3,
        };
        order.extend(
            p.tracks_of(TrackKind::Video)
                .into_iter()
                .rev()
                .chain(p.tracks_of(TrackKind::Audio))
                .map(|i| &p.tracks[i])
                .map(|t| (t.id.as_str(), track(t), share(t))),
        );
        let big = order.iter().position(|t| Some(t.0) == enlarged);
        let want: Vec<f32> = order
            .iter()
            .enumerate()
            .map(|(i, &(_, h, _))| {
                if Some(i) == big {
                    (h * ENLARGE).max(ENLARGED_MIN)
                } else {
                    h
                }
            })
            .collect();
        let room = (area.height() - strip).max(1.0);
        let total: f32 = want.iter().sum();
        let kept = big.map_or(0.0, |b| want[b]);
        let factor = if total > room {
            ((room - kept) / (total - kept)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut heights: Vec<f32> = want.iter().map(|h| (h * factor).floor().max(min)).collect();
        if let Some(b) = big {
            let others: f32 = heights.iter().sum::<f32>() - heights[b];
            heights[b] = if total > room {
                (room - others).floor().max(min)
            } else {
                want[b]
            };
        }
        let used: f32 = heights.iter().sum();
        let mut y = area.bottom() + 1.0 - used + lift.clamp(0.0, (used - room).max(0.0));
        let rows = order
            .iter()
            .zip(heights)
            .map(|(&(id, _, share), h)| {
                let rect =
                    Rect::from_min_max(pos2(area.left(), y), pos2(area.right(), y + h - 1.0));
                y += h;
                Row {
                    track: id.to_string(),
                    rect: TrackRect::new(rect, share),
                }
            })
            .collect();
        Self { rows, area }
    }

    pub fn get(&self, track: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.track == track)
    }

    pub fn at(&self, y: f32) -> Option<&Row> {
        if y < self.area.top() || y > self.area.bottom() {
            return None;
        }
        self.rows
            .iter()
            .find(|r| r.rect.rect.top() <= y && y <= r.rect.rect.bottom())
    }

    pub fn used(&self) -> f32 {
        match (self.rows.first(), self.rows.last()) {
            (Some(a), Some(b)) => b.rect.rect.bottom() + 1.0 - a.rect.rect.top(),
            _ => 0.0,
        }
    }

    pub fn overflow(&self, strip: f32) -> f32 {
        (self.used() - (self.area.height() - strip).max(1.0)).max(0.0)
    }

    pub fn span(&self) -> (f32, f32) {
        (
            self.rows
                .first()
                .map_or(self.area.top(), |r| r.rect.rect.top()),
            self.rows
                .last()
                .map_or(self.area.top(), |r| r.rect.rect.bottom()),
        )
    }

    pub fn reaches(&self, pos: Pos2) -> bool {
        Rect::from_min_max(
            self.area.min - vec2(24.0, 0.0),
            self.area.max + vec2(24.0, 24.0),
        )
        .contains(pos)
    }

    pub fn target(&self, p: &Project, y: f32) -> Option<String> {
        if let Some(r) = self.at(y) {
            return (!is_subtitle(&r.track)).then(|| r.track.clone());
        }
        let (top, bottom) = self.span();
        let count = |k| p.tracks_of(k).len() + 1;
        if y < top && y >= self.area.top() - 4.0 {
            Some(format!("V{}", count(TrackKind::Video)))
        } else if y > bottom && y <= self.area.bottom() + 24.0 {
            Some(format!("A{}", count(TrackKind::Audio)))
        } else {
            None
        }
    }

    pub fn subtitle_target(&self, p: &Project, y: f32) -> String {
        match self.at(y) {
            Some(r) if is_subtitle(&r.track) => r.track.clone(),
            _ => format!("S{}", p.subtitles.len() + 1),
        }
    }

    pub fn phantom(&self, track: &str) -> TrackRect {
        let (top, bottom) = self.span();
        let rect = if track.starts_with('V') || is_subtitle(track) {
            Rect::from_min_max(
                pos2(
                    self.area.left(),
                    (top - NEW_TRACK_STRIP).max(self.area.top()),
                ),
                pos2(self.area.right(), top - 1.0),
            )
        } else {
            Rect::from_min_max(
                pos2(self.area.left(), bottom + 1.0),
                pos2(self.area.right(), bottom + NEW_TRACK_STRIP),
            )
        };
        TrackRect::new(rect, if track.starts_with('A') { 1.0 } else { 0.3 })
    }

    pub fn rect_of(&self, track: &str) -> TrackRect {
        self.get(track)
            .map_or_else(|| self.phantom(track), |r| r.rect)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Clip(String),
    Edge(String, Edge),
    Roll { outgoing: String, incoming: String },
    Slip(String),
    AudioEdit { outgoing: String, incoming: String },
    Empty,
}

const ROLL: f32 = 3.0;
const EDGE: f32 = 10.0;
const AUDIO_EDIT: f32 = 12.0;
const SLIP: f32 = 9.0;
const FADE_EDGE: f32 = 5.0;
const VOLUME_BAND: f32 = 20.0;
const VOLUME_INSET: f32 = 2.0;
const VOLUME_GRAB: f32 = 4.0;
const KEY_GRAB: f32 = 5.0;

pub fn clips_in(track: &Track, a: i64, b: i64) -> std::ops::Range<usize> {
    let first = track.clips.partition_point(|c| c.end() <= a);
    let last = track.clips.partition_point(|c| c.start < b);
    first..last.max(first)
}

fn adjacent(a: &Clip, b: &Clip) -> bool {
    a.end() == b.start
}

pub fn slip_handle(tr: &TrackRect, scale: &Scale, c: &Clip) -> Option<Rect> {
    let (x0, x1) = (scale.xf(c.start), scale.xf(c.end()));
    if x1 - x0 < 48.0 {
        return None;
    }
    let banded = tr.audio_top > tr.rect.top();
    let band_bottom = if banded {
        tr.audio_top
    } else {
        tr.rect.bottom()
    };
    let top = speed_strip(tr, scale, c).map_or(tr.rect.top(), |s| s.bottom());
    let bottom = (tr.rect.top() + (band_bottom - tr.rect.top()) * 0.45).max(top + 12.0);
    let center = (x0 + x1) / 2.0;
    Some(Rect::from_x_y_ranges(
        center - SLIP..=center + SLIP,
        top..=bottom,
    ))
}

pub fn hit(track: &Track, scale: &Scale, tr: &TrackRect, pos: Pos2) -> Hit {
    if !tr.rect.contains(pos) {
        return Hit::Empty;
    }
    let frame = scale.frame(pos.x);
    let slack = (f64::from(EDGE) / scale.ppf).ceil() as i64 + 1;
    let range = clips_in(
        track,
        frame.floor() as i64 - slack,
        frame.ceil() as i64 + slack,
    );
    let range = range.start.saturating_sub(1)..(range.end + 1).min(track.clips.len());
    let clips = &track.clips[range];
    let banded = tr.audio_top > tr.rect.top();
    let audio = banded && tr.in_audio(pos.y);

    if audio {
        for w in clips.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            if adjacent(a, b) && a.audio.is_some() && b.audio.is_some() {
                let at = a.end() + a.audio.as_ref().map_or(0, |x| x.tail);
                if (pos.x - scale.xf(at)).abs() <= AUDIO_EDIT {
                    return Hit::AudioEdit {
                        outgoing: a.id.clone(),
                        incoming: b.id.clone(),
                    };
                }
            }
        }
    }
    for w in clips.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if !adjacent(a, b) {
            continue;
        }
        let x = scale.xf(a.end());
        let edge = EDGE
            .min((scale.xf(a.end()) - scale.xf(a.start)) / 3.0)
            .max(ROLL + 1.0);
        let d = pos.x - x;
        if d.abs() <= ROLL && !audio {
            return Hit::Roll {
                outgoing: a.id.clone(),
                incoming: b.id.clone(),
            };
        }
        if -edge <= d && d < -ROLL {
            return Hit::Edge(a.id.clone(), Edge::Tail);
        }
        let edge_b = EDGE
            .min((scale.xf(b.end()) - scale.xf(b.start)) / 3.0)
            .max(ROLL + 1.0);
        if ROLL < d && d <= edge_b {
            return Hit::Edge(b.id.clone(), Edge::Head);
        }
    }
    for c in clips {
        let (x0, x1) = (scale.xf(c.start), scale.xf(c.end()));
        if pos.x < x0 - ROLL || pos.x > x1 + ROLL {
            continue;
        }
        let edge = EDGE.min((x1 - x0) / 3.0);
        if pos.x - x0 <= edge && pos.x >= x0 - ROLL {
            return Hit::Edge(c.id.clone(), Edge::Head);
        }
        if x1 - pos.x <= edge && pos.x <= x1 + ROLL {
            return Hit::Edge(c.id.clone(), Edge::Tail);
        }
        if pos.x < x0 || pos.x >= x1 {
            continue;
        }
        if speed_labels(tr, scale, c)
            .iter()
            .any(|l| l.rect.contains(pos))
        {
            return Hit::Clip(c.id.clone());
        }
        if slip_handle(tr, scale, c).is_some_and(|h| h.contains(pos)) {
            return Hit::Slip(c.id.clone());
        }
        return Hit::Clip(c.id.clone());
    }
    Hit::Empty
}

pub fn sound_band(tr: &TrackRect, scale: &Scale, c: &Clip) -> Option<Rect> {
    let (ra, rb) = c.audio_range()?;
    let (a0, a1) = (scale.xf(ra), scale.xf(rb).max(scale.xf(ra) + 1.0));
    Some(Rect::from_x_y_ranges(
        a0..=a1,
        tr.audio_top..=tr.rect.bottom(),
    ))
}

pub const SPEED_STRIP: f32 = 16.0;
const SPEED_LABEL: f32 = 52.0;

pub struct SpeedLabel {
    pub at: i64,
    pub speed: f64,
    pub rect: Rect,
}

pub fn speed_strip(tr: &TrackRect, scale: &Scale, c: &Clip) -> Option<Rect> {
    c.speed.as_ref().filter(|s| s.sections.len() > 1)?;
    let band = if c.video {
        let (x0, x1) = (scale.xf(c.start), scale.xf(c.end()));
        Rect::from_x_y_ranges(x0..=x1, tr.rect.top()..=tr.audio_top - 1.0)
    } else {
        sound_band(tr, scale, c)?
    };
    (band.height() >= 2.0 * SPEED_STRIP)
        .then(|| Rect::from_x_y_ranges(band.x_range(), band.top()..=band.top() + SPEED_STRIP))
}

pub fn speed_keyframes(c: &Clip) -> Vec<i64> {
    c.speed.as_ref().map_or(Vec::new(), |s| {
        s.keyframes()
            .iter()
            .map(|k| c.start + k.frame - s.skip)
            .filter(|t| (c.start + 1..c.end()).contains(t))
            .collect()
    })
}

pub fn speed_labels(tr: &TrackRect, scale: &Scale, c: &Clip) -> Vec<SpeedLabel> {
    let (Some(strip), Some(s)) = (speed_strip(tr, scale, c), c.speed.as_ref()) else {
        return Vec::new();
    };
    let (left, right) = (
        strip.left().max(tr.rect.left()),
        strip.right().min(tr.rect.right()),
    );
    let frame = |at: i64| (c.start + at - s.skip).clamp(c.start, c.end());
    let map = &s.sections;
    map.iter()
        .enumerate()
        .filter_map(|(i, x)| {
            let a = if i == 0 { c.start } else { frame(x.at) };
            let b = map.get(i + 1).map_or(c.end(), |n| frame(n.at));
            let (l, r) = (scale.xf(a).max(left), scale.xf(b).min(right));
            (b > a && r - l >= SPEED_LABEL + 4.0).then(|| SpeedLabel {
                at: a,
                speed: x.speed,
                rect: Rect::from_center_size(
                    pos2((l + r) / 2.0, strip.center().y),
                    vec2(SPEED_LABEL, SPEED_STRIP),
                ),
            })
        })
        .collect()
}

pub fn speed_label_at(
    p: &Project,
    rows: &Rows,
    scale: &Scale,
    pos: Pos2,
) -> Option<(String, SpeedLabel)> {
    let r = rows.at(pos.y)?;
    let t = p.track(&r.track)?;
    let f = scale.under(pos.x);
    let c = t.clips[clips_in(t, f, f + 1)].first()?;
    speed_labels(&r.rect, scale, c)
        .into_iter()
        .find(|l| l.rect.contains(pos))
        .map(|l| (c.id.clone(), l))
}

pub fn volume_level(gain_db: f64) -> f64 {
    let (lo, hi) = (*GAIN.start(), *GAIN.end());
    let zero = lo / (lo - hi);
    let g = gain_db.clamp(lo, hi);
    let (x, room) = if g >= 0.0 {
        (g / hi, 1.0 - zero)
    } else {
        (-g / lo, zero)
    };
    zero + room * x * (2.0 - x.abs())
}

pub fn volume_gain(level: f64) -> f64 {
    let (lo, hi) = (*GAIN.start(), *GAIN.end());
    let zero = lo / (lo - hi);
    let level = level.clamp(0.0, 1.0);
    let (part, end) = if level >= zero {
        ((level - zero) / (1.0 - zero), hi)
    } else {
        ((zero - level) / zero, lo)
    };
    end * (1.0 - (1.0 - part).max(0.0).sqrt())
}

pub fn volume_span(band: Rect) -> f32 {
    (band.height() - 2.0 * VOLUME_INSET).max(1.0)
}

pub fn level_at(band: Rect, y: f32) -> f64 {
    f64::from((band.bottom() - VOLUME_INSET - y) / volume_span(band)).clamp(0.0, 1.0)
}

pub fn volume_y(band: Rect, gain_db: f64) -> Option<f32> {
    (band.height() >= VOLUME_BAND)
        .then(|| band.bottom() - VOLUME_INSET - volume_span(band) * volume_level(gain_db) as f32)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OnVolume {
    Line,
    Key(i64),
    Handle(i64, bool),
}

pub fn curve_y(band: Rect, c: &Clip, t: f64) -> Option<f32> {
    volume_y(band, c.gain_at(t)?)
}

pub fn key_point(band: Rect, scale: &Scale, c: &Clip, k: &Keyframe) -> Option<Pos2> {
    let x = scale.x(c.key_time(k.frame));
    band.x_range()
        .contains(x)
        .then(|| Some(pos2(x, volume_y(band, k.value)?)))
        .flatten()
}

pub fn handle_point(band: Rect, scale: &Scale, c: &Clip, i: usize, out: bool) -> Option<Pos2> {
    let keys = &c.audio.as_ref()?.gain_keys;
    let (a, b) = if out {
        (keys.get(i)?, keys.get(i + 1)?)
    } else {
        (keys.get(i.checked_sub(1)?)?, keys.get(i)?)
    };
    let (frame, value) = vidcut_core::keyframes::handle_point(a, b, out)?;
    let x = scale.x(c.time_of(frame));
    let y = volume_y(band, value.clamp(*GAIN.start(), *GAIN.end()))?;
    band.x_range().contains(x).then_some(pos2(x, y))
}

pub fn chosen_on(chosen: &[(String, i64)], c: &Clip, frame: i64) -> bool {
    chosen.iter().any(|(id, f)| *id == c.id && *f == frame)
}

pub fn volume_target(
    track: &Track,
    scale: &Scale,
    tr: &TrackRect,
    pos: Pos2,
    chosen: &[(String, i64)],
) -> Option<(String, Rect, OnVolume)> {
    if track.locked {
        return None;
    }
    let frame = scale.under(pos.x);
    let range = clips_in(track, frame - 1, frame + 2);
    let range = range.start.saturating_sub(1)..(range.end + 1).min(track.clips.len());
    let clips = &track.clips[range];
    let near = |q: Pos2| (q - pos).length() <= KEY_GRAB;
    for c in clips {
        let Some(band) = sound_band(tr, scale, c).filter(|b| b.height() >= VOLUME_BAND) else {
            continue;
        };
        let keys = c.audio.as_ref().map_or(&[][..], |a| &a.gain_keys[..]);
        for (i, k) in keys.iter().enumerate() {
            if !chosen_on(chosen, c, k.frame) {
                continue;
            }
            for out in [false, true] {
                if handle_point(band, scale, c, i, out).is_some_and(near) {
                    return Some((c.id.clone(), band, OnVolume::Handle(k.frame, out)));
                }
            }
        }
    }
    if !matches!(hit(track, scale, tr, pos), Hit::Clip(_) | Hit::Slip(_)) {
        return None;
    }
    clips
        .iter()
        .filter_map(|c| {
            let band = sound_band(tr, scale, c).filter(|b| b.contains(pos))?;
            let a = c.audio.as_ref()?;
            if let Some(k) = a
                .gain_keys
                .iter()
                .find(|k| key_point(band, scale, c, k).is_some_and(near))
            {
                return Some((0.0, c.id.clone(), band, OnVolume::Key(k.frame)));
            }
            let y = curve_y(band, c, scale.frame(pos.x))?;
            let off = (pos.y - y).abs();
            (off <= VOLUME_GRAB).then(|| (off, c.id.clone(), band, OnVolume::Line))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id, band, on)| (id, band, on))
}

pub fn picture_transition<'a>(p: &Project, track: &'a Track, at: i64) -> Option<&'a Transition> {
    let cuts = Cuts::of(p, track);
    track
        .transitions
        .iter()
        .find(|x| !x.sound && cuts.place(p, x) == Some(at))
}

pub fn fade_edge_at(
    p: &Project,
    track: &Track,
    scale: &Scale,
    tr: &TrackRect,
    pos: Pos2,
) -> Option<(i64, bool)> {
    let banded = tr.audio_top > tr.rect.top();
    if !tr.rect.contains(pos) || (banded && tr.in_audio(pos.y)) {
        return None;
    }
    let cuts = Cuts::of(p, track);
    track.transitions.iter().filter(|x| !x.sound).find_map(|x| {
        let at = cuts.place(p, x)?;
        if (pos.x - scale.xf(at)).abs() <= ROLL {
            return None;
        }
        let near = |f: i64| f != at && (scale.xf(f) - pos.x).abs() <= FADE_EDGE;
        if near(at + x.after) {
            Some((at, true))
        } else if near(at - x.before) {
            Some((at, false))
        } else {
            None
        }
    })
}

pub fn transition_at(
    p: &Project,
    track: &Track,
    scale: &Scale,
    tr: &TrackRect,
    pos: Pos2,
) -> Option<i64> {
    let banded = tr.audio_top > tr.rect.top();
    if !tr.rect.contains(pos) || (banded && tr.in_audio(pos.y)) {
        return None;
    }
    let cuts = Cuts::of(p, track);
    track.transitions.iter().filter(|x| !x.sound).find_map(|x| {
        let at = cuts.place(p, x)?;
        let over = scale.xf(at - x.before) <= pos.x && pos.x <= scale.xf(at + x.after);
        (over && (pos.x - scale.xf(at)).abs() > ROLL).then_some(at)
    })
}

pub fn hit_rows(p: &Project, rows: &Rows, scale: &Scale, pos: Pos2) -> (Option<String>, Hit) {
    match rows.at(pos.y) {
        Some(r) => {
            let what = p
                .track(&r.track)
                .map_or(Hit::Empty, |t| hit(t, scale, &r.rect, pos));
            (Some(r.track.clone()), what)
        }
        None => (None, Hit::Empty),
    }
}

pub fn edit_near(track: &Track, scale: &Scale, x: f32, px: f32) -> Option<i64> {
    let mut points = vec![0];
    if let Some(last) = track.clips.last() {
        points.push(last.end());
    }
    let frame = scale.frame(x);
    let slack = (f64::from(px) / scale.ppf).ceil() as i64 + 1;
    let range = clips_in(
        track,
        frame.floor() as i64 - slack,
        frame.ceil() as i64 + slack,
    );
    let clips = &track.clips[range];
    for w in clips.windows(2) {
        if adjacent(&w[0], &w[1]) {
            points.push(w[0].end());
        }
    }
    points
        .into_iter()
        .filter(|&f| (scale.xf(f) - x).abs() <= px)
        .min_by(|a, b| {
            (scale.xf(*a) - x)
                .abs()
                .total_cmp(&(scale.xf(*b) - x).abs())
        })
}

#[derive(Debug, Clone, Default)]
pub struct Snap {
    points: Vec<i64>,
}

pub const SNAP_PX: f32 = 8.0;

impl Snap {
    pub fn new(p: &Project, playhead: Option<i64>, moving: &[String]) -> Self {
        let mut points: Vec<i64> = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| !moving.contains(&c.id))
            .flat_map(|c| [c.start, c.end()])
            .chain(
                p.subtitles
                    .iter()
                    .flat_map(|t| t.cues.iter())
                    .filter(|c| !moving.contains(&c.id))
                    .flat_map(|c| [c.start, c.end()]),
            )
            .chain(p.markers.iter().map(|k| k.at))
            .chain(p.marks.mark_in)
            .chain(p.marks.mark_out)
            .chain(playhead)
            .chain(std::iter::once(0))
            .collect();
        points.sort_unstable();
        points.dedup();
        Self { points }
    }

    pub fn snap(&self, frame: i64, scale: &Scale, px: f32) -> Option<i64> {
        let i = self.points.partition_point(|&p| p < frame);
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|i| self.points.get(i).copied())
            .filter(|&p| ((p - frame) as f64 * scale.ppf).abs() <= f64::from(px))
            .min_by_key(|&p| (p - frame).abs())
    }

    pub fn snap_block(&self, start: i64, len: i64, scale: &Scale, px: f32) -> (i64, Option<i64>) {
        let head = self.snap(start, scale, px).map(|t| (t - start, t));
        let tail = self
            .snap(start + len, scale, px)
            .map(|t| (t - start - len, t));
        match (head, tail) {
            (Some(h), Some(t)) if t.0.abs() < h.0.abs() => (start + t.0, Some(t.1)),
            (Some(h), _) => (start + h.0, Some(h.1)),
            (None, Some(t)) => (start + t.0, Some(t.1)),
            (None, None) => (start, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{pos2, vec2};
    use vidcut_core::model::{ClipAudio, TrackKind};

    use super::*;

    fn clip(id: &str, start: i64, duration: i64, audio: Option<(i64, i64)>) -> Clip {
        Clip {
            id: id.into(),
            media: "m1".into(),
            start,
            duration,
            source_in: 0,
            video: true,
            audio: audio.map(|(head, tail)| ClipAudio {
                stream: 0,
                head,
                tail,
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

    const THIN: Scale = Scale {
        origin: 0.0,
        ppf: 0.1,
    };

    #[test]
    fn a_thin_clip_is_covered_by_the_next_in_its_pixel() {
        assert!(covered(&THIN, (0, 5), Some((5, 9))));
    }

    #[test]
    fn the_last_thin_clip_in_its_pixel_is_drawn() {
        assert!(!covered(&THIN, (5, 10), Some((10, 15))));
    }

    #[test]
    fn a_clip_a_pixel_wide_is_drawn() {
        assert!(!covered(&THIN, (0, 10), Some((10, 15))));
    }

    fn track() -> Track {
        Track {
            clips: vec![
                clip("c1", 0, 100, Some((0, 10))),
                clip("c2", 100, 50, Some((10, 0))),
                clip("c3", 200, 100, None),
            ],
            ..Track::new("V1", TrackKind::Video)
        }
    }

    fn setup() -> (Track, Scale, TrackRect) {
        let tr = TrackRect::new(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 100.0)),
            0.3,
        );
        (track(), Scale::fit(0.0, 1000.0, 500), tr)
    }

    #[test]
    fn edit_points_near_the_pointer() {
        let (t, s, _) = setup();
        assert_eq!(edit_near(&t, &s, 203.0, 6.0), Some(100));
        assert_eq!(edit_near(&t, &s, 2.0, 6.0), Some(0));
        assert_eq!(edit_near(&t, &s, 596.0, 6.0), Some(300));
        assert_eq!(edit_near(&t, &s, 300.0, 6.0), None);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderParts {
    pub enlarge: Rect,
    pub name: Rect,
    pub toggles: [Rect; 3],
}

const HEADER_TOGGLE: f32 = 17.0;
const HEADER_GAP: f32 = 2.0;
const HEADER_LINE: f32 = 18.0;

pub fn header_parts(header: Rect) -> HeaderParts {
    let h = HEADER_LINE.min(header.height() - 2.0).max(1.0);
    let top = (header.center().y - h / 2.0).round();
    let right = header.right().round();
    let toggle = |i: u8| {
        let end = right - f32::from(2 - i) * (HEADER_TOGGLE + HEADER_GAP);
        Rect::from_min_max(pos2(end - HEADER_TOGGLE, top), pos2(end, top + h))
    };
    let toggles = [toggle(0), toggle(1), toggle(2)];
    let left = header.left().round();
    let enlarge = Rect::from_min_max(pos2(left, top), pos2(left + HEADER_TOGGLE, top + h));
    HeaderParts {
        enlarge,
        name: Rect::from_min_max(
            pos2(enlarge.right() + 2.0 * HEADER_GAP, top),
            pos2(toggles[0].left() - 2.0 * HEADER_GAP, top + h),
        ),
        toggles,
    }
}
