use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::generators::{Axis, Pattern};
use crate::model::{
    AlphaMode, Chart, Clip, ClipCache, ClipRetime, ColorSpace, CompositeMode, Compound, DuckDetect,
    Generator, Gradient, Hdr, Keyframe, Media, MediaSuperScale, MotionPath, PathSample, Picture,
    PictureTransition, Project, PushPreset, Retime, Section, Settings, SlidePreset, SmoothMode,
    SuperScale, TimelineColors, Title, Track, TrackKind, Transition,
};
use crate::motion::Area;
use crate::otio::transitions;
use crate::time::Rational;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RenderPlan {
    pub fps: Rational,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Output length in frames.
    pub frames: i64,
    pub video: Vec<VideoPiece>,
    /// Audio lanes to mix; each covers all of [`samples`](Self::samples).
    pub audio: Vec<Vec<AudioPiece>>,
    /// The ducking of audio lanes (decision D76).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ducking: Vec<Ducking>,
    /// Timeline range this plan renders, `[start, end)`.
    pub range: (i64, i64),
    /// A name for the output (per-clip exports: the clip's name).
    pub name: String,
    /// The timeline color space, resolved: its HDR transfer, or none for
    /// SDR. Every mix happens in it (decision D50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Hdr>,
    /// The `.cube` file that the whole frame goes through after its
    /// subtitles, before the output's colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_lut: Option<PathBuf>,
}

pub fn timeline_colors(p: &Project) -> Option<Hdr> {
    match p.settings.timeline_colors {
        TimelineColors::Sdr => None,
        TimelineColors::Pq => Some(Hdr::Pq),
        TimelineColors::Hlg => Some(Hdr::Hlg),
        TimelineColors::Auto => auto_colors(p),
    }
}

pub fn auto_colors(p: &Project) -> Option<Hdr> {
    let (plan, _) = plan_lanes(p, 0, p.end()).ok()?;
    colors_in(p, &plan)
}

fn colors_in(p: &Project, plan: &RenderPlan) -> Option<Hdr> {
    let pool = p.pool();
    let share = crate::export::color::picture_share(&plan.video, |piece| {
        let hdr = match piece {
            VideoPiece::Source {
                media, color_space, ..
            } if color_space.is_tagged() => pool
                .get(media.as_str())
                .and_then(|m| m.info.video.as_ref())
                .and_then(|v| v.hdr),
            VideoPiece::Source { color_space, .. } => color_space.hdr(),
            _ => None,
        };
        crate::export::color::PictureKind { hdr, depth: 0 }
    });
    share.mostly_hdr().then(|| share.transfer())
}

/// A ducked track's lanes, and the sounds that dip them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Ducking {
    /// The lanes of the plan's `audio` that it dips: its track's.
    pub lanes: Vec<usize>,
    pub dip: Dip,
    pub detect: DuckDetect,
    /// The peak in dBFS from which a source is active, by level.
    pub threshold_db: f64,
    /// The sound of its sources, where it can reach the plan.
    pub sources: Vec<DuckSource>,
}

/// The gain of a ducked lane over the plan's samples. The times are in
/// samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Dip {
    pub amount_db: f64,
    pub lookahead: f64,
    pub rise: f64,
    pub hold: f64,
    pub recovery: f64,
    /// Where a source is active, in samples of the plan, sorted and apart.
    /// Empty until the sources are heard (`duck::hear`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active: Vec<(i64, i64)>,
}

/// The sound of a ducking source's clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DuckSource {
    pub media: String,
    pub stream: u32,
    /// Raw timestamp (s) that source position 0 refers to.
    pub origin: f64,
    /// The timeline frames where it sounds, `[start, end)`.
    pub frames: (i64, i64),
    /// The clip's first frame, its source position there, and its pace.
    pub start: i64,
    pub source_in: i64,
    pub pace: Pace,
    /// The clip gain and the track's fader, in dB; with `keys`, the fader
    /// only.
    pub gain_db: f64,
    /// The clip's volume keyframes (decision D81), on top of `gain_db`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Keyframe>,
    /// The frame of the keyframes' axis at `start`: its source frame, or on a
    /// clip with a speed, its frame on the retimed source.
    #[serde(default, skip_serializing_if = "zero_i64")]
    pub key: i64,
}

impl Dip {
    pub fn gain_db(&self, s: i64) -> f64 {
        let x = s as f64;
        let (rise, recovery) = (self.rise.max(1.0), self.recovery.max(1.0));
        let after = self.hold + recovery;
        let first = self.active.partition_point(|&(_, b)| b as f64 + after <= x);
        let mut db = 0.0f64;
        for &(a, b) in &self.active[first..] {
            let (a, b) = (a as f64, b as f64);
            let fall = a - self.lookahead - rise;
            if x < fall {
                break;
            }
            let g = if x < a - self.lookahead {
                (1.0 - (std::f64::consts::PI * (x - fall) / rise).cos()) / 2.0
            } else if x < b + self.hold {
                1.0
            } else {
                (1.0 + (std::f64::consts::PI * (x - b - self.hold) / recovery).cos()) / 2.0
            };
            db = db.min(-self.amount_db * g);
        }
        db
    }

    pub fn gain(&self, s: i64) -> f32 {
        let db = self.gain_db(s);
        if db == 0.0 {
            1.0
        } else {
            10f64.powf(db / 20.0) as f32
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VideoPiece {
    Source {
        media: String,
        path: PathBuf,
        /// Raw timestamp (s) of the media's first video frame (`o`).
        origin: f64,
        /// First source position, in project frames from `origin`.
        source_frame: i64,
        frames: i64,
        /// Pictures that aren't a video file's frames.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<Image>,
        /// How it walks its source: frame `k` shows [`VideoPiece::position`].
        #[serde(default, skip_serializing_if = "Pace::is_normal")]
        pace: Pace,
        /// How its pictures' colors are read: the media's `color_space`.
        #[serde(default, skip_serializing_if = "ColorSpace::is_tagged")]
        color_space: ColorSpace,
        /// How its pictures' alpha channel is read: the media's `alpha_mode`.
        #[serde(default, skip_serializing_if = "AlphaMode::is_straight")]
        alpha_mode: AlphaMode,
        /// How its pictures are enlarged: the media's `super_scale`, with
        /// the upscaler that its setting gives on this computer.
        #[serde(default, skip_serializing_if = "SuperScale::is_off")]
        super_scale: SuperScale,
        /// How it makes a frame between two source frames: its clip's
        /// `retime`, or the project's, as it runs on this computer. Nearest
        /// when each frame lands on a source frame.
        #[serde(default, skip_serializing_if = "Retime::is_nearest")]
        retime: Retime,
        /// Where its picture goes over the frame: its clip's picture, then
        /// that of each compound clip it plays inside, outward. Empty, it
        /// covers the whole frame, its bars black.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        placed: Vec<Placed>,
        /// Its clip's Render Cache.
        #[serde(default, skip_serializing_if = "ClipCache::is_auto")]
        cache: ClipCache,
        /// Its track is out of the render cache's User mode.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        excluded: bool,
        /// Its frames are made frames of a placed picture of this shape,
        /// from the render cache: colors over the whole frame. `placed`
        /// gives their edges and alpha.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        made: Option<Shape>,
        /// The `.cube` file that its pictures go through just after their
        /// conversion into the timeline color space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_lut: Option<PathBuf>,
    },
    /// A black frame.
    Black { frames: i64 },
    /// A generator's picture, made at the plan's size.
    Generated {
        frames: i64,
        picture: Generated,
        /// Where its picture goes over the frame, as a source's. Empty, it
        /// covers the whole frame.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        placed: Vec<Placed>,
        /// Its track is out of the render cache's User mode.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        excluded: bool,
    },
    /// A compound clip as one picture of the frame size, for a Smooth Cut.
    /// Frame `k` is frame `skip + k` of its `pieces`. Past their end, their
    /// last frame stays.
    Flat {
        frames: i64,
        pieces: Arc<[VideoPiece]>,
        skip: i64,
        /// The compound clips that it plays inside.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        placed: Vec<Placed>,
        /// Its track is out of the render cache's User mode.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        excluded: bool,
    },
    /// Pictures over each other: `under`, then each of `over` on top of it,
    /// at its level while a dissolve plays (decision D50), each as long as
    /// the piece. `under` is a source, a generator's picture, black or a
    /// titled picture; each of `over` a source, a generator's picture or
    /// black.
    Mix {
        under: Box<VideoPiece>,
        over: Vec<Over>,
    },
    /// Titles over a picture (decision D65): `under`, then each of
    /// `titles` drawn on top of it in order. `under` is a source, black or
    /// a mix.
    Titled {
        under: Box<VideoPiece>,
        titles: Vec<ShownTitle>,
    },
}

/// What a [`VideoPiece::Generated`] shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Generated {
    /// One color: `[r, g, b]`, each from 0 to 255.
    SolidColor { color: [u8; 3] },
    /// Four colors from the corners. `frame` is the source frame of the
    /// piece's first frame when the gradient has keyframes, and 0 when not.
    FourColorGradient { gradient: Gradient, frame: i64 },
    /// A picture that a rule draws from the size of the timeline.
    Chart { chart: Chart },
}

impl Generated {
    pub fn of(g: &Generator, frame: i64) -> Option<Self> {
        match g {
            Generator::SolidColor { color } => Some(Self::SolidColor { color: *color }),
            Generator::FourColorGradient(gradient) => Some(Self::FourColorGradient {
                gradient: gradient.clone(),
                frame: if gradient.is_keyed() { frame } else { 0 },
            }),
            other => other.chart().map(|chart| Self::Chart { chart }),
        }
    }

    pub fn is_keyed(&self) -> bool {
        matches!(self, Self::FourColorGradient { gradient, .. } if gradient.is_keyed())
    }

    pub fn after(&self, k: i64) -> Self {
        match self {
            Self::FourColorGradient { gradient, frame } if gradient.is_keyed() => {
                Self::FourColorGradient {
                    gradient: gradient.clone(),
                    frame: frame + k,
                }
            }
            other => other.clone(),
        }
    }

    pub fn held(&self) -> Self {
        match self {
            Self::FourColorGradient { gradient, frame } if gradient.is_keyed() => {
                let at = |axis| gradient.center_at(axis, *frame as f64);
                Self::FourColorGradient {
                    gradient: Gradient {
                        center_x: at(Axis::X),
                        center_x_keys: Vec::new(),
                        center_y: at(Axis::Y),
                        center_y_keys: Vec::new(),
                        ..gradient.clone()
                    },
                    frame: 0,
                }
            }
            other => other.clone(),
        }
    }

    pub fn at(&self, k: i64) -> Pattern {
        match self {
            Self::SolidColor { color } => Pattern::SolidColor(*color),
            Self::FourColorGradient { gradient, frame } => {
                Pattern::of_gradient(gradient, (frame + k) as f64)
            }
            Self::Chart { chart } => Pattern::Chart(*chart),
        }
    }
}

/// A title of a [`VideoPiece::Titled`], with where the piece is in its clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShownTitle {
    pub clip: String,
    pub title: Title,
    pub run: Run,
    /// The source frame of its clip's first frame, which its keyframes are
    /// counted from.
    pub source_in: i64,
    /// How a dissolve at its clip's edge fades it in or out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade: Option<Ramp>,
    /// Where it goes over the frame: the picture of each compound clip it
    /// plays inside, outward. Empty, it is drawn over the frame as it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placed: Vec<Placed>,
    /// The clip whose motion path it follows. Left out when it follows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<Arc<Lead>>,
}

/// The clip whose motion path a title follows, as it goes over the frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Lead {
    /// The picture of that clip on the first frame of the title's clip, with
    /// the path.
    pub placed: Placed,
    /// The id of the path.
    pub path: String,
    /// Where the fitted picture of that clip is in the frame: x, y, width
    /// and height, in pixels.
    pub fitted: [f64; 4],
    /// The size of its upright source picture, in pixels.
    pub pixels: (f64, f64),
    /// The size of the frame, in pixels.
    pub frame: (f64, f64),
    /// The frames of the title's clip on which that clip plays: the first
    /// and the last, counted from the first frame of the title's clip.
    pub lasts: (i64, i64),
}

impl Lead {
    pub fn of(p: &Project, c: &Clip, follow: &crate::model::Follow) -> Option<Self> {
        let a = p.clip(&follow.clip)?;
        let path = a.motion.iter().find(|m| m.id == follow.path)?;
        let size = (p.settings.width, p.settings.height);
        let frame = (f64::from(size.0), f64::from(size.1));
        let video = p.media(&a.media).and_then(|m| m.info.video.as_ref());
        let (fitted, pixels) = match video.filter(|v| v.width > 0 && v.height > 0) {
            Some(v) => {
                let shape = ((v.width, v.height), (v.sar[0], v.sar[1]), v.rotation);
                (
                    crate::place::fitted(shape.0, shape.1, shape.2, size),
                    crate::place::upright_pixels(shape.0, shape.2),
                )
            }
            None => ([0.0, 0.0, frame.0, frame.1], frame),
        };
        let run = Run {
            at: c.start - a.start,
            of: a.duration,
            held: false,
        };
        let mut placed = Placed::at(a, a.picture.clone().unwrap_or_default(), run, c.start);
        placed.followed = Followed::named(a, path);
        Some(Self {
            placed,
            path: follow.path.clone(),
            fitted,
            pixels,
            frame,
            lasts: (a.start - c.start, a.end() - 1 - c.start),
        })
    }

    pub fn spot(&self, at: i64) -> Option<[f64; 2]> {
        let placed = self.placed.after(at.clamp(self.lasts.0, self.lasts.1));
        let area = placed.followed.held(&self.path, placed.source)?;
        let level = crate::place::level(&placed, self.frame, self.fitted, self.pixels);
        let [x, y] = level.forward(area.center);
        (x.is_finite() && y.is_finite()).then(|| [x / self.frame.0, y / self.frame.1])
    }
}

/// Where a piece is in its clip, which a moving title and a Dynamic Zoom
/// are placed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Run {
    /// Frames of the clip before the piece's first.
    pub at: i64,
    /// The clip's length in frames, negative where it plays backward.
    pub of: i64,
    /// The clip stays on its frame `at`, as in a frozen compound clip.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
}

impl Run {
    pub fn after(&self, k: i64) -> Self {
        Self {
            at: if self.held { self.at } else { self.at + k },
            ..*self
        }
    }

    pub fn top(&self, k: i64, picture: f64, tall: f64) -> f64 {
        let t = ((self.at + k) as f64 + 0.5) / self.of.max(1) as f64;
        picture - t * (picture + tall)
    }

    pub fn share(&self) -> f64 {
        if self.of == 0 {
            0.0
        } else {
            self.at as f64 / self.of as f64
        }
    }
}

impl ShownTitle {
    pub fn after(&self, k: i64) -> Self {
        Self {
            clip: self.clip.clone(),
            title: self.title.clone(),
            run: self.run.after(k),
            source_in: self.source_in,
            fade: self.fade.map(|r| r.after(k)),
            placed: self.placed.iter().map(|p| p.after(k)).collect(),
            lead: self.lead.clone(),
        }
    }

    pub fn frame(&self) -> f64 {
        (self.source_in + self.run.at) as f64
    }

    pub fn led(&self, mut title: Title) -> Title {
        let follows = title.follows.take().and(self.lead.as_ref());
        if let Some([x, y]) = follows.and_then(|l| l.spot(self.run.at)) {
            title.moved([x - 0.5, y - 0.5]);
        }
        title
    }

    pub fn sampled(self) -> Self {
        Self {
            title: self.led(self.title.sampled(self.frame())),
            ..self
        }
    }

    pub fn alpha(&self) -> f64 {
        self.title.opaque() * self.fade.map_or(1.0, |r| r.level(0))
    }
}

/// A picture over the others of a [`VideoPiece::Mix`]: at the level its
/// ramp gives each frame, or placed over the frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Over {
    pub piece: VideoPiece,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp: Option<Ramp>,
    /// The incoming side of a Smooth Cut. Each frame is made from the
    /// picture before it and this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth: Option<Smooth>,
}

/// How a Smooth Cut makes each frame from its two pictures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Smooth {
    /// The process that makes a frame between the two pictures.
    pub retime: Retime,
    /// How far the frame is from the outgoing picture to the incoming one,
    /// from 0 to 1, at frames from the start of the transition.
    pub keys: Vec<Keyframe>,
    /// The composite mode of the outgoing clip. It mixes the made frame with
    /// the layers below. Left out, the made frame covers them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<CompositeMode>,
    /// A side is a picture with alpha. The layers below show through the
    /// made frame.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear: bool,
}

impl Smooth {
    fn of(p: &Project, d: &transitions::Playing) -> Option<(Self, SmoothMode)> {
        let (kind, keys) = transitions::course_played(d.transition, d.frames.1 - d.frames.0)?;
        let PictureTransition::SmoothCut { mode, process, .. } = kind else {
            return None;
        };
        let retime = process.retime();
        let composite = d
            .before
            .map(|c| mode_at(p, c, c.end() - 1))
            .filter(|m| *m != CompositeMode::Normal);
        let smooth = Self {
            retime,
            keys,
            mode: composite,
            clear: false,
        };
        Some((smooth, mode))
    }

    pub fn lay(&self, placed: &mut Vec<Placed>) {
        if let Some(own) = placed.first_mut() {
            own.picture.composite_mode = self.mode.or(own.picture.composite_mode);
            return;
        }
        placed.extend((self.mode.is_some() || self.clear).then(|| Placed {
            clip: String::new(),
            picture: Picture {
                composite_mode: self.mode,
                ..Picture::default()
            },
            run: None,
            source: 0,
            frame: 0,
            start: 0,
            ready: true,
            ramp: None,
            crossing: false,
            moving: None,
            followed: Followed::default(),
        }));
    }

    pub fn at(&self, ramp: &Ramp, k: i64) -> f64 {
        let f = ramp.frame(k) as f64;
        let p = crate::keyframes::value_at(&self.keys, f).unwrap_or(0.0);
        (p + 0.5 / ramp.of.max(1) as f64).min(1.0)
    }
}

/// The size of a picture before it is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shape {
    /// Width and height, in pixels.
    pub size: (u32, u32),
    /// The shape of its pixels, width to height.
    pub sar: (i32, i32),
    /// Clockwise degrees that make it upright.
    pub rotation: u16,
}

