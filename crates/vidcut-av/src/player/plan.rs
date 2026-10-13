//! A render plan as the player reads it: which picture each timeline frame
//! shows, and where frames and samples meet.

use std::sync::Arc;

use vidcut_core::render::{Image, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

/// What a picture shows, wherever it is on a timeline: the same source
/// position of the same file at one rate, or black of one size. Pictures
/// with equal ids are equal, so kept pictures survive edits that move them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Id {
    source: Arc<str>,
    pos: i64,
}

#[cfg(test)]
impl Id {
    /// Position `pos` of `source`, for tests.
    pub(crate) fn of(source: &str, pos: i64) -> Self {
        Self {
            source: source.into(),
            pos,
        }
    }
}

/// A plan, indexed.
pub(crate) struct Plan {
    pub(crate) plan: RenderPlan,
    /// The first frame of each video piece, counted from the plan's start.
    starts: Vec<i64>,
    /// What each video piece shows, apart from where in it.
    sources: Vec<Arc<str>>,
    /// Samples (per channel) the plan lasts.
    pub(crate) samples: i64,
}

impl Plan {
    pub(crate) fn new(plan: RenderPlan) -> Self {
        let mut starts = Vec::with_capacity(plan.video.len());
        let mut t = 0;
        let sources = plan
            .video
            .iter()
            .map(|piece| {
                starts.push(t);
                t += piece.frames();
                source_key(piece, &plan)
            })
            .collect();
        let samples = plan.samples();
        Self {
            plan,
            starts,
            sources,
            samples,
        }
    }

    pub(crate) fn fps(&self) -> Rational {
        self.plan.fps
    }

    pub(crate) fn rate(&self) -> u32 {
        self.plan.sample_rate
    }

    /// The plan's first timeline frame.
    pub(crate) fn first(&self) -> i64 {
        self.plan.range.0
    }

    /// The plan's last timeline frame.
    pub(crate) fn last(&self) -> i64 {
        self.plan.range.0 + (self.plan.frames - 1).max(0)
    }

    pub(crate) fn clamp(&self, t: i64) -> i64 {
        t.clamp(self.first(), self.last())
    }

    /// The video piece showing timeline frame `t` (clamped to the plan),
    /// and `t`'s frame in it.
    fn locate(&self, t: i64) -> (usize, i64) {
        let rel = self.clamp(t) - self.first();
        let i = self.starts.partition_point(|&s| s <= rel).saturating_sub(1);
        (i, rel - self.starts.get(i).copied().unwrap_or(0))
    }

    /// The picture timeline frame `t` shows.
    pub(crate) fn id(&self, t: i64) -> Id {
        let (i, k) = self.locate(t);
        let pos = match self.plan.video.get(i) {
            Some(VideoPiece::Source {
                source_frame,
                image,
                ..
            }) if !matches!(image, Some(Image::Still)) => source_frame + k,
            // Every frame of black, or of a still, is one picture.
            _ => 0,
        };
        let source = self.sources.get(i).cloned().unwrap_or_else(|| "".into());
        Id { source, pos }
    }

    /// Timeline frame `t` shows a picture of a file (not black).
    pub(crate) fn shows_a_file(&self, t: i64) -> bool {
        let (i, _) = self.locate(t);
        matches!(self.plan.video.get(i), Some(VideoPiece::Source { .. }))
    }

    /// The plan's video from timeline frame `t` on.
    pub(crate) fn pieces_from(&self, t: i64) -> Vec<VideoPiece> {
        let (i, k) = self.locate(t);
        let mut pieces = self.plan.video[i.min(self.plan.video.len())..].to_vec();
        if let Some(first) = pieces.first_mut() {
            *first = cut(first, k);
        }
        pieces
    }

    /// Timeline frame `t`'s piece, cut to start at `t`, and the timeline
    /// frame that piece starts at.
    pub(crate) fn piece_at(&self, t: i64) -> Option<(VideoPiece, i64)> {
        let (i, k) = self.locate(t);
        let piece = self.plan.video.get(i)?;
        Some((cut(piece, k), self.clamp(t) - k))
    }

    /// The first sample of timeline frame `t`, counted from the plan's
    /// start.
    pub(crate) fn sample(&self, t: i64) -> i64 {
        self.fps().frame_to_sample(t - self.first(), self.rate())
    }

    /// The timeline frame whose samples hold sample `s` (counted from the
    /// plan's start), clamped to the plan.
    pub(crate) fn frame_at_sample(&self, s: i64) -> i64 {
        self.clamp(self.first() + frame_of(self.fps(), s, self.rate()))
    }
}

