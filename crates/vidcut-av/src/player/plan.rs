//! A render plan as the player reads it: which picture each timeline frame
//! shows, and where frames and samples meet.

use std::sync::Arc;

use vidcut_core::render::{Image, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

/// What a picture shows, wherever it is on a timeline: the same source
/// position of the same file at one rate, or black of one size; while a
/// dissolve plays, with the same pictures over it at the same weights.
/// Pictures with equal ids are equal, so kept pictures survive edits that
/// move them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Id {
    source: Arc<str>,
    pos: i64,
    /// A mix's pictures over the first: what each shows, where, and its
    /// weight.
    over: Option<Arc<[Layer]>>,
}

/// What a picture over another shows, where, and its weight.
type Layer = (Arc<str>, i64, u32);

#[cfg(test)]
impl Id {
    /// Frame `pos` of the file `source`.
    pub(crate) fn at(source: &str, pos: i64) -> Self {
        Self {
            source: source.into(),
            pos,
            over: None,
        }
    }
}

/// A plan, indexed.
pub(crate) struct Plan {
    pub(crate) plan: RenderPlan,
    /// The first frame of each video piece, counted from the plan's start.
    starts: Vec<i64>,
    /// What each video piece shows, apart from where in it: each of a
    /// mix's pictures.
    sources: Vec<Vec<Arc<str>>>,
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
                piece
                    .layers()
                    .into_iter()
                    .map(|(l, _)| source_key(l, &plan))
                    .collect()
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
        let Some(piece) = self.plan.video.get(i) else {
            return Id {
                source: "".into(),
                pos: 0,
                over: None,
            };
        };
        let pos = |l: &VideoPiece| match l {
            VideoPiece::Source { image, .. } if !matches!(image, Some(Image::Still)) => {
                l.position(k)
            }
            // Every frame of black, or of a still, is one picture.
            _ => 0,
        };
        let keys = &self.sources[i];
        let layers = piece.layers();
        let over: Vec<Layer> = layers[1..]
            .iter()
            .zip(&keys[1..])
            .map(|((l, ramp), key)| (key.clone(), pos(l), ramp.map_or(0, |r| r.weight(k))))
            .collect();
        Id {
            source: keys[0].clone(),
            pos: pos(layers[0].0),
            over: (!over.is_empty()).then(|| over.into()),
        }
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
    piece.part(k, piece.frames() - k)
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
        VideoPiece::Black { .. } | VideoPiece::Mix { .. } => {
            format!("black|{}x{}", plan.width, plan.height).into()
        }
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