/// A clip's picture as it goes over the frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Placed {
    pub clip: String,
    pub picture: Picture,
    /// Where the piece's first frame is in the clip, for a picture with a
    /// Dynamic Zoom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
    /// The keyframe position of this picture: its source frame, or on a
    /// clip with a speed, its frame on the retimed source. Keyed values are
    /// read here. `after` moves it with the piece (decision D75).
    #[serde(default, skip_serializing_if = "zero_i64")]
    pub source: i64,
    /// How many frames this picture is after the clip's first, for an effect
    /// that changes by itself from frame to frame. A cached piece has 0 here
    /// when no effect of its picture does.
    #[serde(default, skip_serializing_if = "zero_i64")]
    pub frame: i64,
    /// Timeline start of the clip, until the first `after` places it.
    #[serde(default, skip_serializing_if = "zero_i64")]
    pub start: i64,
    /// Set once `after` has placed this picture on a timeline frame.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ready: bool,
    /// The level or the move of a transition at an edge of this compound
    /// clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp: Option<Ramp>,
    /// This is no picture. It joins the two sides of a transition at an edge
    /// of a compound clip: they add up, then go over the layers below.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crossing: bool,
    /// The Push or the Slide of this crossing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moving: Option<Move>,
    /// The motion paths that the areas of its effects follow. Left out
    /// when none follows a path.
    #[serde(default, skip_serializing_if = "Followed::is_empty")]
    pub followed: Followed,
}

fn zero_i64(n: &i64) -> bool {
    *n == 0
}

/// The motion paths that the areas of a picture's effects follow, with the
/// way from a keyframe position of the clip to its source frame.
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Followed {
    /// The paths, on source frames, without their names.
    pub paths: Arc<[MotionPath]>,
    /// How the clip walks its source from its first frame.
    pub pace: Pace,
    /// The source frame of the clip's first frame.
    pub from: i64,
    /// The keyframe position of the clip's first frame.
    pub key: i64,
}

impl std::fmt::Debug for Followed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Followed({})", self.paths.len())
    }
}

impl PartialEq for Followed {
    fn eq(&self, other: &Self) -> bool {
        let same = |(a, b): (&MotionPath, &MotionPath)| {
            a.id == b.id
                && a.subject == b.subject
                && (Arc::ptr_eq(&a.samples, &b.samples) || a.samples == b.samples)
        };
        self.from == other.from
            && self.key == other.key
            && self.pace == other.pace
            && self.paths.len() == other.paths.len()
            && self.paths.iter().zip(other.paths.iter()).all(same)
    }
}

impl Followed {
    pub fn of(clip: &Clip, picture: &Picture) -> Self {
        let named = |e: &crate::model::Effect| e.area.as_ref()?.follows.clone();
        let named: Vec<String> = picture.effects.iter().filter_map(named).collect();
        let follows = |m: &&MotionPath| named.iter().any(|n| m.follows(n));
        let shared = |m: &MotionPath| MotionPath {
            id: m.id.clone(),
            name: None,
            subject: m.subject,
            tracked: m.tracked,
            samples: m.samples.clone(),
        };
        let paths: Arc<[MotionPath]> = clip.motion.iter().filter(follows).map(shared).collect();
        if paths.is_empty() {
            return Self::default();
        }
        Self {
            paths,
            pace: clip.pace(),
            from: clip.source_in,
            key: clip.content_at(clip.start),
        }
    }

    pub fn named(clip: &Clip, path: &MotionPath) -> Self {
        Self {
            paths: Arc::new([MotionPath {
                name: None,
                ..path.clone()
            }]),
            pace: clip.pace(),
            from: clip.source_in,
            key: clip.content_at(clip.start),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn held(&self, named: &str, source: i64) -> Option<Area> {
        let frame = (self.from + self.pace.offset(source - self.key)) as f64;
        let path = self.paths.iter().find(|m| m.id == named)?;
        path.held_at(frame)
    }

    pub fn bases(&self, named: &str, source: i64) -> Vec<Area> {
        let frame = (self.from + self.pace.offset(source - self.key)) as f64;
        let followed = self.paths.iter().filter(|m| m.follows(named));
        followed.filter_map(|m| m.base_at(frame)).collect()
    }

    pub fn stamp_at(&self, source: i64) -> u64 {
        use std::hash::{Hash, Hasher};
        let frame = (self.from + self.pace.offset(source - self.key)) as f64;
        let mut hasher = std::hash::DefaultHasher::new();
        for m in self.paths.iter() {
            if let Some(area) = m.base_at(frame) {
                m.id.hash(&mut hasher);
                let numbers = area.center.iter().chain(&area.size).chain([&area.rotation]);
                for v in numbers {
                    v.to_bits().hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }

    pub fn at(&self, source: i64) -> Self {
        let held = |m: &MotionPath| {
            let area = m.base_at((self.from + self.pace.offset(source - self.key)) as f64)?;
            Some(MotionPath {
                id: m.id.clone(),
                name: None,
                subject: m.subject,
                tracked: m.tracked,
                samples: Arc::new(vec![PathSample {
                    frame: 0,
                    center: area.center,
                    size: area.size,
                    rotation: area.rotation,
                }]),
            })
        };
        Self {
            paths: self.paths.iter().filter_map(held).collect(),
            pace: Pace::default(),
            from: 0,
            key: 0,
        }
    }
}

impl Placed {
    pub fn of(clip: &Clip, picture: Picture, run: Run) -> Self {
        let picture = crate::ops::picture::working(picture);
        let followed = Followed::of(clip, &picture);
        Self {
            clip: clip.id.clone(),
            run: picture.dynamic_zoom.map(|_| run),
            picture,
            source: clip.content_at(clip.start),
            frame: 0,
            start: clip.start,
            ready: false,
            ramp: None,
            crossing: false,
            moving: None,
            followed,
        }
    }

    pub fn at(clip: &Clip, picture: Picture, run: Run, time: i64) -> Self {
        let picture = crate::ops::picture::working(picture);
        let run = picture.dynamic_zoom.map(|_| run);
        let followed = Followed::of(clip, &picture);
        Self {
            clip: clip.id.clone(),
            picture,
            run,
            source: clip.content_at(time),
            frame: time - clip.start,
            start: clip.start,
            ready: true,
            ramp: None,
            crossing: false,
            moving: None,
            followed,
        }
    }

    fn keeps_cover(&self) -> bool {
        !self.crossing && self.ramp.is_none() && self.picture.is_usual()
    }

    pub fn source_after(&self, k: i64) -> i64 {
        self.source + if self.ready { k } else { k - self.start }
    }

    pub fn after(&self, k: i64) -> Self {
        Self {
            clip: self.clip.clone(),
            picture: self.picture.clone(),
            run: self.run.map(|r| r.after(k)),
            source: self.source_after(k),
            frame: self.frame + if self.ready { k } else { k - self.start },
            start: self.start,
            ready: true,
            ramp: self.ramp.map(|r| r.after(k)),
            crossing: self.crossing,
            moving: self.moving.clone(),
            followed: self.followed.clone(),
        }
    }

    pub fn zoomed(&self) -> Option<crate::model::ZoomBox> {
        let z = self.picture.dynamic_zoom?;
        Some(z.at(self.run.map_or(0.0, |r| r.share())))
    }
}

/// How a dissolve's level goes over its frames (decision D50): straight
/// from 0 to 1, or from 1 to 0 when `falling`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ramp {
    /// Frames of the dissolve before the piece's first.
    pub at: i64,
    /// The dissolve's length in frames.
    pub of: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub falling: bool,
    /// The level stays at frame `at`, as in a frozen compound clip.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    /// A wipe, a Push or a Slide moves this picture, and its level stays 1.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moved: bool,
}

pub const WHOLE: u32 = 1 << 16;

pub const BLACK: [u8; 3] = [0, 0, 0];

impl Ramp {
    pub fn frame(&self, k: i64) -> i64 {
        if self.held { self.at } else { self.at + k }
    }

    pub fn rising(&self, k: i64) -> (i64, i64) {
        (2 * self.frame(k) + 1, 2 * self.of)
    }

    pub fn fraction(&self, k: i64) -> (i64, i64) {
        let rising = self.rising(k);
        if self.falling {
            (rising.1 - rising.0, rising.1)
        } else {
            rising
        }
    }

    pub fn level(&self, k: i64) -> f64 {
        let (n, d) = self.fraction(k);
        n as f64 / d as f64
    }

    pub fn weight(&self, k: i64) -> u32 {
        let (n, d) = self.fraction(k);
        let w = (i128::from(n) * i128::from(WHOLE) * 2 + i128::from(d)) / (2 * i128::from(d));
        w.clamp(0, i128::from(WHOLE)) as u32
    }

    pub fn after(&self, k: i64) -> Self {
        Self {
            at: self.frame(k),
            ..*self
        }
    }
}

/// The way the pictures of a wipe, a Push or a Slide go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Way {
    Push(PushPreset),
    /// The incoming picture slides in.
    Slide(SlidePreset),
    /// The outgoing picture slides out.
    SlideOut(SlidePreset),
    /// An edge at this angle in degrees crosses the frame. The incoming
    /// picture shows behind it.
    Wipe(i32),
}

/// How a wipe, a Push or a Slide moves its two pictures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Move {
    pub way: Way,
    /// The band on the moving edge: for a Push or a Slide as a part of the
    /// shorter side of the frame, for a wipe as a part of its width.
    #[serde(default, skip_serializing_if = "zero_f64")]
    pub border: f64,
    /// The band fades one picture into the other.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub feather: bool,
    pub color: [u8; 3],
    /// The motion blur, from 0 to 1.
    #[serde(default, skip_serializing_if = "zero_f64")]
    pub blur: f64,
    /// The progress of the move, from 0 to 1, at frames from the start of
    /// the transition.
    pub keys: Vec<Keyframe>,
}

fn zero_f64(n: &f64) -> bool {
    *n == 0.0
}

impl Move {
    fn of(d: &transitions::Playing) -> Option<Self> {
        let (kind, keys) = transitions::course_played(d.transition, d.frames.1 - d.frames.0)?;
        let (way, motion) = match kind {
            PictureTransition::Wipe { wipe } => (
                Way::Wipe(wipe.angle),
                crate::model::Motion {
                    border: wipe.border,
                    feather: wipe.feather,
                    color: wipe.color,
                    motion_blur: 0.0,
                    course: wipe.course,
                },
            ),
            PictureTransition::Push { preset, motion } => (Way::Push(preset), motion),
            PictureTransition::Slide {
                preset,
                out,
                motion,
            } => match out {
                false => (Way::Slide(preset), motion),
                true => (Way::SlideOut(preset), motion),
            },
            _ => return None,
        };
        Some(Self {
            way,
            border: motion.border.clamp(0.0, crate::model::MAX_BORDER),
            feather: motion.feather,
            color: motion.color,
            blur: motion.motion_blur.clamp(0.0, 1.0),
            keys,
        })
    }

    pub fn at(&self, ramp: &Ramp, k: i64) -> Moving {
        let f = ramp.frame(k) as f64;
        let p = |f: f64| crate::keyframes::value_at(&self.keys, f).unwrap_or(0.0);
        Moving {
            way: self.way,
            progress: p(f),
            step: p(f + 1.0) - p(f),
            border: self.border,
            feather: self.feather,
            color: self.color,
            blur: self.blur,
        }
    }
}

/// A frame of a [`Move`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moving {
    pub way: Way,
    pub progress: f64,
    pub step: f64,
    pub border: f64,
    pub feather: bool,
    pub color: [u8; 3],
    pub blur: f64,
}

/// Where the two pictures of a [`Moving`] are, in pixels of the frame.
/// The upper picture carries the band on the edges that face the lower
/// picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shift {
    pub upper_in: bool,
    pub upper: [f64; 2],
    pub lower: [f64; 2],
    pub picture: [f64; 4],
    pub band: [f64; 4],
    pub facing: [f64; 2],
    pub edge: [f64; 2],
    pub border: f64,
    pub feather: bool,
    pub blur: [f64; 3],
    pub mixes_blur: bool,
    pub still: bool,
    pub wipe: Option<[f64; 3]>,
}

fn push_sign(preset: PushPreset) -> [f64; 2] {
    match preset {
        PushPreset::Left => [1.0, 0.0],
        PushPreset::Right => [-1.0, 0.0],
        PushPreset::Up => [0.0, -1.0],
        PushPreset::Down => [0.0, 1.0],
    }
}

fn slide_sign(preset: SlidePreset) -> [f64; 2] {
    match preset {
        SlidePreset::LeftRight => [1.0, 0.0],
        SlidePreset::RightLeft => [-1.0, 0.0],
        SlidePreset::BottomUp => [0.0, -1.0],
        SlidePreset::TopDown => [0.0, 1.0],
        SlidePreset::TopLeft => [1.0, 1.0],
        SlidePreset::BottomRight => [-1.0, -1.0],
    }
}

impl Moving {
    pub fn shift(&self, (w, h): (u32, u32)) -> Shift {
        let size = [f64::from(w), f64::from(h)];
        let (sign, push, out) = match self.way {
            Way::Push(preset) => (push_sign(preset), true, false),
            Way::Slide(preset) => (slide_sign(preset), false, false),
            Way::SlideOut(preset) => (slide_sign(preset), false, true),
            Way::Wipe(angle) => return self.wiped(angle, size),
        };
        let b = self.border * size[0].min(size[1]);
        let p = self.progress;
        let (mut upper, mut lower, mut facing) = ([0.0; 2], [0.0; 2], [0.0; 2]);
        for a in 0..2 {
            let s = sign[a];
            if s == 0.0 {
                continue;
            }
            let m = p * (size[a] + b);
            facing[a] = if out { -s } else { s };
            upper[a] = if out { s * m } else { s * (m - size[a] - b) };
            lower[a] = if push { s * m } else { 0.0 };
            if self.feather {
                upper[a] += facing[a] * b;
                if push {
                    lower[a] -= facing[a] * b;
                }
            }
        }
        let picture = [upper[0], upper[1], upper[0] + size[0], upper[1] + size[1]];
        let grown = |a: usize, side: f64| match self.feather {
            false if facing[a] == side => b,
            _ => 0.0,
        };
        let band = [
            picture[0] - grown(0, -1.0),
            picture[1] - grown(1, -1.0),
            picture[2] + grown(0, 1.0),
            picture[3] + grown(1, 1.0),
        ];
        let edge = [0, 1].map(|a| match facing[a] > 0.0 {
            true => picture[a + 2],
            false => picture[a],
        });
        let way = [sign[0], if sign[0] == 0.0 { -sign[1] } else { sign[1] }];
        let reach = way[0].hypot(way[1]).max(1.0);
        let forward = |v: [f64; 2]| [0, 1].map(|a| sign[a] * (sign[a] * v[a]).max(0.0));
        let (upper, lower) = match (push, out) {
            (true, _) => (upper, forward(lower)),
            (false, true) => (forward(upper), lower),
            (false, false) => (upper, lower),
        };
        Shift {
            upper_in: !out,
            upper: upper.map(f64::trunc),
            lower: lower.map(f64::trunc),
            picture,
            band,
            facing,
            edge,
            border: b,
            feather: self.feather,
            blur: [
                way[0] / reach,
                way[1] / reach,
                self.blur * 2.0 * size[1] * self.step.abs(),
            ],
            mixes_blur: !push,
            still: self.feather && p <= 0.0,
            wipe: None,
        }
    }

    fn wiped(&self, angle: i32, size: [f64; 2]) -> Shift {
        let (sin, cos) = f64::from(angle).to_radians().sin_cos();
        let way = [sin, -cos];
        let reach = [[0.0, 0.0], [size[0], 0.0], [0.0, size[1]], size]
            .map(|c| c[0] * way[0] + c[1] * way[1]);
        let first = reach.into_iter().fold(f64::INFINITY, f64::min);
        let last = reach.into_iter().fold(f64::NEG_INFINITY, f64::max);
        let b = self.border * size[0];
        let edge = first - b / 2.0 + self.progress * (last - first + b);
        let whole = [0.0, 0.0, size[0], size[1]];
        Shift {
            upper_in: true,
            upper: [0.0; 2],
            lower: [0.0; 2],
            picture: whole,
            band: whole,
            facing: [0.0; 2],
            edge: [0.0; 2],
            border: b,
            feather: self.feather,
            blur: [0.0; 3],
            mixes_blur: false,
            still: false,
            wipe: Some([way[0], way[1], edge]),
        }
    }
}

/// What a source piece shows when it isn't a video file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Image {
    /// One picture (the piece's `path`), for every frame.
    Still,
    /// Numbered pictures.
    Sequence(Box<Numbered>),
}

/// Numbered pictures, one a frame at `rate`: picture `k` is the file
/// `pattern` (a full path, FFmpeg's image2 form) names with the number
/// `first + k`. The piece's `path` is the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Numbered {
    pub pattern: PathBuf,
    pub first: u64,
    pub count: u64,
    pub rate: Rational,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AudioPiece {
    Source {
        media: String,
        path: PathBuf,
        /// Which of the media's audio streams.
        stream: u32,
        /// Raw timestamp (s) that source position 0 refers to (`o`).
        origin: f64,
        /// First source sample, counted at the plan's sample rate from `origin`.
        source_sample: i64,
        samples: i64,
        gain_db: f64,
        /// Its track's fader is at the bottom: no sound, until a fader
        /// preview moves the lane's gain.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        silent: bool,
        /// Source samples per sample heard (decision D48), the pitch kept;
        /// below 0 the source plays backward from `source_sample`.
        #[serde(default = "normal_speed", skip_serializing_if = "is_normal_speed")]
        speed: f64,
        /// It goes on from the piece before, in the next section of its
        /// clip's speed: the sound plays on through the change.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        joined: bool,
        /// A cross fade over it (decision D50): its gain on top of
        /// `gain_db`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fade: Option<Fade>,
        /// The fades at the edges of the compound clips around it: their
        /// gains on top of `gain_db`.
        #[serde(default, skip_serializing_if = "<[Fade]>::is_empty")]
        edges: Box<[Fade]>,
        /// Its clip's volume keyframes (decision D81): their gain at each
        /// sample heard, on top of `gain_db`, which then holds the
        /// track's fader only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keyed: Option<Box<Keyed>>,
        /// Voice Isolation's Amount over it, up to 100: the clip's own,
        /// else its track's; left out when that is off or at 0, where the
        /// model does not run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        isolation: Option<f64>,
        /// Its clip's pan, from -100 to 100. Left out at 0.
        #[serde(default, skip_serializing_if = "is_centered")]
        pan: f64,
        /// Its clip's equalizer, left out when it changes nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        equalizer: Option<Box<crate::model::Equalizer>>,
        /// Its clip's Dialogue Leveler, left out when it changes nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        leveler: Option<Box<crate::model::DialogueLeveler>>,
        /// Its clip's compressor, left out when it changes nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compressor: Option<Box<crate::model::Compressor>>,
        /// Its clip's Noise Reduction, left out when it changes nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        noise_reduction: Option<Box<crate::model::NoiseReduction>>,
    },
    Silence {
        samples: i64,
    },
}

/// A clip's volume keyframes over a piece of its sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Keyed {
    /// The keyframes, their frames on `at`'s axis in project frames.
    pub keys: Vec<Keyframe>,
    /// Plan samples in a project frame.
    pub per_frame: f64,
    /// The piece's first sample on the keyframes' axis, in plan samples:
    /// the source sample, or on a clip with a speed, the sample on the
    /// retimed source.
    pub at: f64,
}