/// `piece` from its frame `k` on.
fn cut(piece: &VideoPiece, k: i64) -> VideoPiece {
    let mut p = piece.clone();
    match &mut p {
        VideoPiece::Source {
            source_frame,
            frames,
            image,
            ..
        } => {
            // A still's piece always reads its one picture from the start.
            if !matches!(image, Some(Image::Still)) {
                *source_frame += k;
            }
            *frames -= k;
        }
        VideoPiece::Black { frames } => *frames -= k,
    }
    p
}

/// What a piece shows, apart from where in it: a file at the plan's rate,
/// or black of the plan's size.
fn source_key(piece: &VideoPiece, plan: &RenderPlan) -> Arc<str> {
    match piece {
        VideoPiece::Source {
            path,
            origin,
            image,
            ..
        } => format!(
            "{}|{:x}|{image:?}|{}",
            path.display(),
            origin.to_bits(),
            plan.fps
        )
        .into(),
        VideoPiece::Black { .. } => format!("black|{}x{}", plan.width, plan.height).into(),
    }
}

/// The frame whose sample span holds `sample`: frame `j` spans
/// `[frame_to_sample(j), frame_to_sample(j + 1))`. 0 at or before the
/// start.
pub(crate) fn frame_of(fps: Rational, sample: i64, rate: u32) -> i64 {
    if sample <= 0 || rate == 0 {
        return 0;
    }
    let mut frame = (i128::from(sample) * i128::from(fps.num())
        / (i128::from(rate) * i128::from(fps.den()))) as i64;
    while frame > 0 && fps.frame_to_sample(frame, rate) > sample {
        frame -= 1;
    }
    while fps.frame_to_sample(frame + 1, rate) <= sample {
        frame += 1;
    }
    frame
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn source(media: &str, source_frame: i64, frames: i64) -> VideoPiece {
        VideoPiece::Source {
            media: media.into(),
            path: PathBuf::from(format!("/{media}.mp4")),
            origin: 0.0,
            source_frame,
            frames,
            image: None,
        }
    }

    fn plan(video: Vec<VideoPiece>, range: (i64, i64)) -> Plan {
        let plan = RenderPlan {
            fps: Rational::new(30000, 1001).unwrap(),
            width: 320,
            height: 180,
            sample_rate: 48_000,
            channels: 2,
            frames: range.1 - range.0,
            video,
            audio: Vec::new(),
            range,
            name: "t".into(),
        };
        Plan::new(plan)
    }

    #[test]
    fn a_sample_belongs_to_the_frame_that_starts_at_or_before_it() {
        // 29.97 fps at 48 kHz: frames alternate 1601 and 1602 samples.
        let fps = Rational::new(30000, 1001).unwrap();
        let rate = 48_000;
        for j in [0, 1, 2, 29, 30, 1000, 107_892] {
            let s = fps.frame_to_sample(j, rate);
            assert_eq!(frame_of(fps, s, rate), j, "frame {j}'s first sample");
            assert_eq!(frame_of(fps, s - 1, rate), (j - 1).max(0));
        }
        assert_eq!(frame_of(fps, -5, rate), 0);
    }

    #[test]
    fn frames_map_to_pictures_that_survive_moves() {
        // A plan of timeline frames 100..160: m1 0..30, black 10, m2 5..25.
        let p = plan(
            vec![
                source("m1", 0, 30),
                VideoPiece::Black { frames: 10 },
                source("m2", 5, 20),
            ],
            (100, 160),
        );
        assert_eq!(p.first(), 100);
        assert_eq!(p.last(), 159);
        assert_eq!(p.id(129), p.id(129));
        assert_ne!(p.id(129), p.id(130), "the cut into black");
        assert_eq!(p.id(130), p.id(139), "black is one picture");
        // m2's source frame 5 is timeline frame 140; past the end clamps.
        let (piece, start) = p.piece_at(145).unwrap();
        assert_eq!(start, 140);
        assert!(matches!(
            piece,
            VideoPiece::Source {
                source_frame: 10,
                frames: 15,
                ..
            }
        ));
        assert_eq!(p.id(500), p.id(159));
        let from = p.pieces_from(135);
        assert_eq!(from.len(), 2);
        assert_eq!(from[0], VideoPiece::Black { frames: 5 });
        // The same source frames elsewhere on another timeline are the same
        // pictures.
        let moved = plan(vec![source("m2", 0, 40)], (0, 40));
        assert_eq!(moved.id(10), p.id(145));
        // Sample positions and frames meet exactly.
        for t in [100, 101, 129, 130, 159] {
            assert_eq!(p.frame_at_sample(p.sample(t)), t);
        }
    }
}
