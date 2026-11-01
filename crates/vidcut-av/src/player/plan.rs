use std::sync::Arc;

use vidcut_core::model::{Retime, WipeDirection};
use vidcut_core::render::{Image, MovingKey, Placed, RenderPlan, Run, ShownTitle, VideoPiece};
use vidcut_core::time::Rational;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Id {
    source: Arc<str>,
    pos: Pos,
    over: Option<Arc<[Layer]>>,
}

type Pos = (i64, Option<Keyed>);

type Keyed = Arc<[(Option<i64>, Option<Run>)]>;

type Layer = (
    Arc<str>,
    Pos,
    u32,
    Option<WipeDirection>,
    Option<MovingKey>,
    Option<(u64, Retime)>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fill {
    pub(crate) ahead: bool,
    pub(crate) behind: bool,
    pub(crate) lead: bool,
    pub(crate) made: bool,
}

impl Fill {
    const ALL: Self = Self {
        ahead: true,
        behind: true,
        lead: true,
        made: false,
    };

    fn of(piece: &VideoPiece) -> Self {
        match piece {
            VideoPiece::Source { super_scale, .. } if super_scale.ai() => Self {
                behind: false,
                lead: false,
                ..Self::ALL
            },
            VideoPiece::Source { retime, .. } if retime.ai() => Self {
                behind: false,
                lead: false,
                made: true,
                ..Self::ALL
            },
            _ => Self::ALL,
        }
    }

    fn and(self, other: Self) -> Self {
        Self {
            ahead: self.ahead && other.ahead,
            behind: self.behind && other.behind,
            lead: self.lead && other.lead,
            made: self.made || other.made,
        }
    }
}

#[cfg(test)]
impl Id {
    pub(crate) fn at(source: &str, pos: i64) -> Self {
        Self {
            source: source.into(),
            pos: (pos, None),
            over: None,
        }
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
                pos: (0, None),
                over: None,
            };
        };
        let pos = |l: &VideoPiece| {
            let at = match l {
                VideoPiece::Source { image, retime, .. }
                    if !matches!(image, Some(Image::Still)) && !retime.is_nearest() =>
                {
                    (l.exact(k) * 1_000_000.0).round() as i64
                }
                VideoPiece::Source { image, .. } if !matches!(image, Some(Image::Still)) => {
                    l.position(k)
                }
                _ => 0,
            };
            (at, placed_now(l, k))
        };
        let keys = &self.sources[i];
        let layers = piece.layers();
        let motions = piece.motions();
        let smooths = piece.smooths();
        let over: Vec<Layer> = layers[1..]
            .iter()
            .zip(&motions[1..])
            .zip(&smooths[1..])
            .zip(&keys[1..])
            .map(|((((l, ramp), motion), smooth), key)| {
                let (weight, wipe) = ramp.map_or((0, None), |r| (r.weight(k), r.wipe));
                let moving = motion.zip(*ramp).map(|(m, r)| m.at(&r, k).key());
                let smoothed = smooth
                    .zip(*ramp)
                    .map(|(s, r)| (s.at(&r, k).to_bits(), s.retime));
                (key.clone(), pos(l), weight, wipe, moving, smoothed)
            })
            .collect();
        Id {
            source: keys[0].clone(),
            pos: pos(layers[0].0),
            over: (!over.is_empty()).then(|| over.into()),
        }
    }

    pub(crate) fn titles_at(&self, t: i64) -> Vec<(usize, ShownTitle)> {
        let (i, k) = self.locate(t);
        self.plan
            .video
            .get(i)
            .map(|p| titles_of(p, k))
            .unwrap_or_default()
    }

    pub(crate) fn placed_at(
        &self,
        t: i64,
        preview: &[(String, vidcut_core::model::Picture)],
    ) -> Vec<Vec<vidcut_core::render::Placed>> {
        let (i, k) = self.locate(t);
        self.plan
            .video
            .get(i)
            .map(|p| placed_of(p, k, preview))
            .unwrap_or_default()
    }

    pub(crate) fn scales_at(&self, t: i64) -> Vec<Option<vidcut_core::model::Sharpness>> {
        let (i, _) = self.locate(t);
        self.plan.video.get(i).map(scales_of).unwrap_or_default()
    }

    pub(crate) fn super_scales_at(&self, t: i64) -> Vec<vidcut_core::model::SuperScale> {
        let (i, _) = self.locate(t);
        self.plan
            .video
            .get(i)
            .map(VideoPiece::super_scales)
            .unwrap_or_default()
    }

    pub(crate) fn retimes_at(&self, t: i64) -> Vec<vidcut_core::model::Retime> {
        let (i, _) = self.locate(t);
        self.plan
            .video
            .get(i)
            .map(VideoPiece::retimes)
            .unwrap_or_default()
    }

    pub(crate) fn fill_at(&self, t: i64) -> Fill {
        let (i, _) = self.locate(t);
        self.plan.video.get(i).map_or(Fill::ALL, |piece| {
            let made = Fill {
                behind: false,
                lead: false,
                made: true,
                ..Fill::ALL
            };
            piece
                .layers()
                .into_iter()
                .fold(Fill::ALL, |f, (l, _)| f.and(Fill::of(l)))
                .and(
                    match piece.smooths().into_iter().flatten().any(|s| s.retime.ai()) {
                        true => made,
                        false => Fill::ALL,
                    },
                )
        })
    }

    pub(crate) fn crosses_at(&self, t: i64) -> Vec<bool> {
        let (i, _) = self.locate(t);
        self.plan
            .video
            .get(i)
            .map(VideoPiece::crosses)
            .unwrap_or_default()
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.plan.width, self.plan.height)
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