impl Keyed {
    pub fn db_at(&self, sample: f64) -> f64 {
        crate::keyframes::value_at(&self.keys, sample / self.per_frame).map_or(0.0, |v| {
            v.clamp(*crate::validate::GAIN.start(), *crate::validate::GAIN.end())
        })
    }

    pub fn gain(&self, sample: f64) -> f32 {
        10f64.powf(self.db_at(sample) / 20.0) as f32
    }
}

/// A sound's gain through a cross fade (decision D50): rising from silence
/// to all of it along its curve, or falling when `falling`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Fade {
    /// Samples of the fade before the piece's first.
    pub at: i64,
    /// The fade's length in samples.
    pub of: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub falling: bool,
    pub curve: Curve,
}

/// How a cross fade's gain goes with its share `x` of the way (0 to 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Curve {
    /// Cross Fade +3 dB: the square root of `x`, so the two sounds keep
    /// their power together.
    #[serde(rename = "+3 dB")]
    Plus3Db,
    /// Cross Fade 0 dB: `x`.
    #[serde(rename = "0 dB")]
    ZeroDb,
    /// Cross Fade -3 dB: `x` to the power of 1.5.
    #[serde(rename = "-3 dB")]
    Minus3Db,
}

impl Curve {
    pub fn gain(self, x: f64) -> f64 {
        match self {
            Self::Plus3Db => x.sqrt(),
            Self::ZeroDb => x,
            Self::Minus3Db => x * x.sqrt(),
        }
    }
}

impl Fade {
    pub fn gain(&self, s: i64) -> f64 {
        let x = ((self.at + s) as f64 / self.of as f64).clamp(0.0, 1.0);
        self.curve.gain(if self.falling { 1.0 - x } else { x })
    }

    pub fn starts_silent(&self) -> bool {
        !self.falling && self.at == 0
    }

    pub fn ends_silent(&self, samples: i64) -> bool {
        self.falling && self.at + samples >= self.of
    }
}

impl VideoPiece {
    pub fn black(frames: i64) -> Self {
        Self::Black { frames }
    }
    pub fn frames(&self) -> i64 {
        match self {
            Self::Source { frames, .. }
            | Self::Black { frames, .. }
            | Self::Generated { frames, .. }
            | Self::Flat { frames, .. } => *frames,
            Self::Mix { under, .. } | Self::Titled { under, .. } => under.frames(),
        }
    }

    pub fn placed(&self) -> &[Placed] {
        match self {
            Self::Source { placed, .. }
            | Self::Generated { placed, .. }
            | Self::Flat { placed, .. } => placed,
            _ => &[],
        }
    }

    pub fn layers(&self) -> Vec<(&VideoPiece, Option<Ramp>)> {
        match self {
            Self::Mix { under, over } => under
                .layers()
                .into_iter()
                .chain(over.iter().map(|o| (&o.piece, o.ramp)))
                .collect(),
            Self::Titled { under, .. } => under.layers(),
            _ => vec![(self, None)],
        }
    }

    pub fn layers_mut(&mut self) -> Vec<&mut VideoPiece> {
        match self {
            Self::Mix { under, over } => under
                .layers_mut()
                .into_iter()
                .chain(over.iter_mut().map(|o| &mut o.piece))
                .collect(),
            Self::Titled { under, .. } => under.layers_mut(),
            _ => vec![self],
        }
    }

    pub fn with_flat_pieces(&self) -> Vec<&VideoPiece> {
        std::iter::once(self)
            .chain(self.layers().into_iter().flat_map(|(l, _)| match l {
                Self::Flat { pieces, .. } => {
                    pieces.iter().flat_map(Self::with_flat_pieces).collect()
                }
                _ => Vec::new(),
            }))
            .collect()
    }

    pub fn flat_layers_mut(&mut self) -> Vec<&mut VideoPiece> {
        self.layers_mut()
            .into_iter()
            .flat_map(|l| match l {
                Self::Flat { pieces, .. } => Arc::make_mut(pieces)
                    .iter_mut()
                    .flat_map(Self::flat_layers_mut)
                    .collect(),
                _ => vec![l],
            })
            .collect()
    }

    pub fn smooths(&self) -> Vec<Option<&Smooth>> {
        match self {
            Self::Mix { under, over } => under
                .smooths()
                .into_iter()
                .chain(over.iter().map(|o| o.smooth.as_ref()))
                .collect(),
            Self::Titled { under, .. } => under.smooths(),
            _ => vec![None],
        }
    }

