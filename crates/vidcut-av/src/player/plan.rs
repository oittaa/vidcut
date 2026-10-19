use std::sync::Arc;

use vidcut_core::render::{Image, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Id {
    source: Arc<str>,
    pos: i64,
    over: Option<Arc<[Layer]>>,
    small: bool,
}

type Layer = (Arc<str>, i64, u32);

#[cfg(test)]
impl Id {
    pub(crate) fn at(source: &str, pos: i64) -> Self {
        Self {
            source: source.into(),
            pos,
            over: None,
            small: false,
        }
    }
}

impl Id {
    pub(crate) fn small(&self) -> Self {
        Self {
            small: true,
            ..self.clone()
        }
    }

    pub(crate) fn is_small(&self) -> bool {
        self.small
    }
}

pub(crate) struct Plan {
    pub(crate) plan: RenderPlan,
    starts: Vec<i64>,
    sources: Vec<Vec<Arc<str>>>,
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

    pub(crate) fn first(&self) -> i64 {
        self.plan.range.0
    }

    pub(crate) fn last(&self) -> i64 {
        self.plan.range.0 + (self.plan.frames - 1).max(0)
    }

    pub(crate) fn clamp(&self, t: i64) -> i64 {
        t.clamp(self.first(), self.last())
    }

    fn locate(&self, t: i64) -> (usize, i64) {
        let rel = self.clamp(t) - self.first();
        let i = self.starts.partition_point(|&s| s <= rel).saturating_sub(1);
        (i, rel - self.starts.get(i).copied().unwrap_or(0))
    }

    pub(crate) fn id(&self, t: i64) -> Id {
        let (i, k) = self.locate(t);
        let Some(piece) = self.plan.video.get(i) else {
            return Id {
                source: "".into(),
                pos: 0,
                over: None,
                small: false,
            };
        };
        let pos = |l: &VideoPiece| match l {
            VideoPiece::Source { image, .. } if !matches!(image, Some(Image::Still)) => {
                l.position(k)
            }
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
            small: false,
        }
    }

    pub(crate) fn pieces_from(&self, t: i64) -> Vec<VideoPiece> {
        let (i, k) = self.locate(t);
        let mut pieces = self.plan.video[i.min(self.plan.video.len())..].to_vec();
        if let Some(first) = pieces.first_mut() {
            *first = cut(first, k);
        }
        pieces
    }

    pub(crate) fn piece_at(&self, t: i64) -> Option<(VideoPiece, i64)> {
        let (i, k) = self.locate(t);
        let piece = self.plan.video.get(i)?;
        Some((cut(piece, k), self.clamp(t) - k))
    }

    pub(crate) fn sample(&self, t: i64) -> i64 {
        self.fps().frame_to_sample(t - self.first(), self.rate())
    }

    pub(crate) fn frame_at_sample(&self, s: i64) -> i64 {
        self.clamp(self.first() + frame_of(self.fps(), s, self.rate()))
    }
}

fn cut(piece: &VideoPiece, k: i64) -> VideoPiece {
    piece.part(k, piece.frames() - k)
}

fn source_key(piece: &VideoPiece, plan: &RenderPlan) -> Arc<str> {
    match piece {
        VideoPiece::Source {
            path,
            origin,
            image,
            color_space,
            ..
        } => format!(
            "{}|{:x}|{image:?}|{}|{}",
            path.display(),
            origin.to_bits(),
            plan.fps,
            color_space.key()
        )
        .into(),
        VideoPiece::Black { .. } | VideoPiece::Mix { .. } | VideoPiece::Titled { .. } => {
            format!("black|{}x{}", plan.width, plan.height).into()
        }
    }
}

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
