use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{
    Clip, ClipRetime, ColorSpace, DuckDetect, Hdr, Keyframe, Media, MediaSuperScale, Picture,
    PictureTransition, Project, PushPreset, Retime, Section, Settings, SlidePreset, SuperScale,
    TimelineColors, Title, TrackKind, WipeDirection,
};
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
    },
    /// A frame of one color: black, or the color that a dip to color
    /// goes through.
    Black {
        frames: i64,
        #[serde(default, skip_serializing_if = "is_black")]
        color: [u8; 3],
    },
    /// Pictures over each other: `under`, then each of `over` on top of it,
    /// at its level while a dissolve plays (decision D50), each as long as
    /// the piece. `under` is a source, black or a titled picture; each of
    /// `over` a source or black.
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

/// A title of a [`VideoPiece::Titled`], with where the piece is in its clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShownTitle {
    pub clip: String,
    pub title: Title,
    pub run: Run,
    /// How a dissolve at its clip's edge fades it in or out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade: Option<Ramp>,
    /// Where it goes over the frame: the picture of each compound clip it
    /// plays inside, outward. Empty, it is drawn over the frame as it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placed: Vec<Placed>,
}

/// Where a piece is in its clip, which a moving title and a Dynamic Zoom
/// are placed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
            fade: self.fade.map(|r| r.after(k)),
            placed: self.placed.iter().map(|p| p.after(k)).collect(),
        }
    }

    pub fn alpha(&self) -> f64 {
        self.title.opaque() * self.fade.map_or(1.0, |r| r.level(0))
    }
}

/// A picture over the others of a [`VideoPiece::Mix`]: a dissolve's, at
/// the level its ramp gives each frame, a side of a Push or a Slide, or one
/// placed over the frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Over {
    pub piece: VideoPiece,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp: Option<Ramp>,
    /// With the picture before it, the two sides of one dissolve with a
    /// placed picture: they mix into each other, then go over what is
    /// below them together.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cross: bool,
    /// A side of a Push or a Slide. The outgoing side and the incoming
    /// side after it move over each other, then go over what is below them
    /// together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion: Option<Move>,
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
    /// Timeline start of the clip, until the first `after` places it.
    #[serde(default, skip_serializing_if = "zero_i64")]
    pub start: i64,
    /// Set once `after` has placed this picture on a timeline frame.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ready: bool,
}

fn zero_i64(n: &i64) -> bool {
    *n == 0
}

impl Placed {
    pub fn of(clip: &Clip, picture: Picture, run: Run) -> Self {
        Self {
            clip: clip.id.clone(),
            run: picture.dynamic_zoom.map(|_| run),
            picture,
            source: clip.content_at(clip.start),
            start: clip.start,
            ready: false,
        }
    }

    pub fn at(clip: &Clip, picture: Picture, run: Run, time: i64) -> Self {
        let run = picture.dynamic_zoom.map(|_| run);
        Self {
            clip: clip.id.clone(),
            picture,
            run,
            source: clip.content_at(time),
            start: clip.start,
            ready: true,
        }
    }