    pub fn super_scales(&self) -> Vec<SuperScale> {
        self.layers()
            .into_iter()
            .flat_map(|(l, _)| match l {
                Self::Source { super_scale, .. } if !super_scale.is_off() => vec![*super_scale],
                Self::Flat { pieces, .. } => pieces.iter().flat_map(Self::super_scales).collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    pub fn retimes(&self) -> Vec<Retime> {
        self.layers()
            .into_iter()
            .flat_map(|(l, _)| match l {
                Self::Source { image, retime, .. }
                    if !matches!(image, Some(Image::Still)) && !retime.is_nearest() =>
                {
                    vec![*retime]
                }
                Self::Flat { pieces, .. } => pieces.iter().flat_map(Self::retimes).collect(),
                _ => Vec::new(),
            })
            .chain(self.smooths().into_iter().flatten().map(|s| s.retime))
            .collect()
    }

    pub fn titles(&self) -> Vec<(usize, &ShownTitle)> {
        match self {
            Self::Mix { under, .. } => under.titles(),
            Self::Titled { under, titles } => {
                let below = under.layers().len();
                under
                    .titles()
                    .into_iter()
                    .chain(titles.iter().map(|t| (below, t)))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn with_over(self, o: Over) -> VideoPiece {
        match self {
            Self::Mix { under, mut over } => {
                over.push(o);
                Self::Mix { under, over }
            }
            under => Self::Mix {
                under: Box::new(under),
                over: vec![o],
            },
        }
    }

    fn with_title(self, t: ShownTitle) -> VideoPiece {
        match self {
            Self::Titled { under, mut titles } => {
                titles.push(t);
                Self::Titled { under, titles }
            }
            under => Self::Titled {
                under: Box::new(under),
                titles: vec![t],
            },
        }
    }

    pub fn part(&self, k: i64, frames: i64) -> VideoPiece {
        match self {
            Self::Source {
                media,
                path,
                origin,
                image,
                pace,
                color_space,
                alpha_mode,
                super_scale,
                retime,
                placed,
                cache,
                excluded,
                made,
                input_lut,
                ..
            } => {
                let still = matches!(image, Some(Image::Still));
                Self::Source {
                    media: media.clone(),
                    path: path.clone(),
                    origin: *origin,
                    source_frame: self.position(if still { 0 } else { k }),
                    frames,
                    image: image.clone(),
                    pace: if still { pace.clone() } else { pace.after(k) },
                    color_space: *color_space,
                    alpha_mode: *alpha_mode,
                    super_scale: *super_scale,
                    retime: *retime,
                    placed: placed.iter().map(|p| p.after(k)).collect(),
                    cache: *cache,
                    excluded: *excluded,
                    made: *made,
                    input_lut: input_lut.clone(),
                }
            }
            Self::Black { .. } => Self::Black { frames },
            Self::Generated {
                picture,
                placed,
                excluded,
                ..
            } => Self::Generated {
                frames,
                picture: picture.after(k),
                placed: placed.iter().map(|p| p.after(k)).collect(),
                excluded: *excluded,
            },
            Self::Flat {
                pieces,
                skip,
                placed,
                excluded,
                ..
            } => Self::Flat {
                frames,
                pieces: pieces.clone(),
                skip: skip + k,
                placed: placed.iter().map(|p| p.after(k)).collect(),
                excluded: *excluded,
            },
            Self::Mix { under, over } => Self::Mix {
                under: Box::new(under.part(k, frames)),
                over: over
                    .iter()
                    .map(|o| Over {
                        piece: o.piece.part(k, frames),
                        ramp: o.ramp.map(|r| r.after(k)),
                        smooth: o.smooth.clone(),
                    })
                    .collect(),
            },
            Self::Titled { under, titles } => Self::Titled {
                under: Box::new(under.part(k, frames)),
                titles: titles.iter().map(|t| t.after(k)).collect(),
            },
        }
    }

    fn goes_on_as(&self, next: &VideoPiece) -> bool {
        match (self, next) {
            (Self::Black { .. }, Self::Black { .. }) => true,
            (
                Self::Generated {
                    frames,
                    picture,
                    placed,
                    excluded,
                },
                Self::Generated {
                    picture: p2,
                    placed: l2,
                    excluded: e2,
                    ..
                },
            ) => {
                picture.after(*frames) == *p2
                    && excluded == e2
                    && placed.len() == l2.len()
                    && placed.iter().zip(l2).all(|(a, b)| a.after(*frames) == *b)
            }
            (
                Self::Source {
                    media,
                    source_frame,
                    frames,
                    cache,
                    pace,
                    retime,
                    placed,
                    excluded,
                    input_lut,
                    ..
                },
                Self::Source {
                    media: m2,
                    source_frame: s2,
                    cache: c2,
                    pace: p2,
                    retime: r2,
                    placed: l2,
                    excluded: e2,
                    input_lut: i2,
                    ..
                },
            ) => {
                media == m2
                    && excluded == e2
                    && retime == r2
                    && input_lut == i2
                    && pace.is_normal()
                    && p2.is_normal()
                    && cache == c2
                    && *source_frame + *frames == *s2
                    && placed.len() == l2.len()
                    && placed.iter().zip(l2).all(|(a, b)| a.after(*frames) == *b)
            }
            (
                Self::Mix { under, over },
                Self::Mix {
                    under: u2,
                    over: o2,
                },
            ) => {
                let k = self.frames();
                under.goes_on_as(u2)
                    && over.len() == o2.len()
                    && over.iter().zip(o2).all(|(a, b)| {
                        a.ramp.map(|r| r.after(k)) == b.ramp
                            && a.smooth == b.smooth
                            && a.piece.goes_on_as(&b.piece)
                    })
            }
            (
                Self::Titled { under, titles },
                Self::Titled {
                    under: u2,
                    titles: t2,
                },
            ) => {
                let k = self.frames();
                titles.iter().map(|t| t.after(k)).eq(t2.iter().cloned()) && under.goes_on_as(u2)
            }
            _ => false,
        }
    }

    fn lengthen(&mut self, more: i64) {
        match self {
            Self::Source { frames, .. }
            | Self::Black { frames, .. }
            | Self::Generated { frames, .. }
            | Self::Flat { frames, .. } => *frames += more,
            Self::Mix { under, over } => {
                under.lengthen(more);
                for o in over {
                    o.piece.lengthen(more);
                }
            }
            Self::Titled { under, .. } => under.lengthen(more),
        }
    }
}

fn normal_speed() -> f64 {
    1.0
}

fn is_normal_speed(speed: &f64) -> bool {
    *speed == 1.0
}

fn is_centered(pan: &f64) -> bool {
    *pan == 0.0
}

pub fn pan_db(pan: f64) -> (f64, f64) {
    let share = (pan.abs() / 100.0).min(1.0);
    let steps = |gain: f64| (gain.log10() * 200.0).round() / 10.0;
    let near = steps(1.0 + share);
    let far = steps(1.0 - share).max(-6.0);
    if pan < 0.0 { (near, far) } else { (far, near) }
}

pub fn pan_gains(pan: f64) -> (f32, f32) {
    let (left, right) = pan_db(pan);
    (
        10f64.powf(left / 20.0) as f32,
        10f64.powf(right / 20.0) as f32,
    )
}

impl AudioPiece {
    pub fn samples(&self) -> i64 {
        match self {
            Self::Source { samples, .. } | Self::Silence { samples } => *samples,
        }
    }
}

impl RenderPlan {
    pub fn broken_composition(&self) -> Option<&str> {
        self.video
            .iter()
            .flat_map(VideoPiece::layers)
            .flat_map(|(layer, _)| layer.placed())
            .find(|p| {
                p.picture
                    .composition
                    .as_ref()
                    .is_some_and(|c| c.chain().is_none())
            })
            .map(|p| p.clip.as_str())
    }

    pub fn has_lut(&self) -> bool {
        self.output_lut.is_some()
            || self
                .video
                .iter()
                .flat_map(VideoPiece::layers)
                .any(|(p, _)| {
                    matches!(
                        p,
                        VideoPiece::Source {
                            input_lut: Some(_),
                            ..
                        }
                    )
                })
    }

    pub fn samples(&self) -> i64 {
        self.fps.frame_to_sample(self.frames, self.sample_rate)
    }
}

/// How a picture piece walks its source (decision D48): frame `k` of it
/// shows source frame `source_frame + offset(k)`, by the sampling rule
/// (export.md §2): at speed 2 every other frame, at 0.5 each twice, at -1
/// backward, at 0 the first held. Each frame shows the source frame its
/// stretch of the source starts in, counted forward: backward, the bottom
/// of the stretch, as editors show it. Each section of the clip's speed
/// keeps this rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Pace {
    /// The sections of the clip's speed.
    pub map: Arc<[Section]>,
    /// Frames of its clip's retimed source before the piece: where in its
    /// pace it starts.
    pub skip: i64,
}

static NORMAL: std::sync::LazyLock<Arc<[Section]>> =
    std::sync::LazyLock::new(|| crate::model::Speed::constant(1.0).sections);

impl Default for Pace {
    fn default() -> Self {
        Self::of(1.0, 0)
    }
}

const FRAME_SLACK: f64 = 0.002;

pub fn source_frame(x: f64) -> i64 {
    (x + FRAME_SLACK).floor() as i64
}

impl Pace {
    pub fn of(speed: f64, skip: i64) -> Self {
        if speed == 1.0 {
            return Self::mapped(NORMAL.clone(), skip);
        }
        Self::mapped(crate::model::Speed::constant(speed).sections, skip)
    }

    pub fn mapped(map: Arc<[Section]>, skip: i64) -> Self {
        Self { map, skip }
    }

    pub fn is_normal(&self) -> bool {
        matches!(&*self.map, [s] if s.speed == 1.0)
    }

    fn section(&self, u: i64) -> &Section {
        let i = self.map.partition_point(|s| s.at <= u);
        &self.map[i.saturating_sub(1)]
    }

    fn index(&self, x: f64) -> usize {
        self.map
            .partition_point(|s| s.at as f64 <= x)
            .saturating_sub(1)
    }

    fn linear(&self, i: usize, x: f64) -> f64 {
        let s = &self.map[i];
        s.from + (x - s.at as f64) * s.speed
    }

    fn eased(&self, i: usize) -> bool {
        i > 0 && self.map.get(i).is_some_and(|s| s.ease.is_some())
    }

    pub fn bend(&self, i: usize) -> Option<[f64; 2]> {
        if !self.eased(i) {
            return None;
        }
        let s = &self.map[i];
        let [a, c] = s.ease?;
        let a = if i > 1 {
            a.max((self.map[i - 1].at - s.at) as f64)
        } else {
            a
        };
        let after = self.map.get(i + 1).map_or(f64::INFINITY, |n| {
            let gap = (n.at - s.at) as f64;
            n.ease.map_or(gap, |[b, _]| gap + b.max(-gap))
        });
        Some([a, c.min(after)])
    }

    fn bent(&self, i: usize, x: f64) -> Option<f64> {
        let [a, c] = self.bend(i)?;
        let s = &self.map[i];
        let k = s.at as f64;
        if x <= k + a || x >= k + c {
            return None;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..60 {
            let t: f64 = (lo + hi) / 2.0;
            if a * (1.0 - t).powi(3) + c * t.powi(3) < x - k {
                lo = t;
            } else {
                hi = t;
            }
        }
        let t: f64 = (lo + hi) / 2.0;
        let head = self.linear(i - 1, k + a) - s.from;
        Some(s.from + head * (1.0 - t).powi(3) + c * s.speed * t.powi(3))
    }

    pub fn position(&self, x: f64) -> f64 {
        let i = self.index(x);
        self.bent(i, x)
            .or_else(|| self.bent(i + 1, x))
            .unwrap_or_else(|| self.linear(i, x))
    }

    fn bends(&self, u: i64) -> bool {
        let i = self.index(u as f64);
        [i, i + 1].into_iter().any(|j| {
            self.bend(j).is_some_and(|[a, c]| {
                let k = self.map[j].at as f64;
                (k + a).floor() as i64 <= u && u < (k + c).ceil() as i64
            })
        })
    }

    fn top(&self, u: i64) -> f64 {
        let s = self.section(u);
        if s.speed < 0.0 && self.map.len() == 1 {
            return s.from + (u + 1 - s.at) as f64 * s.speed;
        }
        self.position(u as f64)
    }

    fn frame(&self, u: i64) -> i64 {
        source_frame(self.top(u))
    }

    pub fn speed(&self) -> f64 {
        self.section(self.skip).speed
    }

    pub fn shown(&self) -> i64 {
        self.frame(self.skip)
    }

    pub fn speed_at(&self, k: i64) -> f64 {
        self.section(self.skip + k).speed
    }

    pub fn rate(&self, k: i64) -> f64 {
        let u = self.skip + k;
        if self.bends(u) {
            return self.position((u + 1) as f64) - self.position(u as f64);
        }
        self.section(u).speed
    }

    pub fn steady(&self, k: i64) -> Option<i64> {
        let u = self.skip + k;
        if self.bends(u) {
            return Some(1);
        }
        let (j, s) = self
            .map
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, s)| s.at > u)?;
        let lo = self
            .bend(j)
            .map_or(s.at, |[a, _]| (s.at as f64 + a).floor() as i64);
        Some(lo.max(u + 1) - u)
    }

    pub fn offset(&self, k: i64) -> i64 {
        if self.is_normal() {
            return k;
        }
        self.frame(self.skip + k) - self.frame(self.skip)
    }

    pub fn exact(&self, k: i64) -> f64 {
        if self.is_normal() {
            return k as f64;
        }
        self.top(self.skip + k) - self.frame(self.skip) as f64
    }

    pub fn after(&self, k: i64) -> Self {
        Self::mapped(self.map.clone(), self.skip + k)
    }

    pub fn phase(&self) -> f64 {
        let s = self.section(self.skip);
        if self.is_normal() || s.speed == 0.0 {
            return 0.0;
        }
        (self.top(self.skip) - self.frame(self.skip) as f64).max(0.0)
    }

    pub fn sound(&self, x: f64) -> f64 {
        if let [s] = &*self.map {
            let top = if s.speed < 0.0 { -s.speed } else { 0.0 };
            return self.phase() + top + x * s.speed;
        }
        self.position(self.skip as f64 + x) - self.frame(self.skip) as f64
    }

    pub fn sound_in(&self, k: i64, x: f64) -> f64 {
        let u = self.skip + k;
        if self.bends(u) {
            let at = self.position(u as f64) + (self.skip as f64 + x - u as f64) * self.rate(k);
            return at - self.frame(self.skip) as f64;
        }
        let s = self.section(u);
        s.from + (self.skip as f64 + x - s.at as f64) * s.speed - self.frame(self.skip) as f64
    }
}

impl VideoPiece {
    pub fn position(&self, k: i64) -> i64 {
        match self {
            VideoPiece::Source {
                source_frame, pace, ..
            } => source_frame + pace.offset(k),
            VideoPiece::Black { .. } | VideoPiece::Generated { .. } => 0,
            VideoPiece::Flat { pieces, skip, .. } => {
                let last = pieces.iter().map(VideoPiece::frames).sum::<i64>() - 1;
                (skip + k).min(last).max(0)
            }
            VideoPiece::Mix { under, .. } | VideoPiece::Titled { under, .. } => under.position(k),
        }
    }

    pub fn exact(&self, k: i64) -> f64 {
        match self {
            VideoPiece::Source {
                source_frame, pace, ..
            } => *source_frame as f64 + pace.exact(k),
            VideoPiece::Black { .. } | VideoPiece::Generated { .. } => 0.0,
            VideoPiece::Flat { .. } => self.position(k) as f64,
            VideoPiece::Mix { under, .. } | VideoPiece::Titled { under, .. } => under.exact(k),
        }
    }
}

fn push_video(out: &mut Vec<VideoPiece>, piece: VideoPiece) {
    if piece.frames() <= 0 {
        return;
    }
    match out.last_mut() {
        Some(last) if last.goes_on_as(&piece) => last.lengthen(piece.frames()),
        _ => out.push(piece),
    }
}

fn push_audio(out: &mut Vec<AudioPiece>, piece: AudioPiece) {
    if piece.samples() <= 0 {
        return;
    }
    match (out.last_mut(), &piece) {
        (Some(AudioPiece::Silence { samples }), AudioPiece::Silence { samples: more }) => {
            *samples += more
        }
        (
            Some(AudioPiece::Source {
                media,
                stream,
                source_sample,
                samples,
                gain_db,
                speed,
                fade,
                edges,
                isolation,
                keyed,
                pan,
                equalizer,
                leveler,
                compressor,
                noise_reduction,
                ..
            }),
            AudioPiece::Source {
                media: m2,
                stream: st2,
                source_sample: s2,
                samples: n2,
                gain_db: g2,
                speed: sp2,
                fade: f2,
                edges: x2,
                isolation: i2,
                keyed: k2,
                pan: p2,
                equalizer: e2,
                leveler: l2,
                compressor: c2,
                noise_reduction: nr2,
                ..
            },
        ) if media == m2
            && stream == st2
            && gain_db == g2
            && isolation == i2
            && pan == p2
            && equalizer == e2
            && leveler == l2
            && compressor == c2
            && noise_reduction == nr2
            && keyed
                .as_ref()
                .map(|k| (&k.keys, k.per_frame, k.at + *samples as f64))
                == k2.as_ref().map(|k| (&k.keys, k.per_frame, k.at))
            && *speed == 1.0
            && *sp2 == 1.0
            && fade.is_none()
            && f2.is_none()
            && edges.is_empty()
            && x2.is_empty()
            && *source_sample + *samples == *s2 =>
        {
            *samples += n2
        }
        _ => out.push(piece),
    }
}

impl Project {
    pub(crate) fn media_origin(&self, media: &str) -> Result<(PathBuf, f64)> {
        Ok(origin_of(found(self.media(media), media)?))
    }

    pub fn media_image(&self, media: &str) -> Option<Image> {
        image_of(self.media(media)?)
    }
}

fn found<'a>(m: Option<&'a Media>, id: &str) -> Result<&'a Media> {
    m.ok_or_else(|| Error::not_found(format!("no media {id}")))
}

fn origin_of(m: &Media) -> (PathBuf, f64) {
    (m.path.clone(), m.origin())
}

pub fn retime_of(c: &Clip, t: i64, m: &Media, s: &Settings) -> Retime {
    retime_at(c.retime, c.speed_at(t), m, s)
}

pub fn retime_at(chosen: ClipRetime, speed: f64, m: &Media, s: &Settings) -> Retime {
    let lands = m.info.video.as_ref().is_some_and(|v| {
        let step = speed * v.fps.as_f64() / s.fps.as_f64();
        (step - step.round()).abs() < 1e-9
    });
    if lands || m.is_still() {
        return Retime::Nearest;
    }
    let slow = speed > 0.0 && speed < 1.0;
    let auto = match slow {
        true => Retime::ai_here(),
        false => Retime::Nearest,
    };
    chosen
        .process()
        .or_else(|| s.retime.process())
        .unwrap_or(auto)
}

pub fn super_scale_of(m: MediaSuperScale, s: &Settings) -> SuperScale {
    SuperScale {
        on: m.on,
        sharpness: m.sharpness,
        upscaler: m.upscaler.upscaler().unwrap_or(s.upscaler.upscaler()),
    }
}

fn image_of(m: &Media) -> Option<Image> {
    let v = m.info.video.as_ref()?;
    match &m.sequence {
        Some(s) => Some(Image::Sequence(Box::new(Numbered {
            pattern: m.path.parent().unwrap_or(Path::new("")).join(&s.pattern),
            first: s.first,
            count: s.count,
            rate: v.fps,
        }))),
        None if v.still => Some(Image::Still),
        None => None,
    }
}

pub(crate) fn plays_inside(c: &Clip) -> bool {
    c.compound.as_ref().is_some_and(|k| !k.tracks.is_empty())
}

#[derive(Debug, Clone)]
struct Edge {
    frames: (i64, i64),
    falling: bool,
    smooth: bool,
    crossing: Option<Placed>,
    modes: (i64, Vec<CompositeMode>),
}

impl Edge {
    fn of(
        p: &Project,
        d: &transitions::Playing,
        falling: bool,
        (side, over, inside): (Option<&Clip>, (i64, i64), bool),
    ) -> Self {
        let modes = (over.0..over.1)
            .map(|t| side.map_or(CompositeMode::Normal, |c| mode_at(p, c, t)))
            .collect();
        Self {
            frames: span_of(d, falling),
            falling,
            smooth: matches!(
                transitions::kind_of(d.transition),
                Some(PictureTransition::SmoothCut { .. })
            ),
            crossing: crossing(d, inside),
            modes: (over.0, modes),
        }
    }

    fn ramp(&self, now: i64, held: bool) -> Ramp {
        Ramp {
            at: now - self.frames.0,
            of: self.frames.1 - self.frames.0,
            falling: self.falling,
            held,
            moved: self.crossing.as_ref().is_some_and(|c| c.moving.is_some()),
        }
    }

    fn crossed(&self, x: i64, ramp: Ramp) -> Option<Placed> {
        let mut c = self.crossing.as_ref()?.after(x - self.frames.0);
        let (from, modes) = &self.modes;
        c.picture.composite_mode = modes
            .get((self.frames.0 + ramp.at - from) as usize)
            .copied()
            .filter(|m| *m != CompositeMode::Normal);
        Some(Placed {
            ramp: ramp.moved.then_some(Ramp {
                falling: false,
                ..ramp
            }),
            ..c
        })
    }
}

#[derive(Debug, Clone)]
struct Around {
    placed: Placed,
    held: Option<i64>,
    edges: Vec<Edge>,
    opened: usize,
}

impl Around {
    fn at(&self, x: i64) -> Vec<Placed> {
        let now = self.held.unwrap_or(x);
        let mut own = self.placed.after(x);
        let edge = self
            .edges
            .iter()
            .find(|e| e.frames.0 <= now && now < e.frames.1);
        let Some(edge) = edge else {
            return vec![own];
        };
        let ramp = edge.ramp(now, self.held.is_some());
        own.ramp = Some(ramp);
        let crossed = edge.crossed(x, ramp);
        if crossed.is_some() {
            own.picture.composite_mode = None;
        }
        std::iter::once(own).chain(crossed).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Fading {
    frames: (i64, i64),
    falling: bool,
    curve: Curve,
}

#[derive(Debug, Clone, Default)]
struct Nest {
    outer: Vec<Around>,
    run: Option<Run>,
    flat: Option<Box<Clip>>,
    opened: usize,
}

fn run_of(c: &Clip, run: Option<Run>) -> Run {
    run.unwrap_or(Run {
        at: -c.start,
        of: c.duration,
        held: false,
    })
}

#[derive(Debug, Clone, Copy)]
struct Shown {
    from: i64,
    to: i64,
    held: Option<i64>,
}

const EVERY_FRAME: Shown = Shown {
    from: i64::MIN,
    to: i64::MAX,
    held: None,
};

#[derive(Debug, Clone)]
struct Inner {
    nests: Vec<Nest>,
    shown: Shown,
    fades: Vec<Fading>,
}

type Within = Vec<Inner>;

pub const NESTED: usize = 8;

type Nesting<'a> = (Option<&'a Nest>, &'a [Placed]);

fn visible(t: &Track, c: &Clip) -> bool {
    t.video_enabled && c.enabled && c.video && (!c.is_block() || c.draws())
}

fn titled(t: &Track, c: &Clip) -> bool {
    t.video_enabled && c.enabled && c.title.is_some()
}

fn pictures_at(c: &Clip, t: i64) -> Vec<&Clip> {
    c.compound
        .iter()
        .flat_map(|k| &k.tracks)
        .filter_map(|n| {
            let x = &n.clips[n.clip_covering(t)?];
            shown_inside(n, x, t).then_some(x)
        })
        .collect()
}

fn shown_inside(n: &Track, x: &Clip, t: i64) -> bool {
    let holds = n.video_enabled && x.enabled && !pictures_at(x, x.source_at(t)).is_empty();
    visible(n, x) || titled(n, x) || holds
}

fn crosses_inside(p: &Project, c: &Clip, t: i64) -> bool {
    c.compound
        .iter()
        .flat_map(|k| &k.tracks)
        .filter(|n| !n.transitions.is_empty())
        .any(|n| {
            transitions::playing(p, n, false, transitions::mixes)
                .iter()
                .any(|d| {
                    let side = |x: &Option<&Clip>| x.is_some_and(|x| shown_inside(n, x, t));
                    d.frames.0 <= t && t < d.frames.1 && (side(&d.before) || side(&d.after))
                })
        })
}

fn mode_at(p: &Project, c: &Clip, t: i64) -> CompositeMode {
    let own = c
        .picture
        .as_ref()
        .map_or(CompositeMode::Normal, |x| x.mode());
    let inner = c.source_at(t);
    match pictures_at(c, inner).as_slice() {
        [only] if own == CompositeMode::Normal && !crosses_inside(p, c, inner) => {
            mode_at(p, only, inner)
        }
        _ => own,
    }
}

fn widened(c: &Clip, (from, to): (i64, i64)) -> Clip {
    let mut wide = c.clone();
    wide.move_head(-(c.start - from).max(0));
    wide.duration += (to - c.end()).max(0);
    wide
}

fn without_mode(c: &Clip) -> Option<Clip> {
    let moded = c.picture.as_ref()?.composite_mode.is_some();
    moded.then(|| {
        let mut bare = c.clone();
        if let Some(p) = &mut bare.picture {
            p.composite_mode = None;
        }
        bare
    })
}

fn crossing(d: &transitions::Playing, inside: bool) -> Option<Placed> {
    let moving = Move::of(d);
    let joins = inside
        || moving.is_some()
        || transitions::dip_of(d.transition).is_some()
        || (d.before.is_some() && d.after.is_some());
    joins.then(|| Placed {
        clip: d.transition.id.clone(),
        picture: Picture::default(),
        run: None,
        source: 0,
        frame: 0,
        start: 0,
        ready: true,
        ramp: None,
        crossing: true,
        moving,
        followed: Followed::default(),
    })
}

fn span_of(d: &transitions::Playing, falling: bool) -> (i64, i64) {
    let mid = middle(d.frames);
    match (transitions::dip_of(d.transition).is_some(), falling) {
        (false, _) => d.frames,
        (true, true) => (d.frames.0, mid),
        (true, false) => (mid, d.frames.1),
    }
}

fn sides_at<'a>(d: &transitions::Playing<'a>, now: i64) -> Vec<(Option<&'a Clip>, bool)> {
    [(d.before, true), (d.after, false)]
        .into_iter()
        .filter(|(_, falling)| {
            let (from, to) = span_of(d, *falling);
            from <= now && now < to
        })
        .collect()
}

fn edges_on(p: &Project, t: &Track) -> HashMap<String, Vec<Edge>> {
    let mut edges: HashMap<String, Vec<Edge>> = HashMap::new();
    if !t.clips.iter().any(plays_inside) {
        return edges;
    }
    for d in transitions::playing(p, t, false, joins_compound) {
        for (side, falling) in [(d.before, true), (d.after, false)] {
            let Some(c) = side.filter(|c| plays_inside(c)) else {
                continue;
            };
            edges.entry(c.id.clone()).or_default().push(Edge::of(
                p,
                &d,
                falling,
                (d.before.or(d.after), span_of(&d, falling), false),
            ));
        }
    }
    edges
}

fn edge_fades(p: &Project) -> HashMap<String, Vec<Fading>> {
    let mut fades: HashMap<String, Vec<Fading>> = HashMap::new();
    for t in p.tracks.iter().filter(|t| t.clips.iter().any(plays_inside)) {
        for d in transitions::cross_fades(p, t) {
            let Some(curve) = transitions::cross_fade(d.transition) else {
                continue;
            };
            for (side, falling) in [(d.before, true), (d.after, false)] {
                let Some(c) = side.filter(|c| plays_inside(c)) else {
                    continue;
                };
                let blocks = match c.video {
                    true => crate::otio::tied(p, &c.id),
                    false => vec![c.id.clone()],
                };
                let fading = Fading {
                    frames: d.frames,
                    falling,
                    curve,
                };
                for block in blocks {
                    let held = fades.entry(block).or_default();
                    if !held.contains(&fading) {
                        held.push(fading);
                    }
                }
            }
        }
    }
    fades
}

fn joins_compound(t: &Transition) -> bool {
    transitions::kind_of(t).is_some()
}

fn touches_compound(t: &Track, x: &Transition) -> bool {
    [&x.from, &x.to]
        .into_iter()
        .flatten()
        .any(|id| t.clips.iter().any(|c| &c.id == id && plays_inside(c)))
}

fn nest_in<'a>(inner: Option<&'a Inner>, t: &Track, c: &Clip) -> Option<&'a Nest> {
    inner?
        .nests
        .get(t.clips.iter().position(|x| std::ptr::eq(x, c))?)
}

fn whole<'a>(nest: Option<&'a Nest>, c: &'a Clip) -> &'a Clip {
    nest.and_then(|n| n.flat.as_deref()).unwrap_or(c)
}

fn hollow(p: &Project, c: &Clip) -> Clip {
    let mut hollow = c.clone();
    hollow.compound = c.compound.as_ref().map(|k| Compound {
        stack: Default::default(),
        frames: k.frames,
        tracks: Vec::new(),
        made_from: Vec::new(),
    });
    let mode = mode_at(p, c, c.end() - 1);
    if mode != CompositeMode::Normal {
        hollow.picture.get_or_insert_default().composite_mode = Some(mode);
    }
    hollow
}

fn reached(c: &Clip, sides: impl Iterator<Item = ((i64, i64), bool)>) -> (i64, i64) {
    sides.fold(
        (c.start, c.end()),
        |(from, to), ((a, b), falling)| match falling {
            true => (from, b),
            false => (a, to),
        },
    )
}

fn split_at_fades(runs: Vec<(i64, i64)>, fades: &[Fading]) -> Vec<(i64, i64)> {
    let marks: Vec<i64> = fades
        .iter()
        .flat_map(|f| [f.frames.0, f.frames.1])
        .collect();
    let mut out = Vec::new();
    for (mut x, y) in runs {
        let mut inside: Vec<i64> = marks.iter().copied().filter(|m| x < *m && *m < y).collect();
        inside.sort_unstable();
        inside.dedup();
        for m in inside {
            out.push((x, m));
            x = m;
        }
        out.push((x, y));
    }
    out
}

fn expanded(p: &Project) -> Option<(Project, Within)> {
    let holds = |p: &Project| p.tracks.iter().any(|t| t.clips.iter().any(plays_inside));
    if !holds(p) {
        return None;
    }
    let mut out = p.clone();
    let mut within: Within = out
        .tracks
        .iter()
        .map(|t| Inner {
            nests: vec![Nest::default(); t.clips.len()],
            shown: EVERY_FRAME,
            fades: Vec::new(),
        })
        .collect();
    let mut opened = 0;
    for _ in 0..NESTED {
        if !holds(&out) {
            break;
        }
        let (mut tracks, mut placed) = (Vec::new(), Vec::new());
        let fades = edge_fades(&out);
        for (t, w) in out.tracks.iter().zip(&within) {
            let edges = edges_on(&out, t);
            let (mut host, mut host_within) = (t.clone(), Vec::new());
            host.clips.clear();
            host.transitions
                .retain(|x| x.sound || joins_compound(x) || !touches_compound(t, x));
            let mut inner = Vec::new();
            for (c, nest) in t.clips.iter().zip(&w.nests) {
                if !plays_inside(c) {
                    host.clips.push(c.clone());
                    host_within.push(nest.clone());
                    continue;
                }
                if !c.enabled {
                    continue;
                }
                host.clips.push(hollow(p, c));
                opened += 1;
                let own = c.picture.clone().unwrap_or_default();
                let edges = edges.get(&c.id).cloned().unwrap_or_default();
                let smooth = edges.iter().any(|e| e.smooth);
                let crossed = !edges.is_empty();
                let fading = fades.get(&c.id).cloned().unwrap_or_default();
                let (from, to) = reached(
                    c,
                    edges
                        .iter()
                        .map(|e| (e.frames, e.falling))
                        .chain(fading.iter().map(|f| (f.frames, f.falling))),
                );
                let chain: Vec<Around> = std::iter::once(Around {
                    placed: Placed::of(c, own, run_of(c, nest.run)),
                    held: w.shown.held,
                    edges,
                    opened,
                })
                .chain(nest.outer.iter().cloned())
                .collect();
                let (head, tail) = ((c.start - from).max(0), (to - c.end()).max(0));
                let wide = widened(c, (c.start - head, c.end() + tail));
                let wide = &wide;
                host_within.push(match smooth {
                    true => Nest {
                        outer: nest.outer.clone(),
                        run: nest.run,
                        flat: Some(Box::new(wide.clone())),
                        opened,
                    },
                    false => Nest {
                        flat: crossed.then(|| Box::new(wide.clone())),
                        ..Nest::default()
                    },
                });
                let k = c.compound.as_ref().expect("plays inside");
                let cut = stretches(wide, t.kind);
                let parts = if cut.is_empty() {
                    std::slice::from_ref(wide)
                } else {
                    cut.as_slice()
                };
                let inner_tracks = k.tracks.iter().filter(|n| n.kind == t.kind);
                let playing = parts
                    .iter()
                    .flat_map(|part| inner_tracks.clone().map(move |n| (part, n)));
                for (part, n) in playing {
                    let mut v = n.clone();
                    v.id = t.id.clone();
                    v.duck = None;
                    v.muted |= t.muted;
                    v.solo = t.solo;
                    v.volume_db = t.volume_db;
                    v.video_enabled &= t.video_enabled;
                    v.render_cache &= t.render_cache;
                    let laid = inner_layout(p, n, part);
                    v.transitions = laid.transitions;
                    let mut clips: Vec<(Clip, Nest)> = laid
                        .clips
                        .into_iter()
                        .map(|(y, run)| {
                            let nest = Nest {
                                outer: chain.clone(),
                                run: Some(run),
                                flat: None,
                                opened: 0,
                            };
                            (y, nest)
                        })
                        .collect();
                    clips.sort_by_key(|(y, _)| y.start);
                    let (clips, nests) = clips.into_iter().unzip();
                    v.clips = clips;
                    let held = laid.held.then_some(part.start).or(w.shown.held);
                    let shown = match w.shown.held {
                        Some(_) => Shown { held, ..w.shown },
                        None => Shown {
                            from: w.shown.from.max(from).max(part.start),
                            to: w.shown.to.min(to).min(part.end()),
                            held,
                        },
                    };
                    let fades: Vec<Fading> = w.fades.iter().chain(&fading).copied().collect();
                    inner.push((
                        v,
                        Inner {
                            nests,
                            shown,
                            fades,
                        },
                    ));
                }
            }
            tracks.push(host);
            placed.push(Inner {
                nests: host_within,
                shown: w.shown,
                fades: w.fades.clone(),
            });
            for (v, chains) in inner {
                tracks.push(v);
                placed.push(chains);
            }
        }
        out.tracks = tracks;
        within = placed;
    }
    Some((out, within))
}

pub(crate) fn stretches(c: &Clip, kind: TrackKind) -> Vec<Clip> {
    if c.constant_speed().is_some() {
        return Vec::new();
    }
    let pace = c.pace();
    let mut spans: Vec<(i64, i64)> = Vec::new();
    let mut way = 0.0;
    for (a, b) in c.runs(c.start, c.end()) {
        let v = pace.rate(a - c.start);
        match spans.last_mut() {
            Some((_, end)) if kind == TrackKind::Audio && v * way >= 0.0 => *end = b,
            _ => {
                spans.push((a, b));
                way = 0.0;
            }
        }
        if v != 0.0 {
            way = v;
        }
    }
    spans
        .into_iter()
        .map(|(a, b)| {
            let mut part = c.clone();
            part.move_head(a - c.start);
            part.duration = b - a;
            part
        })
        .collect()
}

struct Laid {
    clips: Vec<(Clip, Run)>,
    transitions: Vec<Transition>,
    held: bool,
}

fn inner_layout(p: &Project, n: &Track, c: &Clip) -> Laid {
    let cuts = transitions::Cuts::of(p, n);
    let (first, last) = c.shown();
    let reached: Vec<(&Transition, i64)> = n
        .transitions
        .iter()
        .filter_map(|t| Some((t, cuts.place(p, t)?)))
        .filter(|(t, cut)| cut - t.before < last && cut + t.after > first)
        .collect();
    let pace = c.pace();
    let rates: Vec<f64> = c
        .runs(c.start, c.end())
        .iter()
        .map(|(a, _)| pace.rate(a - c.start))
        .collect();
    let speed = rates.iter().copied().find(|v| *v != 0.0).unwrap_or(0.0);
    if speed == 0.0 {
        return match reached.first().filter(|_| n.kind == TrackKind::Video) {
            Some(&(t, cut)) => held_on(n, c, t, cut),
            None => Laid {
                clips: n.clips.iter().filter_map(|x| inside(x, c)).collect(),
                transitions: Vec::new(),
                held: false,
            },
        };
    }
    let (lo, hi) = reached.iter().fold((first, last), |(lo, hi), (t, cut)| {
        (lo.min(cut - t.before), hi.max(cut + t.after))
    });
    let room =
        |frames: i64, part: f64, rate: Option<&f64>| match (frames, rate.copied().unwrap_or(speed))
        {
            (0, _) | (_, 0.0) => 0,
            (n, v) => ((n as f64 + part) / v.abs()).ceil() as i64,
        };
    let (early, late) = (rates.first(), rates.last());
    let (near, far) = match n.kind {
        TrackKind::Video if speed > 0.0 => {
            let past = |k: i64| {
                let u = pace.skip + k;
                let rest = pace.position(u as f64) - pace.frame(u) as f64;
                if rest > FRAME_SLACK { rest } else { 0.0 }
            };
            let step = late.copied().unwrap_or(speed);
            (past(0), (1.0 - past(c.duration - 1) - step).max(0.0))
        }
        _ => (0.0, 0.0),
    };
    let (head, tail) = match speed > 0.0 {
        true => (room(first - lo, near, early), room(hi - last, far, late)),
        false => (room(hi - last, 0.0, early), room(first - lo, 0.0, late)),
    };
    let wide = (head > 0 || tail > 0).then(|| {
        let mut wide = c.clone();
        if n.kind == TrackKind::Video {
            let other = |k: i64| pace.speed_at(k) != pace.speed_at(0);
            at_own_speed(
                &mut wide,
                (1..=head).any(|k| other(-k)),
                (0..tail).any(|k| other(c.duration + k)),
                c.duration + tail,
            );
        }
        wide.move_head(-head);
        wide.duration += tail;
        wide
    });
    let wide = wide.as_ref().unwrap_or(c);
    let shown = |frames: i64| (frames as f64 / speed.abs()).floor() as i64;
    let seen_in = |a: i64, b: i64| {
        (0..wide.duration)
            .filter(|&k| (a..b).contains(&wide.source_at(wide.start + k)))
            .count() as i64
    };
    Laid {
        clips: n.clips.iter().filter_map(|x| inside(x, wide)).collect(),
        transitions: reached
            .into_iter()
            .map(|(t, cut)| {
                let mut t = t.clone();
                (t.before, t.after) = if rates.len() > 1 {
                    (seen_in(cut - t.before, cut), seen_in(cut, cut + t.after))
                } else {
                    (shown(t.before), shown(t.after))
                };
                if speed < 0.0 {
                    std::mem::swap(&mut t.from, &mut t.to);
                    std::mem::swap(&mut t.before, &mut t.after);
                }
                t
            })
            .collect(),
        held: false,
    }
}

fn at_own_speed(c: &mut Clip, back: bool, on: bool, reach: i64) {
    let pace = c.pace();
    let Some(s) = c.speed.as_mut().filter(|_| back || on) else {
        return;
    };
    let own = Section {
        at: s.skip,
        from: pace.position(s.skip as f64),
        speed: pace.rate(0),
        ease: None,
    };
    let earlier = s.sections.iter().filter(|x| !back && x.at < own.at);
    let later = s.sections.iter().filter(|x| !on && x.at > own.at);
    let mut sections: Vec<Section> = earlier.chain([&own]).chain(later).copied().collect();
    if sections.len() == 1 {
        sections.push(Section {
            at: own.at + reach,
            from: own.from + reach as f64 * own.speed,
            ..own
        });
    }
    s.sections = sections.into();
}

fn held_on(n: &Track, c: &Clip, t: &Transition, cut: i64) -> Laid {
    let on = c.source_at(c.start);
    let a = c.start - (on - (cut - t.before));
    let m = a + t.before;
    let side = |id: &Option<String>, (s, e): (i64, i64)| {
        let x = n.clips.iter().find(|x| Some(&x.id) == id.as_ref())?;
        let mut at = c.clone();
        (at.start, at.duration) = (s, e - s);
        if let Some(speed) = at.speed.as_mut() {
            speed.sections = crate::model::Speed::constant(0.0).sections;
        }
        seen(x, &at, (on, on + 1))
    };
    let clips = [
        side(&t.from, (a.min(m - 1), m)),
        side(&t.to, (m, (m + t.after).max(m + 1))),
    ];
    Laid {
        clips: clips.into_iter().flatten().collect(),
        transitions: vec![t.clone()],
        held: true,
    }
}

fn flat_pieces(
    p: &Project,
    t: &Track,
    whole: &Clip,
    frames: (i64, i64),
) -> Result<Arc<[VideoPiece]>> {
    let mut clip = whole.clone();
    clip.audio = None;
    let mut track = t.clone();
    track.clips = vec![clip];
    track.transitions.clear();
    let mut alone = p.clone();
    alone.tracks = vec![track];
    let (plan, _) = plan_lanes(&alone, frames.0, frames.1)?;
    Ok(plan.video.into())
}

fn outer_at(nest: Option<&Nest>, x: i64) -> Vec<Placed> {
    nest.iter()
        .flat_map(|n| &n.outer)
        .flat_map(|a| a.at(x))
        .collect()
}

fn placed_of(
    c: &Clip,
    nest: Option<&Nest>,
    fills: bool,
    crossed: &[Placed],
    x: i64,
) -> Vec<Placed> {
    let outer = outer_at(nest, x);
    let own = c.picture.clone().filter(|x| !x.is_usual());
    if own.is_none() && outer.is_empty() && fills && crossed.is_empty() {
        return Vec::new();
    }
    let run = run_of(c, nest.and_then(|n| n.run)).after(x);
    let own = Placed::at(c, own.unwrap_or_default(), run, x);
    std::iter::once(own)
        .chain(crossed.iter().cloned())
        .chain(outer)
        .collect()
}

pub(crate) fn inside(x: &Clip, c: &Clip) -> Option<(Clip, Run)> {
    seen(x, c, (x.start, x.end()))
}

struct PaceStep {
    at: i64,
    end: i64,
    source: i64,
    rate: f64,
}

fn paced_keys(y: &mut Clip, c: &Clip, k0: i64, first_key: i64) {
    let pace = c.pace();
    let steps: Vec<PaceStep> = c
        .runs(c.start, c.end())
        .into_iter()
        .map(|(a, b)| PaceStep {
            at: a - c.start,
            end: b - c.start,
            source: c.source_at(a),
            rate: pace.rate(a - c.start),
        })
        .collect();
    let moving: Vec<&PaceStep> = steps.iter().filter(|s| s.rate != 0.0).collect();
    let (Some(first), Some(last)) = (moving.first(), moving.last()) else {
        let held = first_key as f64;
        y.picture = y.picture.take().map(|p| p.sampled(held));
        y.title = y.title.take().map(|t| t.sampled(held));
        return;
    };
    let forward = first.rate > 0.0;
    let tail = c.duration - last.end;
    let from = c.source_at(c.start + k0) - first_key;
    let to = y.content_at(y.start) - k0;
    let past = |q: i64| {
        moving.partition_point(|s| {
            if forward {
                s.source <= q
            } else {
                s.source >= q
            }
        })
    };
    let section = |q: i64| moving[past(q).saturating_sub(1)];
    let conv = |k: i64| {
        let q = from + k;
        if let Some(s) = steps.iter().find(|s| s.source == q) {
            return to + s.at;
        }
        let s = section(q);
        let at = if past(q) == 0 { 0 } else { s.at };
        let t = at + ((q - s.source) as f64 / s.rate).round() as i64;
        to + if t > last.end { t + tail } else { t }
    };
    let ratio = |k: i64| 1.0 / section(from + k).rate;
    let splits: Vec<i64> = steps[1..].iter().map(|s| s.source - from).collect();
    let holds: Vec<(i64, i64)> = steps
        .iter()
        .filter(|s| s.rate == 0.0)
        .map(|s| (to + s.at, to + s.end))
        .collect();
    let pace_picture = |picture: &mut Picture| {
        for &k in &splits {
            picture.split_keys(k);
        }
        picture.scale_times(&conv, &ratio);
        for &(at, until) in &holds {
            picture.hold_keys(at, until);
        }
    };
    if let Some(picture) = y.picture.as_mut() {
        pace_picture(picture);
    }
    if let Some(title) = y.title.as_mut().filter(|t| t.is_keyed()) {
        title.each_line(|line| {
            let mut picture = line.as_picture();
            pace_picture(&mut picture);
            line.move_handles(conv);
            line.take_picture(&picture);
        });
    }
    if let Some(a) = y.audio.as_mut() {
        for &k in &splits {
            crate::keyframes::split_inside(&mut a.gain_keys, k);
        }
        a.gain_keys = crate::keyframes::scaled_by(&a.gain_keys, conv, &ratio);
        for &(at, until) in &holds {
            crate::keyframes::hold(&mut a.gain_keys, at, until);
        }
    }
}

fn seen(x: &Clip, c: &Clip, (lo, hi): (i64, i64)) -> Option<(Clip, Run)> {
    if c.speed.is_some() {
        let speed = c.pace().rate(0);
        let (k0, k1) = shown_between(c, lo, hi)?;
        let mut y = x.clone();
        let walked = c.source_at(c.start + k0) - x.start;
        y.walk(walked);
        let first_key = y.content_at(y.start);
        (y.start, y.duration) = (c.start + k0, k1 - k0);
        let own = y.speed.take();
        y.speed = paced(c, own.as_ref(), k0);
        paced_keys(&mut y, c, k0, first_key);
        if let Some(a) = y.audio.as_mut() {
            a.head = 0;
            a.tail = 0;
        }
        let run = if speed == 0.0 {
            Run {
                at: walked,
                of: x.duration,
                held: true,
            }
        } else {
            let of = (x.duration as f64 / speed).round() as i64;
            Run {
                at: (walked as f64 / speed).round() as i64 - y.start,
                of: if of == 0 { speed.signum() as i64 } else { of },
                held: false,
            }
        };
        return Some((y, run));
    }
    let (first, last) = (c.source_in, c.source_in + c.duration);
    let (s, e) = (lo.max(first), hi.min(last));
    if e <= s {
        return None;
    }
    let mut y = x.clone();
    y.walk(s - x.start);
    y.start = c.start + s - first;
    y.duration = e - s;
    if let Some(a) = y.audio.as_mut() {
        if s > x.start || e < x.end() {
            a.head = 0;
            a.tail = 0;
        } else {
            a.head = a.head.max(first - x.start);
            a.tail = a.tail.min(last - x.end());
        }
    }
    let run = Run {
        at: s - x.start - y.start,
        of: x.duration,
        held: false,
    };
    Some((y, run))
}

pub(crate) fn shown_between(c: &Clip, a: i64, b: i64) -> Option<(i64, i64)> {
    let shows = |k: i64| (a..b).contains(&c.source_at(c.start + k));
    let k0 = (0..c.duration).find(|&k| shows(k))?;
    let k1 = (k0..c.duration).find(|&k| !shows(k)).unwrap_or(c.duration);
    Some((k0, k1))
}

pub(crate) fn paced(
    c: &Clip,
    own: Option<&crate::model::Speed>,
    k: i64,
) -> Option<crate::model::Speed> {
    if let Some(s) = c.speed.as_ref().filter(|s| s.scalar().is_none()) {
        return Some(match own {
            Some(own) => composed_over(own, c, k),
            None => crate::model::Speed {
                sections: s.sections.clone(),
                skip: c.pace().after(k).skip,
                effect: None,
            },
        });
    }
    let outer = c.pace().speed();
    if let Some(s) = own.filter(|s| s.scalar().is_none() && outer != 0.0) {
        return Some(composed(s, outer));
    }
    let speed = outer * own.and_then(crate::model::Speed::scalar).unwrap_or(1.0);
    (speed != 1.0).then(|| crate::model::Speed {
        effect: Some(crate::ops::time_effect(own.or(c.speed.as_ref()), speed).into()),
        skip: if own.is_none() {
            c.pace().after(k).skip
        } else {
            0
        },
        ..crate::model::Speed::constant(speed)
    })
}

fn composed(own: &crate::model::Speed, v: f64) -> crate::model::Speed {
    crate::model::Speed {
        sections: composed_from(own, own.skip as f64, v).into(),
        skip: 0,
        effect: None,
    }
}

fn composed_over(own: &crate::model::Speed, c: &Clip, k: i64) -> crate::model::Speed {
    let pace = c.pace();
    let on = |at: i64| own.skip as f64 + pace.exact(at) - pace.exact(k);
    let origin = position_in(&own.sections, on(k));
    let mut sections = Vec::new();
    for (a, b) in c.runs(c.start + k, c.end()) {
        let at = a - c.start;
        let v = pace.rate(at);
        let from = position_in(&own.sections, on(at)) - origin;
        if v == 0.0 {
            sections.push(Section {
                at: at - k,
                from,
                speed: 0.0,
                ease: None,
            });
            continue;
        }
        sections.extend(
            composed_from(own, on(at), v)
                .into_iter()
                .filter(|s| s.at < b - a)
                .map(|s| Section {
                    at: s.at + at - k,
                    from: s.from + from,
                    ..s
                }),
        );
    }
    crate::model::Speed {
        sections: sections.into(),
        skip: 0,
        effect: None,
    }
}

fn position_in(map: &[Section], u: f64) -> f64 {
    let s = &map[map.partition_point(|s| s.at as f64 <= u).saturating_sub(1)];
    s.from + (u - s.at as f64) * s.speed
}

fn composed_from(own: &crate::model::Speed, u0: f64, v: f64) -> Vec<Section> {
    let map = &own.sections;
    let index = |u: f64| map.partition_point(|s| s.at as f64 <= u).saturating_sub(1);
    let pos = |u: f64| position_in(map, u);
    let i0 = index(u0);
    let mut bounds = vec![(0, i0)];
    if v > 0.0 {
        for (i, s) in map.iter().enumerate().skip(i0 + 1) {
            bounds.push((((s.at as f64 - u0) / v - 1e-9).ceil() as i64, i));
        }
    } else {
        for i in (1..=i0).rev() {
            let at = ((u0 - map[i].at as f64) / -v + 1e-9).floor() as i64 + 1;
            bounds.push((at, i - 1));
        }
    }
    bounds.dedup_by(|b, a| {
        let same = a.0 == b.0;
        if same {
            a.1 = b.1;
        }
        same
    });
    bounds
        .into_iter()
        .map(|(j, i)| Section {
            at: j,
            from: pos(u0 + j as f64 * v) - pos(u0),
            speed: map[i].speed * v,
            ease: map[i]
                .ease
                .filter(|_| v > 0.0 && j > 0)
                .map(|e| e.map(|x| x / v)),
        })
        .collect()
}

enum Layer {
    Picture(VideoPiece, Option<Ramp>),
    Smoothed(VideoPiece, Ramp, Smooth),
    Title(Box<ShownTitle>),
}

impl Layer {
    fn inside(&self, outer: &[Placed]) -> bool {
        match self {
            Layer::Picture(piece, _) => {
                let placed = piece.placed();
                placed.len() > outer.len() && placed.ends_with(outer)
            }
            Layer::Title(t) => !outer.is_empty() && t.placed.ends_with(outer),
            _ => false,
        }
    }
}

fn hide_inside(outer: &[Placed], stack: &mut Vec<Layer>) {
    match outer.iter().position(|p| !p.keeps_cover()) {
        Some(n) => stack.retain(|l| !l.inside(&outer[n..])),
        None => stack.clear(),
    }
}

#[derive(Default)]
struct Drawn {
    flat: Vec<usize>,
    hides: Option<Vec<Placed>>,
    layers: Vec<Layer>,
}

fn around(piece: &VideoPiece) -> Vec<Placed> {
    piece.placed().get(1..).unwrap_or_default().to_vec()
}

fn uncovered(drawn: Vec<(&VideoLane, Drawn)>) -> Vec<Layer> {
    let flat: Vec<usize> = drawn.iter().flat_map(|(_, d)| d.flat.clone()).collect();
    let mut stack = Vec::new();
    for (lane, d) in drawn {
        if lane.drawn_flat(&flat) {
            continue;
        }
        if let Some(outer) = d.hides {
            hide_inside(&outer, &mut stack);
        }
        stack.extend(d.layers);
    }
    stack
}

fn middle((start, end): (i64, i64)) -> i64 {
    start + (end - start) / 2
}

pub fn flatten(p: &Project, a: i64, b: i64) -> Result<RenderPlan> {
    flatten_lanes(p, a, b).map(|(plan, _)| plan)
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneTrack {
    pub track: String,
    pub volume_db: f64,
}

pub fn flatten_lanes(p: &Project, a: i64, b: i64) -> Result<(RenderPlan, Vec<LaneTrack>)> {
    let (mut plan, lanes) = plan_lanes(p, a, b)?;
    plan.colors = match p.settings.timeline_colors {
        TimelineColors::Auto if (a, b) == (0, p.end()) => colors_in(p, &plan),
        _ => timeline_colors(p),
    };
    Ok((plan, lanes))
}

fn plan_lanes(p: &Project, a: i64, b: i64) -> Result<(RenderPlan, Vec<LaneTrack>)> {
    if a < 0 || b <= a {
        return Err(Error::invalid(format!("nothing to render in {a}..{b}")));
    }
    let expanded = expanded(p);
    let (p, within) = match &expanded {
        Some((q, w)) => (q, Some(w)),
        None => (p, None),
    };
    let s = &p.settings;
    let video = planned_video(p, within, (a, b))?;
    let (audio, lanes) = planned_audio(p, within, (a, b))?;
    let ducking = crate::duck::planned(p, &lanes, a, b);
    let plan = RenderPlan {
        fps: s.fps,
        width: s.width,
        height: s.height,
        sample_rate: s.sample_rate,
        channels: s.channels,
        frames: b - a,
        video,
        audio,
        ducking,
        range: (a, b),
        name: "timeline".into(),
        colors: None,
        output_lut: s.output_lut.clone(),
    };
    Ok((plan, lanes))
}

struct VideoLane<'a> {
    track: &'a Track,
    inner: Option<&'a Inner>,
    played: Vec<transitions::Playing<'a>>,
}

impl VideoLane<'_> {
    fn window(&self) -> Shown {
        self.inner.map_or(EVERY_FRAME, |i| i.shown)
    }