pub(crate) fn previewed(
    placed: &[Placed],
    preview: &[(String, vidcut_core::model::Picture)],
) -> Vec<Placed> {
    placed
        .iter()
        .map(|p| match preview.iter().find(|(clip, _)| *clip == p.clip) {
            Some((_, picture)) => Placed {
                picture: picture.clone(),
                ..p.clone()
            },
            None => p.clone(),
        })
        .collect()
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
            super_scale,
            retime,
            placed,
            ..
        } => {
            let made = if !retime.is_nearest() {
                let placed: Vec<_> = placed
                    .iter()
                    .map(|p| {
                        let mut picture = p.picture.clone();
                        picture
                            .effects
                            .iter_mut()
                            .for_each(|e| e.render_cache = false);
                        picture
                    })
                    .collect();
                format!(
                    "|{super_scale:?}|{}x{}|{:?}|{placed:?}|{retime:?}",
                    plan.width, plan.height, plan.colors
                )
            } else if super_scale.ai() {
                let detail =
                    (!placed.is_empty()).then(|| vidcut_core::place::detail(placed).to_bits());
                format!(
                    "|{:?}|{}x{}|{:?}|{detail:?}",
                    super_scale.upscaler, plan.width, plan.height, plan.colors
                )
            } else {
                String::new()
            };
            format!(
                "{}|{:x}|{image:?}|{}|{}{made}",
                path.display(),
                origin.to_bits(),
                plan.fps,
                color_space.key()
            )
            .into()
        }
        VideoPiece::Black { color, .. } => format!(
            "black|{}x{}|{}",
            plan.width,
            plan.height,
            vidcut_core::model::hex(*color)
        )
        .into(),
        VideoPiece::Mix { .. } | VideoPiece::Titled { .. } => {
            format!("black|{}x{}", plan.width, plan.height).into()
        }
    }
}

