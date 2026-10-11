//! Timeline geometry: frames ↔ pixels, what the pointer is over, snapping.
//! Pure functions over the project, testable without a window.

use eframe::egui::{Pos2, Rect};
use vidcut_core::model::{Clip, Project, Track};
use vidcut_core::ops::Edge;

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
/// Pixels from the audio edit that make a J/L cut.
const AUDIO_EDIT: f32 = 7.0;
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
pub fn hit(track: &Track, scale: &Scale, tr: &TrackRect, pos: Pos2) -> Hit {
    if !tr.rect.contains(pos) {
        return Hit::Empty;
    }
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
    let audio = tr.in_audio(pos.y);

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
        let center = (x0 + x1) / 2.0;
        let handle_bottom = tr.rect.top() + (tr.audio_top - tr.rect.top()) * 0.45;
        if x1 - x0 >= 48.0 && (pos.x - center).abs() <= SLIP && pos.y <= handle_bottom {
            return Hit::Slip(c.id.clone());
        }
        return Hit::Clip(c.id.clone());
    }
    Hit::Empty
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
/// playhead (not when it is fixed: clips don't snap to a fixed playhead,
/// p. 717).
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
            }),
            enabled: true,
            name: None,
        }
    }

    fn track() -> Track {
        // c1 0..100 | c2 100..150 (L-cut: c1's audio runs 10 frames into c2),
        // gap, c3 200..300.
        Track {
            id: "V1".into(),
            kind: TrackKind::Video,
            clips: vec![
                clip("c1", 0, 100, Some((0, 10))),
                clip("c2", 100, 50, Some((10, 0))),
                clip("c3", 200, 100, None),
            ],
            locked: false,
            muted: false,
            video_enabled: true,
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
    fn scale_round_trips() {
        let s = Scale::anchored(500.0, 1000.0, 4.0);
        assert_eq!(s.x(1000.0), 500.0);
        assert_eq!(s.boundary(501.9), 1000);
        assert_eq!(s.under(503.9), 1000);
        assert_eq!(s.under(504.0), 1001);
        assert_eq!(s.visible(0.0, 1000.0), (875, 1125));
    }

    #[test]
    fn the_pointer_position_picks_the_operation() {
        // Spec §8.6, p. 726–728.
        let (t, s, tr) = setup();
        let at = |x: f32, y: f32| hit(&t, &s, &tr, pos2(x, y));
        // The edit between c1 and c2 is at x = 200.
        assert_eq!(
            at(200.0, 30.0),
            Hit::Roll {
                outgoing: "c1".into(),
                incoming: "c2".into()
            }
        );
        assert_eq!(at(195.0, 30.0), Hit::Edge("c1".into(), Edge::Tail));
        assert_eq!(at(206.0, 30.0), Hit::Edge("c2".into(), Edge::Head));
        // The audio edit sits 10 frames later (x = 220), in the audio band.
        assert_eq!(
            at(221.0, 85.0),
            Hit::AudioEdit {
                outgoing: "c1".into(),
                incoming: "c2".into()
            }
        );
        // Clip bodies, the slip handle at the top center, gaps.
        assert_eq!(at(60.0, 50.0), Hit::Clip("c1".into()));
        assert_eq!(at(100.0, 10.0), Hit::Slip("c1".into()));
        assert_eq!(at(100.0, 60.0), Hit::Clip("c1".into()));
        assert_eq!(at(350.0, 30.0), Hit::Empty);
        // Edges next to a gap resize; the timeline start too.
        assert_eq!(at(298.0, 30.0), Hit::Edge("c2".into(), Edge::Tail));
        assert_eq!(at(403.0, 30.0), Hit::Edge("c3".into(), Edge::Head));
        assert_eq!(at(1.0, 30.0), Hit::Edge("c1".into(), Edge::Head));
        assert_eq!(at(599.0, 30.0), Hit::Edge("c3".into(), Edge::Tail));
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

    #[test]
    fn snapping_takes_the_closer_edge() {
        // Spec §8.8: edges, markers, marks and the playhead attract.
        let s = Snap {
            points: vec![0, 100, 150, 200, 300],
        };
        let scale = Scale::fit(0.0, 1000.0, 500); // 2 px per frame
        assert_eq!(s.snap(103, &scale, 8.0), Some(100));
        assert_eq!(s.snap(106, &scale, 8.0), None);
        // A 50-frame block starting at 147: its head is 3 frames from 150,
        // its tail (197) 3 frames from 200; ties go to the head.
        assert_eq!(s.snap_block(147, 50, &scale, 8.0), (150, Some(150)));
        assert_eq!(s.snap_block(149, 49, &scale, 8.0), (150, Some(150)));
        assert_eq!(s.snap_block(122, 80, &scale, 8.0), (120, Some(200)));
    }
}