    fn drawn_flat(&self, flat: &[usize]) -> bool {
        self.inner.is_some_and(|i| {
            i.nests
                .iter()
                .flat_map(|n| &n.outer)
                .any(|a| flat.contains(&a.opened))
        })
    }
}

fn planned_video(
    p: &Project,
    within: Option<&Within>,
    (a, b): (i64, i64),
) -> Result<Vec<VideoPiece>> {
    let lanes: Vec<VideoLane> = p
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TrackKind::Video)
        .map(|(i, t)| VideoLane {
            track: t,
            inner: within.and_then(|w| w.get(i)),
            played: transitions::playing(p, t, false, transitions::mixes),
        })
        .collect();
    let plan = VideoPlan { p, pool: p.pool() };
    let mut video = Vec::new();
    for w in cuts_of(p, &lanes, (a, b)).windows(2) {
        let (x, y) = (w[0], w[1]);
        let mut drawn = Vec::new();
        for lane in &lanes {
            let window = lane.window();
            if x < window.from || x >= window.to {
                continue;
            }
            let span = Span {
                plan: &plan,
                lane,
                x,
                y,
                now: window.held.unwrap_or(x),
                held: window.held.is_some(),
            };
            let mut pictures = span.pictures()?;
            pictures.layers.extend(span.titles());
            drawn.push((lane, pictures));
        }
        push_video(&mut video, piece_of(uncovered(drawn), y - x));
    }
    Ok(video)
}