fn placed_now(piece: &VideoPiece, k: i64) -> Option<Keyed> {
    match piece {
        VideoPiece::Source {
            image,
            retime,
            placed,
            ..
        } if !matches!(image, Some(Image::Still)) && !retime.is_nearest() => Some(
            placed
                .iter()
                .map(|p| {
                    let source = p.picture.is_keyed().then(|| p.source_after(k));
                    (source, p.run.map(|r| r.after(k)))
                })
                .collect(),
        ),
        _ => None,
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

pub(crate) fn titles_of(piece: &VideoPiece, k: i64) -> Vec<(usize, ShownTitle)> {
    piece
        .titles()
        .into_iter()
        .map(|(n, s)| (n, s.after(k)))
        .collect()
}

pub(crate) fn placed_of(
    piece: &VideoPiece,
    k: i64,
    preview: &[(String, vidcut_core::model::Picture)],
) -> Vec<Vec<Placed>> {
    piece
        .layers()
        .into_iter()
        .map(|(l, _)| match l {
            VideoPiece::Source { placed, .. } => {
                let placed: Vec<Placed> = placed.iter().map(|p| p.after(k)).collect();
                previewed(&placed, preview)
            }
            _ => Vec::new(),
        })
        .collect()
}

pub(crate) fn scales_of(piece: &VideoPiece) -> Vec<Option<vidcut_core::model::Sharpness>> {
    piece
        .layers()
        .into_iter()
        .map(|(l, _)| match l {
            VideoPiece::Source { super_scale, .. } => {
                (!super_scale.is_off()).then_some(super_scale.sharpness)
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use vidcut_core::model::{Picture, Retime, SuperScale, Upscaler, WipeDirection};
    use vidcut_core::render::{Over, Placed, Ramp, RenderPlan, VideoPiece};
    use vidcut_core::time::Rational;

    use super::source_key;

    fn plan() -> RenderPlan {
        RenderPlan {
            fps: Rational::new(30, 1).unwrap(),
            width: 64,
            height: 36,
            sample_rate: 48_000,
            channels: 2,
            frames: 10,
            video: Vec::new(),
            audio: Vec::new(),
            ducking: Vec::new(),
            range: (0, 10),
            name: "keys".into(),
            colors: None,
        }
    }

    fn piece(super_scale: SuperScale, retime: Retime, zoom: f64, start: i64) -> VideoPiece {
        VideoPiece::Source {
            media: "m".into(),
            path: "clip.mp4".into(),
            origin: 0.0,
            source_frame: 0,
            frames: 10,
            image: None,
            pace: Default::default(),
            color_space: Default::default(),
            super_scale,
            retime,
            placed: vec![Placed {
                clip: "c".into(),
                picture: Picture {
                    zoom: Some([zoom, zoom]),
                    ..Picture::default()
                },
                run: None,
                source: 0,
                start,
                ready: true,
            }],
            cache: Default::default(),
            excluded: false,
            made: None,
        }
    }

    fn wiped_in(direction: WipeDirection) -> super::Id {
        let off = SuperScale::default();
        let mixed = VideoPiece::Mix {
            under: Box::new(VideoPiece::black(10)),
            over: vec![Over {
                piece: piece(off, Retime::Nearest, 1.0, 0),
                ramp: Some(Ramp {
                    at: 0,
                    of: 10,
                    falling: false,
                    wipe: Some(direction),
                }),
                cross: false,
                motion: None,
                smooth: None,
            }],
        };
        super::Plan::new(RenderPlan {
            video: vec![mixed],
            ..plan()
        })
        .id(5)
    }

    #[test]
    fn a_frame_of_a_wipe_misses_after_a_change_of_its_edge() {
        assert_ne!(wiped_in(WipeDirection::Down), wiped_in(WipeDirection::Left));
    }

    #[test]
    fn a_red_frame_is_kept_apart_from_a_black_one() {
        let red = VideoPiece::Black {
            frames: 10,
            color: [255, 0, 0],
            excluded: false,
        };
        assert_ne!(
            source_key(&VideoPiece::black(10), &plan()),
            source_key(&red, &plan())
        );
    }

    #[test]
    fn a_made_frame_misses_after_a_zoom() {
        let off = SuperScale::default();
        let before = source_key(&piece(off, Retime::FrameBlend, 2.0, 0), &plan());
        let after = source_key(&piece(off, Retime::FrameBlend, 1.5, 0), &plan());
        assert_ne!(before, after);
    }

    #[test]
    fn an_upscaled_frame_stays_kept_after_a_move() {
        let ai = SuperScale {
            on: true,
            upscaler: Upscaler::AiFast,
            ..SuperScale::default()
        };
        let before = source_key(&piece(ai, Retime::Nearest, 2.0, 0), &plan());
        let after = source_key(&piece(ai, Retime::Nearest, 2.0, 48), &plan());
        assert_eq!(before, after);
    }
}
