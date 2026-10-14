//! Timeline geometry: frames ↔ pixels, what the pointer is over, snapping.
//! Pure functions over the project, testable without a window.

use eframe::egui::{Pos2, Rect, pos2, vec2};
use vidcut_core::model::{Clip, Project, Track, TrackKind};
use vidcut_core::ops::{Edge, MAIN};

/// Maps timeline frames to screen x and back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    /// Screen x of frame 0.
    pub origin: f64,
    /// Pixels per frame.
    pub ppf: f64,
}

impl Scale {
    /// `span` frames across `width` pixels from `left`.
    pub fn fit(left: f32, width: f32, span: i64) -> Self {
        Self {
            origin: f64::from(left),
            ppf: f64::from(width.max(1.0)) / span.max(1) as f64,
        }
    }

    /// `ppf` pixels per frame with `frame` at screen x `x`.
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

    /// The frame whose left edge is nearest `x` (edit points sit on frame
    /// boundaries).
    pub fn boundary(&self, x: f32) -> i64 {
        self.frame(x).round() as i64
    }

    /// The frame under `x` (playhead positions).
    pub fn under(&self, x: f32) -> i64 {
        self.frame(x).floor() as i64
    }

    /// The frames visible between `left` and `right`.
    pub fn visible(&self, left: f32, right: f32) -> (i64, i64) {
        (
            self.frame(left).floor() as i64,
            self.frame(right).ceil() as i64,
        )
    }
}

/// Where a track is drawn: the video band above the audio band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackRect {
    pub rect: Rect,
    /// Where the audio band starts.
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

/// A track's row on a timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub track: String,
    pub rect: TrackRect,
}

/// A timeline's rows, top to bottom: the video tracks from the top one down
/// to V1, then the audio tracks A1, A2…. Above the rows is room for
/// a new video track, below them for a new audio track. The rows sit on the
/// bottom, so the last one stays where it is whatever the area's height.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows {
    pub rows: Vec<Row>,
    /// Where the rows were laid out.
    pub area: Rect,
}

/// Height of the strip above the rows where a drop makes a new video track,
/// and of the one drawn below them while a drop there makes a new audio
/// track.
pub const NEW_TRACK_STRIP: f32 = 14.0;

/// An enlarged track's height: this many times its usual one, and at least
/// [`ENLARGED_MIN`] pixels (decision D43).
const ENLARGE: f32 = 3.0;
const ENLARGED_MIN: f32 = 180.0;