fn cuts_of(p: &Project, lanes: &[VideoLane], (a, b): (i64, i64)) -> Vec<i64> {
    let mut cuts = vec![a, b];
    for lane in lanes {
        let (t, inner) = (lane.track, lane.inner);
        let shown = lane.window();
        let (from, to) = (a.max(shown.from), b.min(shown.to));
        if to <= from {
            continue;
        }
        cuts.extend([from, to]);
        for c in t
            .clips
            .iter()
            .filter(|c| c.end() > from && c.start < to && (visible(t, c) || titled(t, c)))
        {
            cuts.push(c.start.max(from));
            cuts.push(c.end().min(to));
            for (s, _) in c.runs(c.start.max(from), c.end().min(to)) {
                cuts.push(s);
            }
        }
        for d in &lane.played {
            cuts.extend([d.frames.0, d.frames.1].map(|f| f.clamp(from, to)));
            if transitions::dip_of(d.transition).is_some() {
                cuts.push(middle(d.frames).clamp(from, to));
            }
            if let Some(c) = d.before.or(d.after).map(|c| whole(nest_in(inner, t, c), c)) {
                cuts.extend(
                    ((from + 1).max(d.frames.0 + 1)..to.min(d.frames.1)).filter(|&f| {
                        c.compound.is_some() && mode_at(p, c, f) != mode_at(p, c, f - 1)
                    }),
                );
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

fn covers(piece: &VideoPiece) -> bool {
    piece.placed().is_empty() && !matches!(piece, VideoPiece::Flat { .. })
}

fn piece_of(stack: Vec<Layer>, frames: i64) -> VideoPiece {
    let mut piece = VideoPiece::black(frames);
    for layer in stack {
        piece = match layer {
            Layer::Picture(shown, None) if covers(&shown) => shown,
            Layer::Picture(shown, ramp) => piece.with_over(Over {
                piece: shown,
                ramp,
                smooth: None,
            }),
            Layer::Smoothed(shown, ramp, smooth) => piece.with_over(Over {
                piece: shown,
                ramp: Some(ramp),
                smooth: Some(smooth),
            }),
            Layer::Title(t) => piece.with_title(*t),
        };
    }
    piece
}

struct VideoPlan<'a> {
    p: &'a Project,
    pool: HashMap<&'a str, &'a Media>,
}

impl VideoPlan<'_> {
    fn fills(&self, c: &Clip) -> bool {
        let (pool, s) = (&self.pool, &self.p.settings);
        c.picture.as_ref().is_none_or(Picture::is_usual)
            && (c.draws()
                || pool
                    .get(c.media.as_str())
                    .is_some_and(|m| crate::place::fills(m, (s.width, s.height))))
    }

    fn picture(&self, c: &Clip, (nest, crossed): Nesting, x: i64, y: i64) -> Result<VideoPiece> {
        let (pool, s) = (&self.pool, &self.p.settings);
        let fills = self.fills(c);
        let drawn = c.generator.as_ref();
        if let Some(picture) = drawn.and_then(|g| Generated::of(g, c.content_at(x))) {
            return Ok(VideoPiece::Generated {
                frames: y - x,
                picture,
                placed: placed_of(c, nest, fills, crossed, x),
                excluded: false,
            });
        }
        let m = found(pool.get(c.media.as_str()).copied(), &c.media)?;
        let (path, origin) = origin_of(m);
        Ok(VideoPiece::Source {
            media: c.media.clone(),
            path,
            origin,
            source_frame: c.source_at(x),
            frames: y - x,
            image: image_of(m),
            pace: c.pace().after(x - c.start),
            color_space: m.color_space,
            alpha_mode: m.alpha_mode,
            super_scale: super_scale_of(m.super_scale, s),
            retime: retime_of(c, x, m, s),
            placed: placed_of(c, nest, fills, crossed, x),
            cache: c.render_cache,
            excluded: false,
            made: None,
            input_lut: m.input_lut.clone(),
        })
    }
}

struct Span<'a> {
    plan: &'a VideoPlan<'a>,
    lane: &'a VideoLane<'a>,
    x: i64,
    y: i64,
    now: i64,
    held: bool,
}

impl<'a> Span<'a> {
    fn nest_of(&self, c: &Clip) -> Option<&'a Nest> {
        nest_in(self.lane.inner, self.lane.track, c)
    }

    fn playing(&self) -> Option<&'a transitions::Playing<'a>> {
        self.lane
            .played
            .iter()
            .find(|d| d.frames.0 <= self.now && self.now < d.frames.1)
    }

    fn laid(&self, c: &Clip, nest: Option<&Nest>, crossed: &[Placed]) -> Result<VideoPiece> {
        let mut piece = self.plan.picture(c, (nest, crossed), self.x, self.y)?;
        if let VideoPiece::Source { excluded: e, .. } | VideoPiece::Generated { excluded: e, .. } =
            &mut piece
        {
            *e = !self.lane.track.render_cache;
        }
        Ok(piece)
    }

    fn ramp_of(&self, d: &transitions::Playing, falling: bool) -> Ramp {
        Edge::of(self.plan.p, d, falling, (None, (self.now, self.now), false))
            .ramp(self.now, self.held)
    }

    fn sided(&self, d: &transitions::Playing, falling: bool) -> (Option<Ramp>, Vec<Placed>) {
        let (x, now) = (self.x, self.now);
        let nest_of = |c| self.nest_of(c);
        let side = d.before.or(d.after).map(|c| whole(nest_of(c), c));
        let inside = d
            .before
            .or(d.after)
            .and_then(nest_of)
            .is_some_and(|n| !n.outer.is_empty());
        let edge = Edge::of(self.plan.p, d, falling, (side, (now, now + 1), inside));
        let ramp = edge.ramp(now, self.held);
        match (edge.crossed(x, ramp), ramp.moved) {
            (Some(across), true) => {
                let own = Placed {
                    picture: Picture::default(),
                    ramp: Some(ramp),
                    crossing: false,
                    moving: None,
                    followed: Followed::default(),
                    ..across.clone()
                };
                (None, vec![own, across])
            }
            (crossed, _) => (Some(ramp), crossed.into_iter().collect()),
        }
    }

    fn flat(
        &self,
        c: &Clip,
        whole: &Clip,
        (at, mode): (i64, SmoothMode),
    ) -> Result<Option<VideoPiece>> {
        let (p, t, (x, y)) = (self.plan.p, self.lane.track, (self.x, self.y));
        let excluded = !t.render_cache;
        if !t.video_enabled {
            return Ok(None);
        }
        let (a, b) = match (self.held, mode) {
            (true, _) => (self.now, self.now + 1),
            (false, SmoothMode::Faster) => (at, at + 1),
            (false, _) => (x, y),
        };
        let bare = without_mode(whole);
        let drawn = bare.as_ref().unwrap_or(whole);
        let pieces = flat_pieces(p, t, drawn, (a.max(0), b.max(1)))?;
        Ok(Some(VideoPiece::Flat {
            frames: y - x,
            pieces,
            skip: 0,
            placed: self.outer_of(c),
            excluded,
        }))
    }

    fn outer_of(&self, c: &Clip) -> Vec<Placed> {
        let bare = Clip {
            picture: None,
            ..c.clone()
        };
        placed_of(&bare, self.nest_of(c), true, &[], self.x)
    }

    fn smoothed(
        &self,
        d: &transitions::Playing,
    ) -> Result<Option<(VideoPiece, VideoPiece, Smooth)>> {
        let (p, t, x) = (self.plan.p, self.lane.track, self.x);
        let shows = |c: &&Clip| visible(t, c);
        let fills = |c: &Clip| self.plan.fills(c);
        let Some((smooth, mode)) = Smooth::of(p, d) else {
            return Ok(None);
        };
        let side = |c: &Clip, at: i64| -> Result<Option<VideoPiece>> {
            let nest = self.nest_of(c);
            if let Some(whole) = nest.and_then(|n| n.flat.as_deref()) {
                return self.flat(c, whole, (at, mode));
            }
            if !shows(&c) {
                return Ok(None);
            }
            let bare = without_mode(c);
            let drawn = bare.as_ref().unwrap_or(c);
            let mut piece = self.laid(drawn, nest, &[])?;
            let moved = |p: &Placed| !fills(drawn) || !p.picture.is_usual();
            let backward = mode != SmoothMode::Faster && c.pace().speed() < 0.0;
            if backward || piece.placed().first().is_some_and(moved) {
                let mut wide = widened(c, d.frames);
                wide.retime = ClipRetime::Nearest;
                return self.flat(c, &wide, (at, mode));
            }
            match &mut piece {
                VideoPiece::Source {
                    source_frame,
                    pace,
                    retime,
                    ..
                } => {
                    *retime = Retime::Nearest;
                    if mode == SmoothMode::Faster {
                        *source_frame = c.source_at(at);
                        *pace = Pace::of(0.0, 0);
                    }
                }
                VideoPiece::Generated { picture, .. } => {
                    if mode == SmoothMode::Faster {
                        *picture = picture.after(at - x).held();
                    }
                }
                _ => return Ok(None),
            }
            Ok(Some(piece))
        };
        let gap = || VideoPiece::Flat {
            frames: self.y - x,
            pieces: vec![VideoPiece::black(self.y - x)].into(),
            skip: 0,
            placed: d
                .before
                .or(d.after)
                .map_or_else(Vec::new, |c| self.outer_of(c)),
            excluded: false,
        };
        let either = |c: Option<&Clip>, at: i64| match c {
            Some(c) => side(c, at),
            None => Ok(Some(gap())),
        };
        Ok(
            match (
                either(d.before, d.frames.0)?,
                either(d.after, d.frames.1 - 1)?,
            ) {
                (Some(f), Some(g)) => Some((f, g, smooth)),
                _ => None,
            },
        )
    }

    fn pictures(&self) -> Result<Drawn> {
        let Some(d) = self.playing() else {
            return self.plain();
        };
        match self.smoothed(d)? {
            Some(sides) => Ok(self.smooth_cut(d, sides)),
            None => self.crossing(d),
        }
    }

    fn smooth_cut(
        &self,
        d: &transitions::Playing,
        (f, g, smooth): (VideoPiece, VideoPiece, Smooth),
    ) -> Drawn {
        let ramp = self.ramp_of(d, false);
        let flat = [d.before, d.after]
            .into_iter()
            .flatten()
            .filter_map(|c| self.nest_of(c).filter(|n| n.flat.is_some()))
            .map(|n| n.opened)
            .collect();
        let smooth = Smooth {
            clear: [&f, &g]
                .iter()
                .any(|p| matches!(p, VideoPiece::Flat { .. })),
            ..smooth
        };
        let keeps_below = smooth.mode.is_some() || smooth.clear;
        Drawn {
            flat,
            hides: (!keeps_below).then(|| around(&f)),
            layers: vec![
                Layer::Picture(f, keeps_below.then(|| self.ramp_of(d, true))),
                Layer::Smoothed(g, ramp, smooth),
            ],
        }
    }

    fn crossing(&self, d: &transitions::Playing) -> Result<Drawn> {
        let (t, (x, y), now) = (self.lane.track, (self.x, self.y), self.now);
        let shows = |c: &&Clip| visible(t, c);
        let fills = |c: &Clip| self.plan.fills(c);
        let excluded = !t.render_cache;
        let sides = sides_at(d, now);
        let fill = sides
            .iter()
            .all(|(c, _)| c.filter(shows).is_some_and(fills));
        let mut moded = false;
        let mut layers = Vec::new();
        for (c, falling) in sides {
            let Some(c) = c.filter(shows) else {
                continue;
            };
            let (ramp, chain) = self.sided(d, falling);
            moded |= chain.iter().any(|c| c.picture.composite_mode.is_some());
            let bare = without_mode(c).filter(|_| !chain.is_empty());
            let drawn = bare.as_ref().unwrap_or(c);
            layers.push(Layer::Picture(
                self.laid(drawn, self.nest_of(c), &chain)?,
                ramp,
            ));
        }
        let outer = outer_at(d.before.or(d.after).and_then(|c| self.nest_of(c)), x);
        if let Some(color) = transitions::dip_of(d.transition) {
            let falling = now < middle(d.frames);
            let (ramp, chain) = self.sided(d, falling);
            let own = chain.first().map(|c| Placed {
                picture: Picture::default(),
                crossing: false,
                moving: None,
                followed: Followed::default(),
                ..c.clone()
            });
            let piece = VideoPiece::Generated {
                frames: y - x,
                picture: Generated::SolidColor { color },
                placed: own.into_iter().chain(chain).chain(outer.clone()).collect(),
                excluded,
            };
            let ramp = ramp.map(|r| Ramp {
                falling: !falling,
                ..r
            });
            layers.push(Layer::Picture(piece, ramp));
        }
        Ok(Drawn {
            hides: (fill && !moded).then_some(outer),
            layers,
            ..Drawn::default()
        })
    }

    fn plain(&self) -> Result<Drawn> {
        let (t, now) = (self.lane.track, self.now);
        let covering = t.clip_covering(now).map(|ci| &t.clips[ci]);
        let Some(c) = covering.filter(|c| visible(t, c)) else {
            return Ok(Drawn::default());
        };
        let piece = self.laid(c, self.nest_of(c), &[])?;
        Ok(Drawn {
            hides: self.plan.fills(c).then(|| around(&piece)),
            layers: vec![Layer::Picture(piece, None)],
            ..Drawn::default()
        })
    }

    fn lettered(&self, c: &Clip, fade: Option<Ramp>, crossed: Vec<Placed>) -> Option<ShownTitle> {
        let x = self.x;
        let nest = self.nest_of(c);
        let run = nest.and_then(|n| n.run).filter(|r| r.held).unwrap_or(Run {
            at: x - c.start,
            of: c.duration,
            held: false,
        });
        Some(ShownTitle {
            clip: c.id.clone(),
            title: c.title.clone()?,
            run,
            source_in: c.source_in,
            fade,
            placed: crossed.into_iter().chain(outer_at(nest, x)).collect(),
            lead: c
                .title
                .as_ref()
                .and_then(|t| t.follows.as_ref())
                .and_then(|f| Lead::of(self.plan.p, c, f))
                .map(Arc::new),
        })
    }

    fn titles(&self) -> Vec<Layer> {
        let (t, now) = (self.lane.track, self.now);
        let sides = match self.playing() {
            Some(d) => sides_at(d, now)
                .into_iter()
                .map(|(c, falling)| {
                    let (fade, crossed) = self.sided(d, falling);
                    (c, fade, crossed)
                })
                .collect(),
            None => vec![(
                t.clip_covering(now).map(|ci| &t.clips[ci]),
                None,
                Vec::new(),
            )],
        };
        sides
            .into_iter()
            .filter_map(|(c, fade, crossed)| {
                let c = c.filter(|c| titled(t, c))?;
                self.lettered(c, fade, crossed)
            })
            .map(|s| Layer::Title(Box::new(s)))
            .collect()
    }
}