    pub fn after(&self, k: i64) -> Self {
        let into = if self.ready { k } else { k - self.start };
        Self {
            clip: self.clip.clone(),
            picture: self.picture.clone(),
            run: self.run.map(|r| r.after(k)),
            source: self.source + into,
            start: self.start,
            ready: true,
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
    /// A wipe's edge, from this direction. Left out, the level is a dissolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wipe: Option<WipeDirection>,
}

pub const WHOLE: u32 = 1 << 16;

pub const BLACK: [u8; 3] = [0, 0, 0];

fn is_black(c: &[u8; 3]) -> bool {
    *c == BLACK
}

impl Ramp {
    pub fn rising(&self, k: i64) -> (i64, i64) {
        (2 * (self.at + k) + 1, 2 * self.of)
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
            at: self.at + k,
            ..*self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wipe {
    pub direction: WipeDirection,
    pub along: (i64, i64),
    pub falling: bool,
}

pub fn wipe_shows_incoming(
    direction: WipeDirection,
    along: (i64, i64),
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> bool {
    let (n, d) = along;
    if d <= 0 || width == 0 || height == 0 {
        return false;
    }
    let n = n.clamp(0, d);
    let (x, y, w, h) = (
        i128::from(x),
        i128::from(y),
        i128::from(width),
        i128::from(height),
    );
    let (n, d) = (i128::from(n), i128::from(d));
    match direction {
        WipeDirection::Left => x * d < n * w,
        WipeDirection::Right => x * d >= (d - n) * w,
        WipeDirection::Up => y * d < n * h,
        WipeDirection::Down => y * d >= (d - n) * h,
    }
}

/// The way the pictures of a Push or a Slide go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Way {
    Push(PushPreset),
    /// The incoming picture slides in.
    Slide(SlidePreset),
    /// The outgoing picture slides out.
    SlideOut(SlidePreset),
}

/// How a Push or a Slide moves its two pictures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Move {
    pub way: Way,
    /// The band on the moving edge, as a part of the shorter side of the
    /// frame.
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
        let (kind, keys) = transitions::moving_played(d.transition, d.frames.1 - d.frames.0)?;
        let (way, motion) = match kind {
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
        let f = (ramp.at + k) as f64;
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

pub type MovingKey = (Way, [u64; 4], bool, [u8; 3]);

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
    pub fn key(&self) -> MovingKey {
        let bits = [self.progress, self.step, self.border, self.blur].map(f64::to_bits);
        (self.way, bits, self.feather, self.color)
    }

    pub fn shift(&self, (w, h): (u32, u32)) -> Shift {
        let size = [f64::from(w), f64::from(h)];
        let b = self.border * size[0].min(size[1]);
        let p = self.progress;
        let (sign, push, out) = match self.way {
            Way::Push(preset) => (push_sign(preset), true, false),
            Way::Slide(preset) => (slide_sign(preset), false, false),
            Way::SlideOut(preset) => (slide_sign(preset), false, true),
        };
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
        }
    }
}

/// What a source piece shows when it isn't a video file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Image {
    /// One picture (the piece's `path`), for every frame.
    Still,
    /// Numbered pictures, one a frame at `rate`: picture `k` is the file
    /// `pattern` (a full path, FFmpeg's image2 form) names with the number
    /// `first + k`. The piece's `path` is the first.
    Sequence {
        pattern: PathBuf,
        first: u64,
        count: u64,
        rate: Rational,
    },
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
        /// Its clip's volume keyframes (decision D81): their gain at each
        /// sample heard, on top of `gain_db`, which then holds the
        /// track's fader only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keyed: Option<Keyed>,
        /// Voice Isolation's Amount over it, from 1 to 100: the clip's own,
        /// else its track's; left out when the clip's own is 0.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        isolation: Option<f64>,
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
        Self::Black {
            frames,
            color: BLACK,
        }
    }
    pub fn frames(&self) -> i64 {
        match self {
            Self::Source { frames, .. } | Self::Black { frames, .. } => *frames,
            Self::Mix { under, .. } | Self::Titled { under, .. } => under.frames(),
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

    pub fn motions(&self) -> Vec<Option<&Move>> {
        match self {
            Self::Mix { under, over } => under
                .motions()
                .into_iter()
                .chain(over.iter().map(|o| o.motion.as_ref()))
                .collect(),
            Self::Titled { under, .. } => under.motions(),
            _ => vec![None],
        }
    }

    pub fn crosses(&self) -> Vec<bool> {
        match self {
            Self::Mix { under, over } => under
                .crosses()
                .into_iter()
                .chain(over.iter().map(|o| o.cross))
                .collect(),
            Self::Titled { under, .. } => under.crosses(),
            _ => vec![false],
        }
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
                super_scale,
                retime,
                placed,
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
                    super_scale: *super_scale,
                    retime: *retime,
                    placed: placed.iter().map(|p| p.after(k)).collect(),
                }
            }
            Self::Black { color, .. } => Self::Black {
                frames,
                color: *color,
            },
            Self::Mix { under, over } => Self::Mix {
                under: Box::new(under.part(k, frames)),
                over: over
                    .iter()
                    .map(|o| Over {
                        piece: o.piece.part(k, frames),
                        ramp: o.ramp.map(|r| r.after(k)),
                        cross: o.cross,
                        motion: o.motion.clone(),
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
            (Self::Black { color: a, .. }, Self::Black { color: b, .. }) => a == b,
            (
                Self::Source {
                    media,
                    source_frame,
                    frames,
                    pace,
                    retime,
                    placed,
                    ..
                },
                Self::Source {
                    media: m2,
                    source_frame: s2,
                    pace: p2,
                    retime: r2,
                    placed: l2,
                    ..
                },
            ) => {
                media == m2
                    && retime == r2
                    && pace.is_normal()
                    && p2.is_normal()
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
                            && a.cross == b.cross
                            && a.motion == b.motion
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
            Self::Source { frames, .. } | Self::Black { frames, .. } => *frames += more,
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

impl AudioPiece {
    pub fn samples(&self) -> i64 {
        match self {
            Self::Source { samples, .. } | Self::Silence { samples } => *samples,
        }
    }
}

impl RenderPlan {
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

pub fn source_frame(x: f64) -> i64 {
    (x + 0.002).floor() as i64
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
            VideoPiece::Black { .. } => 0,
            VideoPiece::Mix { under, .. } | VideoPiece::Titled { under, .. } => under.position(k),
        }
    }

    pub fn exact(&self, k: i64) -> f64 {
        match self {
            VideoPiece::Source {
                source_frame, pace, ..
            } => *source_frame as f64 + pace.exact(k),
            VideoPiece::Black { .. } => 0.0,
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
                isolation,
                keyed,
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
                isolation: i2,
                keyed: k2,
                ..
            },
        ) if media == m2
            && stream == st2
            && gain_db == g2
            && isolation == i2
            && keyed
                .as_ref()
                .map(|k| (&k.keys, k.per_frame, k.at + *samples as f64))
                == k2.as_ref().map(|k| (&k.keys, k.per_frame, k.at))
            && *speed == 1.0
            && *sp2 == 1.0
            && fade.is_none()
            && f2.is_none()
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
    let origin = m
        .info
        .video
        .as_ref()
        .map(|v| v.first_pts)
        .or_else(|| m.info.audio.first().map(|a| a.first_pts))
        .unwrap_or(0.0);
    (m.path.clone(), origin)
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
        Some(s) => Some(Image::Sequence {
            pattern: m.path.parent().unwrap_or(Path::new("")).join(&s.pattern),
            first: s.first,
            count: s.count,
            rate: v.fps,
        }),
        None if v.still => Some(Image::Still),
        None => None,
    }
}

pub(crate) fn plays_inside(c: &Clip) -> bool {
    c.compound.as_ref().is_some_and(|k| !k.tracks.is_empty())
}

#[derive(Debug, Clone, Default)]
struct Nest {
    outer: Vec<Placed>,
    run: Option<Run>,
}

fn run_of(c: &Clip, run: Option<Run>) -> Run {
    run.unwrap_or(Run {
        at: -c.start,
        of: c.duration,
        held: false,
    })
}

type Within = Vec<Vec<Nest>>;

pub const NESTED: usize = 8;

fn expanded(p: &Project) -> Option<(Project, Within)> {
    let holds = |p: &Project| p.tracks.iter().any(|t| t.clips.iter().any(plays_inside));
    if !holds(p) {
        return None;
    }
    let mut out = p.clone();
    let mut within: Within = out
        .tracks
        .iter()
        .map(|t| vec![Nest::default(); t.clips.len()])
        .collect();
    for _ in 0..NESTED {
        if !holds(&out) {
            break;
        }
        let (mut tracks, mut placed) = (Vec::new(), Vec::new());
        for (t, w) in out.tracks.iter().zip(&within) {
            let (mut host, mut host_within) = (t.clone(), Vec::new());
            host.clips.clear();
            let mut inner = Vec::new();
            for (c, nest) in t.clips.iter().zip(w) {
                if !plays_inside(c) {
                    host.clips.push(c.clone());
                    host_within.push(nest.clone());
                    continue;
                }
                if !c.enabled {
                    continue;
                }
                let chain: Vec<Placed> = c
                    .picture
                    .iter()
                    .filter(|x| !x.is_usual())
                    .map(|x| Placed::of(c, x.clone(), run_of(c, nest.run)))
                    .chain(nest.outer.iter().cloned())
                    .collect();
                let k = c.compound.as_ref().expect("plays inside");
                for n in k.tracks.iter().filter(|n| n.kind == t.kind) {
                    let mut v = n.clone();
                    v.id = t.id.clone();
                    v.duck = None;
                    v.muted |= t.muted;
                    v.solo = t.solo;
                    v.volume_db = t.volume_db;
                    v.video_enabled &= t.video_enabled;
                    v.transitions.clear();
                    let mut clips: Vec<(Clip, Nest)> = n
                        .clips
                        .iter()
                        .filter_map(|x| inside(x, c))
                        .map(|(y, run)| {
                            let nest = Nest {
                                outer: chain.clone(),
                                run: Some(run),
                            };
                            (y, nest)
                        })
                        .collect();
                    clips.sort_by_key(|(y, _)| y.start);
                    let (clips, chains) = clips.into_iter().unzip();
                    v.clips = clips;
                    inner.push((v, chains));
                }
            }
            tracks.push(host);
            placed.push(host_within);
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

fn placed_of(c: &Clip, nest: Option<&Nest>, over: bool, x: i64) -> Vec<Placed> {
    let outer = nest.map_or(&[][..], |n| n.outer.as_slice());
    let own = c.picture.clone().filter(|x| !x.is_usual());
    if own.is_none() && outer.is_empty() && !over {
        return Vec::new();
    }
    let run = run_of(c, nest.and_then(|n| n.run)).after(x);
    let own = Placed::at(c, own.unwrap_or_default(), run, x);
    std::iter::once(own)
        .chain(outer.iter().map(|p| p.after(x)))
        .collect()
}

fn inside(x: &Clip, c: &Clip) -> Option<(Clip, Run)> {
    if c.speed.is_some() {
        let speed = c.pace().speed();
        let (k0, k1) = shown_between(c, x.start, x.end())?;
        let mut y = x.clone();
        let walked = c.source_at(c.start + k0) - x.start;
        y.walk(walked);
        (y.start, y.duration) = (c.start + k0, k1 - k0);
        let own = y.speed.take();
        y.speed = paced(c, own.as_ref(), k0);
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
    let (s, e) = (x.start.max(first), x.end().min(last));
    if e <= s {
        return None;
    }
    let mut y = x.clone();
    y.walk(s - x.start);
    y.start = c.start + s - first;
    y.duration = e - s;
    if let Some(a) = y.audio.as_mut()
        && (s > x.start || e < x.end())
    {
        a.head = 0;
        a.tail = 0;
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
    let map = &own.sections;
    let index = |u: f64| map.partition_point(|s| s.at as f64 <= u).saturating_sub(1);
    let pos = |u: f64| {
        let s = &map[index(u)];
        s.from + (u - s.at as f64) * s.speed
    };
    let u0 = own.skip as f64;
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
    let sections: Vec<Section> = bounds
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
        .collect();
    crate::model::Speed {
        sections: sections.into(),
        skip: 0,
        effect: None,
    }
}

enum Layer {
    Picture(VideoPiece, Option<Ramp>),
    Placed(VideoPiece, Option<Ramp>),
    Crossing(VideoPiece, Option<Ramp>),
    Moved(VideoPiece, Ramp, Move, bool),
    Title(Box<ShownTitle>),
}

impl Layer {
    fn inside(&self, outer: &[Placed]) -> bool {
        match self {
            Layer::Placed(VideoPiece::Source { placed, .. }, _)
            | Layer::Crossing(VideoPiece::Source { placed, .. }, _)
            | Layer::Moved(VideoPiece::Source { placed, .. }, ..) => {
                placed.len() > outer.len() && placed.ends_with(outer)
            }
            Layer::Title(t) => !outer.is_empty() && t.placed.ends_with(outer),
            _ => false,
        }
    }
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
    let pool = p.pool();
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t - a, s.sample_rate);

    let visible = |t: &crate::model::Track, c: &Clip| {
        t.video_enabled && c.enabled && c.video && !c.is_block()
    };
    let titled =
        |t: &crate::model::Track, c: &Clip| t.video_enabled && c.enabled && c.title.is_some();
    let (tracks, inside): (Vec<_>, Vec<_>) = p
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TrackKind::Video)
        .map(|(i, t)| (t, within.and_then(|w| w.get(i)).map(Vec::as_slice)))
        .unzip();
    let dissolves: Vec<_> = tracks
        .iter()
        .map(|t| transitions::playing(p, t, false, transitions::mixes))
        .collect();
    let mut cuts = vec![a, b];
    for (t, played) in tracks.iter().zip(&dissolves) {
        for c in t
            .clips
            .iter()
            .filter(|c| c.end() > a && c.start < b && (visible(t, c) || titled(t, c)))
        {
            cuts.push(c.start.max(a));
            cuts.push(c.end().min(b));
            for (s, _) in c.runs(c.start.max(a), c.end().min(b)) {
                cuts.push(s);
            }
        }
        for d in played {
            cuts.extend([d.frames.0, d.frames.1].map(|f| f.clamp(a, b)));
            if transitions::dip_of(d.transition).is_some() {
                cuts.push(middle(d.frames).clamp(a, b));
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let fills = |c: &Clip| {
        c.picture.as_ref().is_none_or(Picture::is_usual)
            && pool
                .get(c.media.as_str())
                .and_then(|m| m.info.video.as_ref())
                .is_some_and(|v| crate::place::fills(v, (s.width, s.height)))
    };
    let picture =
        |c: &Clip, nest: Option<&Nest>, over: bool, x: i64, y: i64| -> Result<VideoPiece> {
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
                super_scale: super_scale_of(m.super_scale, s),
                retime: retime_of(c, x, m, s),
                placed: placed_of(c, nest, over, x),
            })
        };
    let mut video = Vec::new();
    for w in cuts.windows(2) {
        let (x, y) = (w[0], w[1]);
        let mut stack: Vec<Layer> = Vec::new();
        for ((t, played), within) in tracks.iter().zip(&dissolves).zip(&inside) {
            let shows = |c: &&Clip| visible(t, c);
            let shown = |c: &Clip, ramp: Option<Ramp>, below: bool| -> Result<Layer> {
                let nest =
                    within.and_then(|w| w.get(t.clips.iter().position(|x| std::ptr::eq(x, c))?));
                let piece = picture(c, nest, below && !fills(c), x, y)?;
                Ok(match &piece {
                    VideoPiece::Source { placed, .. } if !placed.is_empty() => {
                        Layer::Placed(piece, ramp)
                    }
                    _ => Layer::Picture(piece, ramp),
                })
            };
            match played.iter().find(|d| d.frames.0 <= x && x < d.frames.1) {
                Some(d) if let Some(color) = transitions::dip_of(d.transition) => {
                    let mid = middle(d.frames);
                    let (side, half) = match x < mid {
                        true => (d.before, (x - d.frames.0, mid - d.frames.0, true)),
                        false => (d.after, (x - mid, d.frames.1 - mid, false)),
                    };
                    let half = Some(Ramp {
                        at: half.0,
                        of: half.1,
                        falling: half.2,
                        wipe: None,
                    });
                    let picture = match side.filter(shows) {
                        Some(c) => shown(c, half, !stack.is_empty())?,
                        None => Layer::Picture(VideoPiece::black(y - x), half),
                    };
                    let backdrop = VideoPiece::Black {
                        frames: y - x,
                        color,
                    };
                    stack = vec![Layer::Picture(backdrop, None), picture];
                }
                Some(d) if let Some(m) = Move::of(d) => {
                    let ramp = |falling| Ramp {
                        at: x - d.frames.0,
                        of: d.frames.1 - d.frames.0,
                        falling,
                        wipe: None,
                    };
                    let below = !stack.is_empty();
                    let side = |c: Option<&Clip>| -> Result<(VideoPiece, bool)> {
                        Ok(match c.filter(shows) {
                            Some(c) => match shown(c, None, below)? {
                                Layer::Picture(p, _) => (p, true),
                                Layer::Placed(p, _) => (p, false),
                                _ => unreachable!("a clip shows as a picture"),
                            },
                            None => (VideoPiece::black(y - x), true),
                        })
                    };
                    let ((f, whole_f), (g, whole_g)) = (side(d.before)?, side(d.after)?);
                    if whole_f && whole_g {
                        stack.clear();
                    }
                    stack.push(Layer::Moved(f, ramp(true), m.clone(), false));
                    stack.push(Layer::Moved(g, ramp(false), m, true));
                }
                Some(d) => {
                    let wipe = transitions::wipe_of(d.transition);
                    let ramp = |falling| Ramp {
                        at: x - d.frames.0,
                        of: d.frames.1 - d.frames.0,
                        falling,
                        wipe,
                    };
                    let below = !stack.is_empty();
                    match (d.before.filter(shows), d.after.filter(shows)) {
                        (Some(f), Some(g)) => {
                            let (f, g) =
                                (shown(f, None, below)?, shown(g, Some(ramp(false)), below)?);
                            match (f, g) {
                                (Layer::Picture(f, _), g @ Layer::Picture(..)) => {
                                    stack = vec![Layer::Picture(f, None), g];
                                }
                                (
                                    Layer::Picture(f, _) | Layer::Placed(f, _),
                                    Layer::Picture(g, r) | Layer::Placed(g, r),
                                ) => {
                                    stack.push(Layer::Placed(f, Some(ramp(true))));
                                    stack.push(Layer::Crossing(g, r));
                                }
                                _ => {}
                            }
                        }
                        (Some(f), None) => stack.push(shown(f, Some(ramp(true)), below)?),
                        (None, Some(g)) => stack.push(shown(g, Some(ramp(false)), below)?),
                        (None, None) => {}
                    }
                }
                None => {
                    if let Some(c) = t.clip_covering(x).map(|ci| &t.clips[ci]).filter(shows) {
                        match shown(c, None, !stack.is_empty())? {
                            Layer::Placed(piece, ramp) => {
                                if let VideoPiece::Source { placed, .. } = &piece
                                    && fills(c)
                                {
                                    let outer = &placed[1..];
                                    stack.retain(|l| !l.inside(outer));
                                }
                                stack.push(Layer::Placed(piece, ramp));
                            }
                            l => stack = vec![l],
                        }
                    }
                }
            }
            let lettered = |c: &Clip, fade: Option<Ramp>| {
                let nest =
                    within.and_then(|w| w.get(t.clips.iter().position(|o| std::ptr::eq(o, c))?));
                Some(ShownTitle {
                    clip: c.id.clone(),
                    title: c.title.clone()?,
                    run: Run {
                        at: x - c.start,
                        of: c.duration,
                        held: false,
                    },
                    fade,
                    placed: nest
                        .map_or_else(Vec::new, |n| n.outer.iter().map(|p| p.after(x)).collect()),
                })
            };
            match played.iter().find(|d| d.frames.0 <= x && x < d.frames.1) {
                Some(d) => {
                    let ramp = |falling| Ramp {
                        at: x - d.frames.0,
                        of: d.frames.1 - d.frames.0,
                        falling,
                        wipe: None,
                    };
                    for (c, falling) in [(d.before, true), (d.after, false)] {
                        if let Some(s) = c
                            .filter(|c| titled(t, c))
                            .and_then(|c| lettered(c, Some(ramp(falling))))
                        {
                            stack.push(Layer::Title(Box::new(s)));
                        }
                    }
                }
                None => {
                    if let Some(s) = t
                        .clip_covering(x)
                        .map(|ci| &t.clips[ci])
                        .filter(|c| titled(t, c))
                        .and_then(|c| lettered(c, None))
                    {
                        stack.push(Layer::Title(Box::new(s)));
                    }
                }
            }
        }
        let mut piece = VideoPiece::black(y - x);
        for layer in stack {
            piece = match layer {
                Layer::Picture(shown, None) => shown,
                Layer::Picture(shown, ramp) | Layer::Placed(shown, ramp) => piece.with_over(Over {
                    piece: shown,
                    ramp,
                    cross: false,
                    motion: None,
                }),
                Layer::Crossing(shown, ramp) => piece.with_over(Over {
                    piece: shown,
                    ramp,
                    cross: true,
                    motion: None,
                }),
                Layer::Moved(shown, ramp, m, cross) => piece.with_over(Over {
                    piece: shown,
                    ramp: Some(ramp),
                    cross,
                    motion: Some(m),
                }),
                Layer::Title(t) => piece.with_title(*t),
            };
        }
        push_video(&mut video, piece);
    }

    let any_solo = p.any_solo();
    let mut audio = Vec::new();
    let mut lanes = Vec::new();
    for track in &p.tracks {
        if !track.audible(any_solo) || track.clips.iter().all(|c| c.audio.is_none()) {
            continue;
        }
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
                let (x, y) = (x.max(a), y.min(b));
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
                    for (x, y) in c.runs(x, y) {
                        let mut piece = source_audio(p, c, m, x, sample(y) - sample(x));
                        if let AudioPiece::Source {
                            gain_db,
                            joined,
                            fade: f2,
                            isolation,
                            ..
                        } = &mut piece
                        {
                            *gain_db += track.volume_db;
                            *joined =
                                x > start.max(a) && c.pace().steady(x - 1 - c.start) == Some(1);
                            *f2 = fade(f, x, falling);
                            *isolation = isolation.or(track.voice_isolation).filter(|a| *a > 0.0);
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
    };
    Ok((plan, lanes))
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
    let keyed = audio.is_keyed().then(|| Keyed {
        keys: audio.gain_keys.clone(),
        per_frame,
        at: fps.frame_to_sample(c.content_at(c.start), rate) as f64 + since as f64,
    });
    AudioPiece::Source {
        media: c.media.clone(),
        path,
        stream: audio.stream,
        origin,
        source_sample,
        samples,
        gain_db: if keyed.is_some() { 0.0 } else { audio.gain_db },
        speed: c.pace().rate(x - c.start),
        joined: false,
        fade: None,
        isolation: audio.voice_isolation,
        keyed,
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

pub fn media_frame(p: &Project, media: &str, at: i64) -> Result<VideoPiece> {
    let m = p
        .media(media)
        .ok_or_else(|| Error::not_found(format!("no media {media}")))?;
    Ok(match &m.info.video {
        Some(v) => VideoPiece::Source {
            pace: Default::default(),
            media: media.into(),
            path: m.path.clone(),
            origin: v.first_pts,
            source_frame: at,
            frames: 1,
            image: p.media_image(media),
            color_space: m.color_space,
            super_scale: super_scale_of(m.super_scale, &p.settings),
            retime: Retime::Nearest,
            placed: Vec::new(),
        },
        None => VideoPiece::black(1),
    })
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
                    media: id.clone(),
                    path: path.clone(),
                    origin,
                    source_frame: 0,
                    frames,
                    image: image_of(m),
                    color_space: m.color_space,
                    super_scale: super_scale_of(m.super_scale, &p.settings),
                    retime: Retime::Nearest,
                    placed: Vec::new(),
                },
            );
        } else {
            push_video(&mut video, VideoPiece::black(frames));
        }
        let samples = sample(t + frames) - sample(t);
        if m.info.audio.is_empty() {
            push_audio(&mut lane, AudioPiece::Silence { samples });
        } else {
            push_audio(
                &mut lane,
                AudioPiece::Source {
                    speed: 1.0,
                    joined: false,
                    media: id.clone(),
                    path,
                    stream: 0,
                    origin,
                    source_sample: 0,
                    samples,
                    gain_db: 0.0,
                    fade: None,
                    isolation: None,
                    keyed: None,
                },
            );
        }
        entries.push(TapeEntry {
            media: id.clone(),
            start: t,
            frames,
        });
        t += frames;
    }
    if t == 0 {
        return Err(Error::invalid("no media to show"));
    }
    let plan = RenderPlan {
        fps: s.fps,
        width: s.width,
        height: s.height,
        sample_rate: s.sample_rate,
        channels: s.channels,
        frames: t,
        video,
        audio: vec![lane],
        ducking: Vec::new(),
        range: (0, t),
        name: "tape".into(),
        colors: None,
    };
    Ok((plan, entries))
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
            wipe: None,
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
            wipe: None,
        };
        assert_eq!(ramp.fraction(0), (7, 8));
    }

    #[test]
    fn a_weight_is_the_level_in_sixty_five_thousandths_rounded() {
        let ramp = Ramp {
            at: 0,
            of: 3,
            falling: false,
            wipe: None,
        };
        assert_eq!(ramp.weight(0), 10923);
    }

    #[test]
    fn a_left_wipe_at_its_middle_splits_the_frame_in_half() {
        let incoming: Vec<bool> = (0..4)
            .map(|x| wipe_shows_incoming(WipeDirection::Left, (1, 2), x, 0, 4, 1))
            .collect();
        assert_eq!(incoming, vec![true, true, false, false]);
    }

    #[test]
    fn a_left_wipe_is_on_the_incoming_ramp() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        req(
            &mut f,
            "edit.dissolve",
            json!({"at": 50, "transition": {"type": "wipe", "direction": "left"}}),
        );
        let plan = flatten(f.editor.project(), 0, 100).unwrap();
        let ramp = plan
            .video
            .iter()
            .find_map(|p| match p {
                VideoPiece::Mix { over, .. } => over.iter().find_map(|o| o.ramp),
                _ => None,
            })
            .unwrap();
        assert_eq!(ramp.wipe, Some(WipeDirection::Left));
        assert!(!ramp.falling);
    }

    #[test]
    fn black_and_red_frames_stay_two_pieces() {
        let red = VideoPiece::Black {
            frames: 5,
            color: [255, 0, 0],
        };
        assert!(!VideoPiece::black(5).goes_on_as(&red));
    }

    #[test]
    fn a_dip_to_red_fades_out_over_red_then_in() {
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
            .filter_map(|p| match p {
                VideoPiece::Mix { under, over } => match (&**under, over.first()) {
                    (VideoPiece::Black { color, .. }, Some(o)) => Some((*color, o.ramp?.falling)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(halves, [([255, 0, 0], true), ([255, 0, 0], false)]);
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
            ["m1@0+45", "m2@5+10 at 0/10 over m1@45+10", "m2@15+45"]
        );
    }

    #[test]
    fn a_plan_starting_inside_a_dissolve_starts_its_ramp_there() {
        let mut f = fixture();
        f.append("m1", 0, 50);
        f.append("m2", 10, 60);
        let p = dissolve(&f, Some("c1"), Some("c2"), 5, 5);
        let plan = flatten(&p, 48, 52).unwrap();
        assert_eq!(shown(&plan), ["m2@8+4 at 3/10 over m1@48+4"]);
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