impl Rows {
    /// Lays out `p`'s tracks in `area`, the last on its bottom and the first
    /// at least `strip` below its top (a new video track); what's spare goes
    /// above them. A new audio track is dropped just below the area. Track 1
    /// gets the most room; rows shrink (to `min` pixels) when many tracks
    /// share the area. The `enlarged` track is three times its height, and
    /// the others give way to it first.
    pub fn layout(
        p: &Project,
        area: Rect,
        heights: [f32; 3],
        min: f32,
        strip: f32,
        enlarged: Option<&str>,
    ) -> Self {
        let [v1, video, audio] = heights;
        let mut order: Vec<&Track> = p
            .tracks_of(TrackKind::Video)
            .into_iter()
            .rev()
            .map(|i| &p.tracks[i])
            .collect();
        order.extend(
            p.tracks_of(TrackKind::Audio)
                .into_iter()
                .map(|i| &p.tracks[i]),
        );
        let big = order.iter().position(|t| Some(t.id.as_str()) == enlarged);
        let want: Vec<f32> = order
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let h = match t.kind {
                    TrackKind::Audio => audio,
                    TrackKind::Video if t.id == MAIN => v1,
                    TrackKind::Video => video,
                };
                if Some(i) == big {
                    (h * ENLARGE).max(ENLARGED_MIN)
                } else {
                    h
                }
            })
            .collect();
        let room = (area.height() - strip).max(1.0);
        let total: f32 = want.iter().sum();
        // The others shrink first, to make room for the enlarged track.
        let kept = big.map_or(0.0, |b| want[b]);
        let factor = if total > room {
            ((room - kept) / (total - kept)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Rounded down, so shrunk rows never reach past the bottom.
        let mut heights: Vec<f32> = want.iter().map(|h| (h * factor).floor().max(min)).collect();
        if let Some(b) = big {
            // What the others leave, while they fit; then its own height.
            let others: f32 = heights.iter().sum::<f32>() - heights[b];
            heights[b] = if total > room {
                (room - others).floor().max(min)
            } else {
                want[b]
            };
        }
        let used: f32 = heights.iter().sum();
        let mut y = (area.bottom() + 1.0 - used).max(area.top() + strip);
        let rows = order
            .iter()
            .zip(heights)
            .map(|(t, h)| {
                let rect =
                    Rect::from_min_max(pos2(area.left(), y), pos2(area.right(), y + h - 1.0));
                y += h;
                let share = match t.kind {
                    TrackKind::Audio => 1.0,
                    TrackKind::Video if t.id == MAIN => 0.32,
                    TrackKind::Video => 0.3,
                };
                Row {
                    track: t.id.clone(),
                    rect: TrackRect::new(rect, share),
                }
            })
            .collect();
        Self { rows, area }
    }

    pub fn get(&self, track: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.track == track)
    }

    /// The row under `y`.
    pub fn at(&self, y: f32) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.rect.rect.top() <= y && y <= r.rect.rect.bottom())
    }

    /// The top of the first row and the bottom of the last.
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

    /// Whether a drag at `pos` is still on the timeline: over the area, or
    /// up to 24 px beside it or below it (a new audio track; decision D18).
    pub fn reaches(&self, pos: Pos2) -> bool {
        Rect::from_min_max(
            self.area.min - vec2(24.0, 0.0),
            self.area.max + vec2(24.0, 24.0),
        )
        .contains(pos)
    }

    /// The track a drop at `y` goes to: the row's track, or a new video
    /// track above the rows, or a new audio track below them.
    pub fn target(&self, p: &Project, y: f32) -> Option<String> {
        if let Some(r) = self.at(y) {
            return Some(r.track.clone());
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

    /// Where a track that doesn't exist yet would be drawn: the strip above
    /// the rows for a video track, a strip below them, outside the area, for
    /// an audio track.
    pub fn phantom(&self, track: &str) -> TrackRect {
        let (top, bottom) = self.span();
        let rect = if track.starts_with('V') {
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

    /// The row for `track`, real or phantom.
    pub fn rect_of(&self, track: &str) -> TrackRect {
        self.get(track)
            .map_or_else(|| self.phantom(track), |r| r.rect)
    }
}

/// What the pointer is over on a track (spec §8.1, §8.6: the pointer
/// position picks the operation).
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    /// A clip's body: select, move.
    Clip(String),
    /// A clip edge: resize.
    Edge(String, Edge),
    /// The center of an edit between two adjacent clips: roll.
    Roll { outgoing: String, incoming: String },
    /// A clip's slip handle (center): slip.
    Slip(String),
    /// The audio edit between two adjacent clips with audio: J/L cut.
    AudioEdit { outgoing: String, incoming: String },
    /// Nothing.
    Empty,
}

/// Pixels on each side of an edit's center that roll it.
const ROLL: f32 = 3.0;
/// Pixels from a clip edge that resize it.
const EDGE: f32 = 10.0;
/// Pixels from the audio edit that make a J/L cut. Wider than an edge's:
/// nothing marks the audio edit until the pointer is near it.
const AUDIO_EDIT: f32 = 12.0;
/// Half the slip handle's width.
const SLIP: f32 = 9.0;

/// The clips of `track` that overlap frames `[a, b)`, by index.
pub fn clips_in(track: &Track, a: i64, b: i64) -> std::ops::Range<usize> {
    let first = track.clips.partition_point(|c| c.end() <= a);
    let last = track.clips.partition_point(|c| c.start < b);
    first..last.max(first)
}

fn adjacent(a: &Clip, b: &Clip) -> bool {
    a.end() == b.start
}

/// What is under `pos` on `track`.
/// Where the slip handle of a clip drawn from `x0` to `x1` is: at its top
/// center, over the upper part of the picture; none on a clip too narrow
/// to grab it.
pub fn slip_handle(tr: &TrackRect, x0: f32, x1: f32) -> Option<Rect> {
    if x1 - x0 < 48.0 {
        return None;
    }
    let banded = tr.audio_top > tr.rect.top();
    let band_bottom = if banded {
        tr.audio_top
    } else {
        tr.rect.bottom()
    };
    let bottom = tr.rect.top() + (band_bottom - tr.rect.top()) * 0.45;
    let center = (x0 + x1) / 2.0;
    Some(Rect::from_x_y_ranges(
        center - SLIP..=center + SLIP,
        tr.rect.top()..=bottom,
    ))
}

pub fn hit(track: &Track, scale: &Scale, tr: &TrackRect, pos: Pos2) -> Hit {
    if !tr.rect.contains(pos) {
        return Hit::Empty;
    }
    // On audio tracks the whole row is the audio band.
    let frame = scale.frame(pos.x);
    // Clips near the pointer (edges may be a few pixels outside a clip).
    let slack = (f64::from(EDGE) / scale.ppf).ceil() as i64 + 1;
    let range = clips_in(
        track,
        frame.floor() as i64 - slack,
        frame.ceil() as i64 + slack,
    );
    // One more clip on each side: a J/L cut puts an audio edit inside the
    // neighbor.
    let range = range.start.saturating_sub(1)..(range.end + 1).min(track.clips.len());
    let clips = &track.clips[range];
    // Rows with a picture band above the sound band (video tracks); on audio
    // rows the whole row is sound and works like a picture band.
    let banded = tr.audio_top > tr.rect.top();
    let audio = banded && tr.in_audio(pos.y);

    // The audio band at an edit with a J/L cut moves only the audio edit.
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
    // Edit points between adjacent clips: center rolls, sides resize.
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
        if slip_handle(tr, x0, x1).is_some_and(|h| h.contains(pos)) {
            return Hit::Slip(c.id.clone());
        }
        return Hit::Clip(c.id.clone());
    }
    Hit::Empty
}

/// What is under `pos` on whichever row it is over: the track and the hit.
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

/// Where an edit point is near `x` (within `px`): the boundary between two
/// adjacent clips, the start of the first clip, or the end of the last.
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

/// Snap targets (spec §8.8): clip edges, markers, the timeline marks and the
/// playhead (not when it is fixed: clips don't snap to a fixed playhead).
#[derive(Debug, Clone, Default)]
pub struct Snap {
    points: Vec<i64>,
}

/// Snap distance in pixels.
pub const SNAP_PX: f32 = 8.0;

impl Snap {
    /// Targets in `p`, leaving out the edges of `moving` clips.
    pub fn new(p: &Project, playhead: Option<i64>, moving: &[String]) -> Self {
        let mut points: Vec<i64> = p
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| !moving.contains(&c.id))
            .flat_map(|c| [c.start, c.end()])
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

    /// The target nearest `frame` within `px` pixels.
    pub fn snap(&self, frame: i64, scale: &Scale, px: f32) -> Option<i64> {
        let i = self.points.partition_point(|&p| p < frame);
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|i| self.points.get(i).copied())
            .filter(|&p| ((p - frame) as f64 * scale.ppf).abs() <= f64::from(px))
            .min_by_key(|&p| (p - frame).abs())
    }

    /// A block `[start, start + len)` snapped by whichever edge is closer
    /// to a target: the adjusted start and the frame snapped to.
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
                muted: false,
                effects: None,
            }),
            enabled: true,
            name: None,
            effects: None,
            generator: None,
            compound: None,
            speed: None,
        }
    }

    fn track() -> Track {
        // c1 0..100 | c2 100..150 (L-cut: c1's audio runs 10 frames into c2),
        // gap, c3 200..300.
        Track {
            clips: vec![
                clip("c1", 0, 100, Some((0, 10))),
                clip("c2", 100, 50, Some((10, 0))),
                clip("c3", 200, 100, None),
            ],
            ..Track::new("V1", TrackKind::Video)
        }
    }

    /// 2 px per frame from x = 0; the track spans y 0..100, audio 70..100.
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
        // c2's end (150) is next to a gap, not an edit between clips.
        assert_eq!(edit_near(&t, &s, 300.0, 6.0), None);
    }
}

/// Where a track header's parts go, on one line centered in the row: the
/// enlarge button on the left, the name, then the lock, audio and video
/// buttons in fixed columns ending at the header's right edge (spec §5).
/// Every row puts each button in the same column, whatever its height or
/// kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderParts {
    pub enlarge: Rect,
    pub name: Rect,
    /// Lock, audio and video, left to right.
    pub toggles: [Rect; 3],
}

/// A header button's width, and the space between two.
const HEADER_TOGGLE: f32 = 17.0;
const HEADER_GAP: f32 = 2.0;
/// The height of a header's line, where the row has room for it.
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