fn planned_audio(
    p: &Project,
    within: Option<&Within>,
    (a, b): (i64, i64),
) -> Result<(Vec<Vec<AudioPiece>>, Vec<LaneTrack>)> {
    let pool = p.pool();
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t - a, s.sample_rate);

    let any_solo = p.any_solo();
    let mut audio = Vec::new();
    let mut lanes = Vec::new();
    for (ti, track) in p.tracks.iter().enumerate() {
        if !track.in_mix(any_solo) || track.clips.iter().all(|c| c.audio.is_none()) {
            continue;
        }
        let held = within.and_then(|w| w.get(ti));
        let window = held.map_or(EVERY_FRAME, |i| i.shown);
        let around = held.map_or(&[][..], |i| i.fades.as_slice());
        let fades = transitions::playing(p, track, true, |t| transitions::cross_fade(t).is_some());
        let fade_of = |c: &Clip, into: bool| {
            fades.iter().find(|f| {
                let side = if into { f.after } else { f.before };
                side.is_some_and(|x| std::ptr::eq(x, c))
            })
        };
        let mut two: [(Vec<AudioPiece>, i64); 2] = [(Vec::new(), a), (Vec::new(), a)];
        let mut on = 0;
        for c in &track.clips {
            let Some((ra, rb)) = c.audio_range() else {
                continue;
            };
            let (into, out_of) = (fade_of(c, true), fade_of(c, false));
            if into.is_some_and(|f| f.before.is_some()) {
                on = 1 - on;
            }
            let start = into.map_or(ra, |f| f.frames.0);
            let end = out_of.map_or(rb, |f| f.frames.1);
            let head = into.map_or(start, |f| f.frames.1).min(end);
            let tail = out_of.map_or(end, |f| f.frames.0).max(head);
            let abs = |t: i64| s.fps.frame_to_sample(t, s.sample_rate);
            let fade = |f: Option<&transitions::Playing>, x: i64, falling: bool| {
                f.map(|f| Fade {
                    at: abs(a) + sample(x) - abs(f.frames.0),
                    of: abs(f.frames.1) - abs(f.frames.0),
                    falling,
                    curve: transitions::cross_fade(f.transition).expect("plays"),
                })
            };
            let (lane, t) = &mut two[on];
            for (x, y, f, falling) in [
                (start, head, into, false),
                (head, tail, None, false),
                (tail, end, out_of, true),
            ] {
                let (x, y) = (x.max(a).max(window.from), y.min(b).min(window.to));
                if y <= x {
                    continue;
                }
                push_audio(
                    lane,
                    AudioPiece::Silence {
                        samples: sample(x) - sample(*t),
                    },
                );
                let audio = c.audio.as_ref().expect("has audio");
                if audio.muted {
                    push_audio(
                        lane,
                        AudioPiece::Silence {
                            samples: sample(y) - sample(x),
                        },
                    );
                } else {
                    let m = found(pool.get(c.media.as_str()).copied(), &c.media)?;
                    for (x, y) in split_at_fades(c.runs(x, y), around) {
                        let mut piece = source_audio(p, c, m, x, sample(y) - sample(x));
                        if let AudioPiece::Source {
                            gain_db,
                            silent,
                            joined,
                            fade: f2,
                            edges,
                            isolation,
                            ..
                        } = &mut piece
                        {
                            *gain_db += track.volume_db;
                            *silent = crate::validate::fader_silent(track.volume_db);
                            *joined =
                                x > start.max(a) && c.pace().steady(x - 1 - c.start) == Some(1);
                            *f2 = fade(f, x, falling);
                            *edges = around
                                .iter()
                                .filter(|f| f.frames.0 <= x && x < f.frames.1)
                                .map(|f| Fade {
                                    at: abs(a) + sample(x) - abs(f.frames.0),
                                    of: abs(f.frames.1) - abs(f.frames.0),
                                    falling: f.falling,
                                    curve: f.curve,
                                })
                                .collect();
                            *isolation = audio
                                .voice_isolation
                                .or(track.voice_isolation)
                                .and_then(|v| v.heard());
                        }
                        push_audio(lane, piece);
                    }
                }
                *t = y;
            }
        }
        for (mut lane, t) in two {
            push_audio(
                &mut lane,
                AudioPiece::Silence {
                    samples: sample(b) - sample(t),
                },
            );
            if lane
                .iter()
                .any(|piece| matches!(piece, AudioPiece::Source { .. }))
            {
                audio.push(lane);
                lanes.push(LaneTrack {
                    track: track.id.clone(),
                    volume_db: track.volume_db,
                });
            }
        }
    }

    Ok((audio, lanes))
}

fn source_audio(p: &Project, c: &Clip, m: &Media, x: i64, samples: i64) -> AudioPiece {
    let audio = c.audio.as_ref().expect("has audio");
    let (path, origin) = origin_of(m);
    let (fps, rate) = (p.settings.fps, p.settings.sample_rate);
    let speed = c.speed_at(x);
    let per_frame = f64::from(rate) / fps.as_f64();
    let since = fps.frame_to_sample(x, rate) - fps.frame_to_sample(c.start, rate);
    let source_sample = if c.speed.is_none() {
        fps.frame_to_sample(c.source_at(x), rate)
    } else if speed == 0.0 {
        return AudioPiece::Silence { samples };
    } else if c.constant_speed().is_some() {
        fps.frame_to_sample(c.source_in, rate)
            + (c.pace().sound(0.0) * per_frame + since as f64 * speed).round() as i64
    } else {
        let k = x - c.start;
        fps.frame_to_sample(c.source_in, rate)
            + (c.pace().sound_in(k, since as f64 / per_frame) * per_frame).round() as i64
    };
    let keyed = audio.is_keyed().then(|| {
        Box::new(Keyed {
            keys: audio.gain_keys.clone(),
            per_frame,
            at: fps.frame_to_sample(c.content_at(c.start), rate) as f64 + since as f64,
        })
    });
    AudioPiece::Source {
        media: c.media.clone(),
        path,
        stream: audio.stream,
        origin,
        source_sample,
        samples,
        gain_db: if keyed.is_some() { 0.0 } else { audio.gain_db },
        silent: false,
        speed: c.pace().rate(x - c.start),
        joined: false,
        fade: None,
        edges: Box::default(),
        isolation: None,
        keyed,
        pan: audio.heard_pan(),
        equalizer: audio
            .equalizer
            .clone()
            .filter(crate::model::Equalizer::heard)
            .map(Box::new),
        leveler: audio
            .dialogue_leveler
            .filter(crate::model::DialogueLeveler::heard)
            .map(Box::new),
        compressor: audio
            .compressor
            .filter(crate::model::Compressor::heard)
            .map(Box::new),
        noise_reduction: audio
            .noise_reduction
            .filter(crate::model::NoiseReduction::heard)
            .map(Box::new),
    }
}

pub fn flatten_clip(p: &Project, clip: &str) -> Result<RenderPlan> {
    let c = p
        .clip(clip)
        .ok_or_else(|| Error::not_found(format!("no clip {clip}")))?
        .clone();
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == c.id);
        t.transitions.clear();
        for x in &mut t.clips {
            if let Some(a) = x.audio.as_mut() {
                a.head = 0;
                a.tail = 0;
            }
        }
    }
    let mut plan = flatten(&alone, c.start, c.end())?;
    plan.colors = timeline_colors(p);
    plan.name = c
        .name
        .clone()
        .or_else(|| p.media(&c.media).map(|m| stem(&m.name)))
        .unwrap_or_else(|| c.id.clone());
    Ok(plan)
}

pub fn generator_tape(p: &Project, generator: &Generator, a: i64, b: i64) -> Result<RenderPlan> {
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.clear();
        t.transitions.clear();
        t.video_enabled = true;
    }
    let clip = crate::ops::block_clip(
        String::from("generator"),
        (0, b),
        None,
        Some(generator.clone()),
    );
    alone
        .track_mut(crate::ops::MAIN)
        .expect("every project has V1")
        .clips
        .push(clip);
    let mut plan = flatten(&alone, a, b)?;
    plan.colors = timeline_colors(p);
    plan.name = crate::generators::name(generator).to_string();
    Ok(plan)
}

pub fn sound_alone(p: &Project, clip: &str) -> Result<RenderPlan> {
    let c = p
        .clip(clip)
        .ok_or_else(|| Error::not_found(format!("no clip {clip}")))?
        .clone();
    let a = c
        .audio
        .as_ref()
        .ok_or_else(|| Error::invalid(format!("{clip} has no sound")))?;
    let (from, to) = ((c.start + a.head).max(0), c.end() + a.tail);
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == c.id);
        t.transitions.clear();
        t.muted = false;
        t.solo = false;
        t.volume_db = 0.0;
        t.voice_isolation = None;
        t.duck = None;
        for x in &mut t.clips {
            x.enabled = true;
            if let Some(s) = x.audio.as_mut() {
                s.gain_db = 0.0;
                s.gain_keys.clear();
                s.muted = false;
                s.pan = 0.0;
                s.equalizer = None;
                s.dialogue_leveler = None;
                s.compressor = None;
                s.noise_reduction = None;
                s.voice_isolation = None;
            }
        }
    }
    flatten(&alone, from, to)
}

pub fn clip_frame(p: &Project, clip: &str, at: i64) -> Result<VideoPiece> {
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == clip);
        t.transitions.clear();
        for x in &mut t.clips {
            x.picture = None;
        }
    }
    flatten(&alone, at, at + 1)?
        .video
        .into_iter()
        .next()
        .ok_or_else(|| Error::not_found(format!("no frame {at} of {clip}")))
}

pub fn media_frame(p: &Project, m: &Media, at: i64) -> VideoPiece {
    match &m.info.video {
        Some(_) => VideoPiece::Source {
            pace: Default::default(),
            media: m.id.clone(),
            path: m.path.clone(),
            origin: m.origin(),
            source_frame: at,
            frames: 1,
            image: image_of(m),
            color_space: m.color_space,
            alpha_mode: m.alpha_mode,
            super_scale: super_scale_of(m.super_scale, &p.settings),
            retime: Retime::Nearest,
            placed: Vec::new(),
            cache: Default::default(),
            excluded: false,
            made: None,
            input_lut: None,
        },
        None => VideoPiece::black(1),
    }
}

/// A media item's place on a source tape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TapeEntry {
    pub media: String,
    /// First tape frame.
    pub start: i64,
    pub frames: i64,
}

pub fn tape(p: &Project, media: &[String]) -> Result<(RenderPlan, Vec<TapeEntry>)> {
    laid_out(p, media)
}

pub fn camera_tape(
    p: &Project,
    camera: &crate::multicam::GridCamera,
    end: i64,
) -> Result<(RenderPlan, Vec<TapeEntry>)> {
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t, s.sample_rate);
    let entries: Vec<TapeEntry> = camera
        .files
        .iter()
        .map(|f| TapeEntry {
            media: f.media.clone(),
            start: f.at,
            frames: f.frames,
        })
        .collect();
    if entries.is_empty() {
        return Err(Error::invalid("no media to show"));
    }
    let mut cuts = vec![0, end];
    cuts.extend(entries.iter().flat_map(|e| [e.start, e.start + e.frames]));
    cuts.sort_unstable();
    cuts.dedup();
    let pool = p.pool();
    let (mut video, mut lane) = (Vec::new(), Vec::new());
    for cut in cuts.windows(2) {
        let (a, b) = (cut[0], cut[1]);
        let samples = sample(b) - sample(a);
        let Some(e) = entries
            .iter()
            .find(|e| (e.start..e.start + e.frames).contains(&a))
        else {
            push_video(&mut video, VideoPiece::black(b - a));
            push_audio(&mut lane, AudioPiece::Silence { samples });
            continue;
        };
        let m = found(pool.get(e.media.as_str()).copied(), &e.media)?;
        let from = a - e.start;
        let (path, origin) = origin_of(m);
        push_video(
            &mut video,
            VideoPiece::Source {
                pace: Default::default(),
                media: e.media.clone(),
                path,
                origin,
                source_frame: from,
                frames: b - a,
                image: image_of(m),
                color_space: m.color_space,
                alpha_mode: m.alpha_mode,
                super_scale: super_scale_of(m.super_scale, &p.settings),
                retime: Retime::Nearest,
                placed: Vec::new(),
                cache: Default::default(),
                excluded: false,
                made: None,
                input_lut: None,
            },
        );
        push_audio(&mut lane, sound_of(p, m, from, samples));
    }
    Ok((tape_plan(p, end, video, lane), entries))
}

fn tape_plan(
    p: &Project,
    frames: i64,
    video: Vec<VideoPiece>,
    lane: Vec<AudioPiece>,
) -> RenderPlan {
    let s = &p.settings;
    RenderPlan {
        fps: s.fps,
        width: s.width,
        height: s.height,
        sample_rate: s.sample_rate,
        channels: s.channels,
        frames,
        video,
        audio: vec![lane],
        ducking: Vec::new(),
        range: (0, frames),
        name: "tape".into(),
        colors: None,
        output_lut: None,
    }
}

pub fn places_sound(p: &Project, rows: &[crate::model::GroupRow]) -> Result<RenderPlan> {
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t, s.sample_rate);
    let end = rows
        .iter()
        .flat_map(|r| &r.clips)
        .map(|c| c.at + crate::multicam::visible(p, c))
        .max()
        .unwrap_or(0);
    let mut audio = Vec::with_capacity(rows.len());
    for row in rows {
        let mut lane = Vec::new();
        let mut t = 0;
        for (i, c) in row.clips.iter().enumerate() {
            let m = found(p.media(&c.media), &c.media)?;
            push_audio(
                &mut lane,
                AudioPiece::Silence {
                    samples: sample(c.at) - sample(t),
                },
            );
            let next = row.clips.get(i + 1).map_or(i64::MAX, |n| n.at);
            t = (c.at + crate::multicam::visible(p, c)).min(next);
            let samples = sample(t) - sample(c.at);
            push_audio(&mut lane, sound_of(p, m, c.head, samples));
        }
        push_audio(
            &mut lane,
            AudioPiece::Silence {
                samples: sample(end) - sample(t),
            },
        );
        audio.push(lane);
    }
    Ok(RenderPlan {
        fps: s.fps,
        width: s.width,
        height: s.height,
        sample_rate: s.sample_rate,
        channels: s.channels,
        frames: end,
        video: vec![VideoPiece::black(end)],
        audio,
        ducking: Vec::new(),
        range: (0, end),
        name: "places".into(),
        colors: None,
        output_lut: None,
    })
}

fn sound_of(p: &Project, m: &Media, head: i64, samples: i64) -> AudioPiece {
    if m.info.audio.is_empty() {
        return AudioPiece::Silence { samples };
    }
    let (path, origin) = origin_of(m);
    AudioPiece::Source {
        speed: 1.0,
        joined: false,
        media: m.id.clone(),
        path,
        stream: 0,
        origin,
        source_sample: p.settings.fps.frame_to_sample(head, p.settings.sample_rate),
        samples,
        gain_db: 0.0,
        silent: false,
        fade: None,
        edges: Box::default(),
        isolation: None,
        keyed: None,
        pan: 0.0,
        equalizer: None,
        leveler: None,
        compressor: None,
        noise_reduction: None,
    }
}

fn laid_out(p: &Project, media: &[String]) -> Result<(RenderPlan, Vec<TapeEntry>)> {
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t, s.sample_rate);
    let (mut video, mut lane, mut entries) = (Vec::new(), Vec::new(), Vec::new());
    let pool = p.pool();
    let mut t = 0;
    for id in media {
        let m = found(pool.get(id.as_str()).copied(), id)?;
        let frames = p.media_frames(m);
        if frames <= 0 {
            continue;
        }
        let (path, origin) = origin_of(m);
        if m.info.video.is_some() {
            push_video(
                &mut video,
                VideoPiece::Source {
                    pace: Default::default(),
                    media: id.to_string(),
                    path,
                    origin,
                    source_frame: 0,
                    frames,
                    image: image_of(m),
                    color_space: m.color_space,
                    alpha_mode: m.alpha_mode,
                    super_scale: super_scale_of(m.super_scale, &p.settings),
                    retime: Retime::Nearest,
                    placed: Vec::new(),
                    cache: Default::default(),
                    excluded: false,
                    made: None,
                    input_lut: None,
                },
            );
        } else {
            push_video(&mut video, VideoPiece::black(frames));
        }
        let samples = sample(t + frames) - sample(t);
        push_audio(&mut lane, sound_of(p, m, 0, samples));
        entries.push(TapeEntry {
            media: id.to_string(),
            start: t,
            frames,
        });
        t += frames;
    }
    if entries.is_empty() {
        return Err(Error::invalid("no media to show"));
    }
    Ok((tape_plan(p, t, video, lane), entries))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanPiece {
    pub media: String,
    pub range: (i64, i64),
}

pub fn span_pieces(entries: &[TapeEntry], mark_in: i64, mark_out: i64) -> Vec<SpanPiece> {
    let mut out = Vec::new();
    for e in entries {
        let end = e.start + e.frames;
        let a = mark_in.max(e.start);
        let b = mark_out.min(end);
        if b > a {
            out.push(SpanPiece {
                media: e.media.clone(),
                range: (a - e.start, b - e.start),
            });
        }
    }
    out
}

pub fn active_span(
    entries: &[TapeEntry],
    mark_in: Option<i64>,
    mark_out: Option<i64>,
) -> Option<(i64, i64)> {
    match (mark_in, mark_out) {
        (None, None) => None,
        (Some(i), Some(o)) => (o > i).then_some((i, o)),
        (Some(i), None) => {
            let end = entries
                .iter()
                .find(|e| e.start <= i && i < e.start + e.frames)
                .map(|e| e.start + e.frames)?;
            (end > i).then_some((i, end))
        }
        (None, Some(o)) => {
            let frame = o - 1;
            let start = entries
                .iter()
                .find(|e| e.start <= frame && frame < e.start + e.frames)
                .map(|e| e.start)?;
            (o > start).then_some((start, o))
        }
    }
}

pub fn crossing_span(
    entries: &[TapeEntry],
    mark_in: Option<i64>,
    mark_out: Option<i64>,
) -> Option<(i64, i64)> {
    let (i, o) = (mark_in?, mark_out?);
    (span_pieces(entries, i, o).len() > 1).then_some((i, o))
}

fn stem(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::Fixture;

    fn fixture() -> Fixture {
        Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)])
    }

    #[test]
    fn sections_of_an_inner_clip_end_with_the_run_of_the_compound_clip() {
        let speed = |list: &[(i64, f64, f64)]| crate::model::Speed {
            sections: list
                .iter()
                .map(|&(at, from, speed)| Section {
                    at,
                    from,
                    speed,
                    ease: None,
                })
                .collect::<Vec<_>>()
                .into(),
            skip: 0,
            effect: None,
        };
        let mut f = fixture();
        let id = f.append("m1", 0, 70);
        let mut c = f.editor.project().clip(&id).unwrap().clone();
        c.speed = Some(speed(&[(0, 0.0, 2.0), (10, 20.0, 1.0)]));
        let own = speed(&[(0, 0.0, 1.0), (20, 20.0, 2.0), (40, 60.0, 1.0)]);
        let made = composed_over(&own, &c, 0);
        let got: Vec<_> = made
            .sections
            .iter()
            .map(|s| (s.at, s.from, s.speed))
            .collect();
        assert_eq!(got, [(0, 0.0, 2.0), (10, 20.0, 2.0), (30, 60.0, 1.0)]);
    }

    fn req(f: &mut Fixture, method: &str, params: serde_json::Value) {
        f.ok(json!({"method": method, "params": params}));
    }

    fn dissolve(
        f: &Fixture,
        from: Option<&str>,
        to: Option<&str>,
        before: i64,
        after: i64,
    ) -> Project {
        let mut p = f.editor.project().clone();
        p.track_mut("V1")
            .unwrap()
            .transitions
            .push(crate::model::Transition {
                id: "t1".into(),
                sound: false,
                from: from.map(Into::into),
                to: to.map(Into::into),
                before,
                after,
                otio: json!({"OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve"})
                    .into(),
            });
        p
    }

    fn cross_fade(
        f: &Fixture,
        from: Option<&str>,
        to: Option<&str>,
        before: i64,
        after: i64,
    ) -> Project {
        let mut p = f.editor.project().clone();
        p.track_mut("V1")
            .unwrap()
            .transitions
            .push(crate::model::Transition {
                id: "t1".into(),
                sound: true,
                from: from.map(Into::into),
                to: to.map(Into::into),
                before,
                after,
                otio: json!({
                    "OTIO_SCHEMA": "Transition.1",
                    "transition_type": "Custom_Transition",
                    "metadata": {"Resolve_OTIO": {"Effects": {"Type": 41}}}
                })
                .into(),
            });
        p
    }

    fn heard(plan: &RenderPlan) -> Vec<Vec<String>> {
        plan.audio
            .iter()
            .map(|lane| {
                lane.iter()
                    .map(|p| match p {
                        AudioPiece::Source {
                            media,
                            source_sample,
                            samples,
                            fade,
                            ..
                        } => match fade {
                            None => format!("{media}@{source_sample}+{samples}"),
                            Some(f) => format!(
                                "{media}@{source_sample}+{samples} {} {:?} at {}/{}",
                                if f.falling { "falling" } else { "rising" },
                                f.curve,
                                f.at,
                                f.of
                            ),
                        },
                        AudioPiece::Silence { samples } => format!("silence+{samples}"),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_cross_fade_plays_the_sound_after_the_cut_on_another_lane() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let p = cross_fade(&f, Some("c1"), Some("c2"), 5, 5);
        let plan = flatten(&p, 0, 100).unwrap();
        assert_eq!(
            heard(&plan),
            [
                vec![
                    "m1@0+86400".to_string(),
                    "m1@86400+19200 falling Plus3Db at 0/19200".into(),
                    "silence+86400".into(),
                ],
                vec![
                    "silence+86400".to_string(),
                    "m2@9600+19200 rising Plus3Db at 0/19200".into(),
                    "m2@28800+86400".into(),
                ],
            ]
        );
    }

    #[test]
    fn a_fade_in_from_silence_stays_on_its_lane() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m1", 50, 100);
        req(&mut f, "edit.delete", json!({"clips": "c1", "lift": true}));
        let p = cross_fade(&f, None, Some("c2"), 0, 10);
        let plan = flatten(&p, 0, 100).unwrap();
        assert_eq!(
            heard(&plan),
            [vec![
                "silence+96000".to_string(),
                "m1@96000+19200 rising Plus3Db at 0/19200".into(),
                "m1@115200+76800".into(),
            ]]
        );
    }

    #[test]
    fn a_clip_after_a_cross_fades_cut_is_heard_at_its_share_through_it() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let p = cross_fade(&f, Some("c1"), Some("c2"), 2, 2);
        let t = p.track("V1").unwrap();
        let fades = transitions::cross_fades(&p, t);
        assert_eq!(transitions::heard_at(&fades, &t.clips[1], 49), Some(0.5));
    }

    #[test]
    fn the_middle_of_the_pan_changes_no_side() {
        assert_eq!(pan_db(0.0), (0.0, 0.0));
    }

    #[test]
    fn pan_to_the_right_lowers_the_left_and_raises_the_right() {
        assert_eq!(pan_db(25.0), (-2.5, 1.9));
    }

    #[test]
    fn pan_to_the_left_mirrors_pan_to_the_right() {
        assert_eq!(pan_db(-25.0), (1.9, -2.5));
    }

    #[test]
    fn the_far_side_of_a_pan_stops_at_minus_six_db() {
        assert_eq!(pan_db(75.0), (-6.0, 4.9));
    }

    #[test]
    fn a_full_pan_is_six_db_on_each_side() {
        assert_eq!(pan_db(100.0), (-6.0, 6.0));
    }

    #[test]
    fn a_pan_moves_in_steps_of_a_tenth_of_a_db() {
        assert_eq!(pan_db(1.5), (-0.1, 0.1));
    }

    #[test]
    fn a_centered_pan_has_unity_gains() {
        assert_eq!(pan_gains(0.0), (1.0, 1.0));
    }

    fn audio_pieces(f: &Fixture) -> Vec<AudioPiece> {
        flatten(f.editor.project(), 0, 100).unwrap().audio[0].clone()
    }

    #[test]
    fn a_clips_pan_reaches_its_piece() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(&mut f, "clip.set", json!({"clips": "c1", "pan": -40.0}));
        assert!(matches!(
            audio_pieces(&f)[0],
            AudioPiece::Source { pan, .. } if pan == -40.0
        ));
    }

    #[test]
    fn a_heard_equalizer_reaches_its_piece() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        let eq = json!({"bands": [{"band": 2, "gain": 3.0}]});
        req(&mut f, "clip.set", json!({"clips": "c1", "equalizer": eq}));
        assert!(matches!(
            &audio_pieces(&f)[0],
            AudioPiece::Source { equalizer: Some(e), .. } if e.bands[1].gain == 3.0
        ));
    }

    #[test]
    fn a_flat_equalizer_does_not_reach_its_piece() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(&mut f, "clip.set", json!({"clips": "c1", "equalizer": {}}));
        assert!(matches!(
            &audio_pieces(&f)[0],
            AudioPiece::Source {
                equalizer: None,
                ..
            }
        ));
    }

    #[test]
    fn two_clips_with_other_equalizers_stay_two_pieces() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m1", 50, 100);
        let eq = json!({"bands": [{"band": 2, "gain": 3.0}]});
        req(&mut f, "clip.set", json!({"clips": "c2", "equalizer": eq}));
        assert_eq!(audio_pieces(&f).len(), 2);
    }

    #[test]
    fn a_leveler_with_a_process_on_reaches_its_piece() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        let l = json!({"reduce_loud": true});
        req(
            &mut f,
            "clip.set",
            json!({"clips": "c1", "dialogue_leveler": l}),
        );
        assert!(matches!(
            audio_pieces(&f)[0],
            AudioPiece::Source {
                leveler: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_leveler_with_no_process_on_does_not_reach_its_piece() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(
            &mut f,
            "clip.set",
            json!({"clips": "c1", "dialogue_leveler": {}}),
        );
        assert!(matches!(
            audio_pieces(&f)[0],
            AudioPiece::Source { leveler: None, .. }
        ));
    }

    #[test]
    fn a_compressor_reaches_its_piece() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(&mut f, "clip.set", json!({"clips": "c1", "compressor": {}}));
        assert!(matches!(
            audio_pieces(&f)[0],
            AudioPiece::Source {
                compressor: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn a_noise_reduction_reaches_its_piece() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(
            &mut f,
            "clip.set",
            json!({"clips": "c1", "noise_reduction": {}}),
        );
        assert!(matches!(
            audio_pieces(&f)[0],
            AudioPiece::Source {
                noise_reduction: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn two_clips_with_other_pans_stay_two_pieces() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m1", 50, 100);
        req(&mut f, "clip.set", json!({"clips": "c2", "pan": 30.0}));
        assert_eq!(audio_pieces(&f).len(), 2);
    }

    fn auto_for_hdr_slow_motion() -> Retime {
        let f = fixture();
        let p = f.editor.project();
        let mut m = p.media("m1").unwrap().clone();
        m.info.video.as_mut().unwrap().hdr = Some(crate::model::Hdr::Pq);
        retime_at(ClipRetime::Project, 0.5, &m, &p.settings)
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn auto_gives_hdr_slow_motion_ai_fast() {
        assert_eq!(auto_for_hdr_slow_motion(), Retime::AiFast);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn auto_gives_hdr_slow_motion_the_ai_of_macos() {
        assert_eq!(auto_for_hdr_slow_motion(), Retime::Macos);
    }

    #[test]
    fn a_cross_fade_of_plus_3_db_is_half_way_up_a_quarter_in() {
        assert_eq!(Curve::Plus3Db.gain(0.25), 0.5);
    }

    #[test]
    fn a_cross_fade_of_minus_3_db_is_an_eighth_up_a_quarter_in() {
        assert_eq!(Curve::Minus3Db.gain(0.25), 0.125);
    }

    #[test]
    fn a_falling_fade_is_whole_at_its_first_sample() {
        let fade = Fade {
            at: 0,
            of: 100,
            falling: true,
            curve: Curve::ZeroDb,
        };
        assert_eq!(fade.gain(0), 1.0);
    }

    #[test]
    fn a_rising_fade_is_a_quarter_up_a_quarter_in() {
        let fade = Fade {
            at: 20,
            of: 100,
            falling: false,
            curve: Curve::ZeroDb,
        };
        assert_eq!(fade.gain(5), 0.25);
    }

    fn shown(plan: &RenderPlan) -> Vec<String> {
        fn one(p: &VideoPiece) -> String {
            match p {
                VideoPiece::Source {
                    media,
                    source_frame,
                    frames,
                    ..
                } => format!("{media}@{source_frame}+{frames}"),
                VideoPiece::Black { frames, .. } => format!("black+{frames}"),
                VideoPiece::Generated { frames, .. } => format!("generated+{frames}"),
                VideoPiece::Flat { frames, .. } => format!("flat+{frames}"),
                VideoPiece::Mix { under, over } => {
                    let over: Vec<String> = over
                        .iter()
                        .map(|o| match o.ramp {
                            Some(r) => {
                                let fall = if r.falling { " falling" } else { "" };
                                format!("{} at {}/{}{fall}", one(&o.piece), r.at, r.of)
                            }
                            None => one(&o.piece),
                        })
                        .collect();
                    format!("{} over {}", over.join(", "), one(under))
                }
                VideoPiece::Titled { under, .. } => one(under),
            }
        }
        plan.video.iter().map(one).collect()
    }

    #[test]
    fn a_dissolve_shows_each_frame_at_the_level_of_its_middle() {
        let ramp = Ramp {
            at: 0,
            of: 4,
            falling: false,
            held: false,
            moved: false,
        };
        let levels: Vec<(i64, i64)> = (0..4).map(|k| ramp.fraction(k)).collect();
        assert_eq!(levels, [(1, 8), (3, 8), (5, 8), (7, 8)]);
    }

    #[test]
    fn a_falling_level_starts_where_a_rising_one_ends() {
        let ramp = Ramp {
            at: 0,
            of: 4,
            falling: true,
            held: false,
            moved: false,
        };
        assert_eq!(ramp.fraction(0), (7, 8));
    }

    #[test]
    fn a_weight_is_the_level_in_sixty_five_thousandths_rounded() {
        let ramp = Ramp {
            at: 0,
            of: 3,
            falling: false,
            held: false,
            moved: false,
        };
        assert_eq!(ramp.weight(0), 10923);
    }

    #[test]
    fn a_dip_to_red_brings_the_color_up_then_down() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let dip = json!({"type": "dip_to_color", "color": "#ff0000"});
        req(
            &mut f,
            "edit.dissolve",
            json!({"at": 50, "transition": dip}),
        );
        let plan = flatten(f.editor.project(), 0, 100).unwrap();
        let halves: Vec<([u8; 3], bool)> = plan
            .video
            .iter()
            .filter_map(|p| {
                p.layers().into_iter().find_map(|(l, ramp)| match l {
                    VideoPiece::Generated {
                        picture: Generated::SolidColor { color },
                        ..
                    } => Some((*color, ramp?.falling)),
                    _ => None,
                })
            })
            .collect();
        assert_eq!(halves, [([255, 0, 0], false), ([255, 0, 0], true)]);
    }

    #[test]
    fn a_fade_from_black_shows_the_clip_over_black() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m1", 50, 100);
        req(&mut f, "edit.delete", json!({"clips": "c1", "lift": true}));
        let p = dissolve(&f, None, Some("c2"), 5, 5);
        let plan = flatten(&p, 0, 100).unwrap();
        assert_eq!(
            shown(&plan),
            ["black+45", "m1@45+10 at 0/10 over black+10", "m1@55+45"]
        );
    }

    #[test]
    fn a_fade_to_black_shows_the_clip_falling_over_black() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m1", 50, 100);
        req(&mut f, "edit.delete", json!({"clips": "c2", "lift": true}));
        let p = dissolve(&f, Some("c1"), None, 0, 10);
        let plan = flatten(&p, 0, 60).unwrap();
        assert_eq!(
            shown(&plan),
            ["m1@0+50", "m1@50+10 at 0/10 falling over black+10"]
        );
    }

    #[test]
    fn a_cross_dissolve_shows_the_clip_after_the_cut_over_the_one_before() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let p = dissolve(&f, Some("c1"), Some("c2"), 5, 5);
        let plan = flatten(&p, 0, 100).unwrap();
        assert_eq!(
            shown(&plan),
            [
                "m1@0+45",
                "m1@45+10 at 0/10 falling, m2@5+10 at 0/10 over black+10",
                "m2@15+45"
            ]
        );
    }

    #[test]
    fn a_plan_starting_inside_a_dissolve_starts_its_ramp_there() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let p = dissolve(&f, Some("c1"), Some("c2"), 5, 5);
        let plan = flatten(&p, 48, 52).unwrap();
        assert_eq!(
            shown(&plan),
            ["m1@48+4 at 3/10 falling, m2@8+4 at 3/10 over black+4"]
        );
    }

    #[test]
    fn a_dissolve_whose_frames_hold_another_clip_plays_as_a_cut() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 13);
        f.append("m1", 60, 100);
        let p = dissolve(&f, Some("c1"), Some("c2"), 5, 5);
        let plan = flatten(&p, 0, 93).unwrap();
        assert_eq!(shown(&plan), ["m1@0+50", "m2@10+3", "m1@60+40"]);
    }

    #[test]
    fn a_wipe_plays_as_a_cut() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let mut p = dissolve(&f, Some("c1"), Some("c2"), 5, 5);
        p.track_mut("V1").unwrap().transitions[0].otio =
            json!({"OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Wipe"}).into();
        let plan = flatten(&p, 0, 100).unwrap();
        assert_eq!(shown(&plan), ["m1@0+50", "m2@10+50"]);
    }

    #[test]
    fn video_pieces_follow_clips_gaps_and_disabled_clips() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        f.append("m3", 0, 50);
        f.append("m1", 100, 150);
        req(&mut f, "edit.delete", json!({"clips": "c2", "lift": true}));
        req(&mut f, "clip.set", json!({"clips": "c3", "enabled": false}));
        let plan = flatten(f.editor.project(), 0, 200).unwrap();
        let kinds: Vec<(String, i64)> = plan
            .video
            .iter()
            .map(|v| match v {
                VideoPiece::Source {
                    media,
                    source_frame,
                    frames,
                    ..
                } => (format!("{media}@{source_frame}"), *frames),
                VideoPiece::Black { frames, .. } => ("black".into(), *frames),
                VideoPiece::Generated { frames, .. } => ("generated".into(), *frames),
                VideoPiece::Flat { frames, .. } => ("flat".into(), *frames),
                VideoPiece::Mix { .. } | VideoPiece::Titled { .. } => ("mix".into(), v.frames()),
            })
            .collect();
        assert_eq!(kinds, vec![("m1@0".into(), 100), ("black".into(), 100)]);
        assert_eq!(plan.frames, 200);
    }

    #[test]
    fn contiguous_source_pieces_merge() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        req(&mut f, "edit.split", json!({"at": [30, 60]}));
        let plan = flatten(f.editor.project(), 0, 100).unwrap();
        assert_eq!(plan.video.len(), 1, "a split clip renders as one read");
        assert_eq!(plan.audio[0].len(), 1);
    }

    #[test]
    fn paces_show_where_each_frame_begins() {
        let offsets = |speed, skip| {
            (0..6)
                .map(|k| Pace::of(speed, skip).offset(k))
                .collect::<Vec<_>>()
        };
        assert_eq!(offsets(0.5, 0), [0, 0, 1, 1, 2, 2]);
        assert_eq!(offsets(0.5, 1), [0, 1, 1, 2, 2, 3]);
        assert_eq!(offsets(-0.5, 0), [0, 0, -1, -1, -2, -2]);
        assert_eq!(offsets(-0.5, 1), [0, -1, -1, -2, -2, -3]);
        assert_eq!(offsets(-2.0, 0), [0, -2, -4, -6, -8, -10]);
        assert_eq!(offsets(1.0 / 3.0, 0), [0, 0, 0, 1, 1, 1]);
        assert_eq!(offsets(0.0, 4), [0; 6]);
    }

    #[test]
    fn per_clip_plans_leave_out_neighbors() {
        let mut f = fixture();
        f.append("m1", 0, 100);
        f.append("m2", 10, 60);
        req(
            &mut f,
            "edit.audio_roll",
            json!({"clip": "c1", "edge": "tail", "by": 5}),
        );
        let plan = flatten_clip(f.editor.project(), "c2").unwrap();
        assert_eq!(plan.range, (100, 150));
        assert!(matches!(
            plan.video[0],
            VideoPiece::Source {
                source_frame: 10,
                frames: 50,
                ..
            }
        ));
        assert!(matches!(
            plan.audio[0][0],
            AudioPiece::Source {
                source_sample: 19_200,
                samples: 96_000,
                ..
            }
        ));
        assert_eq!(plan.name, "2");
    }
}
