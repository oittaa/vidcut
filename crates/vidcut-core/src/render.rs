//! Render plans: exactly what an export produces, frame by frame for video and
//! sample by sample for audio.
//!
//! A plan covers a timeline range `[a, b)` at the project frame rate. Video is
//! one contiguous list of pieces (source frames or black). Audio is one or more
//! lanes, each a contiguous list of pieces (source samples or silence),
//! that the engine mixes. Every boundary is computed with exact integer math
//! (see [`Rational::frame_to_sample`]), so lengths never drift.
//!
//! **Sampling rule** (docs/decisions/export.md): output frame `j` of a video
//! piece shows the first source frame with timestamp ≥
//! `origin + (source_frame + j) / fps − ½ frame`, i.e. the nearest source
//! frame, ties to the later one. Frame-exact for sources at the project rate.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Clip, Media, Project, TrackKind};
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
    /// Timeline range this plan renders, `[start, end)`.
    pub range: (i64, i64),
    /// A name for the output (per-clip exports: the clip's name).
    pub name: String,
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
    },
    Black {
        frames: i64,
    },
    /// Pictures over each other while a dissolve plays (decision D50):
    /// `under`, then each of `over` on top of it at its level. Each is a
    /// source or black, as long as the piece.
    Mix {
        under: Box<VideoPiece>,
        over: Vec<Over>,
    },
}

/// A picture over the others of a [`VideoPiece::Mix`], at a level its
/// ramp gives each frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Over {
    pub piece: VideoPiece,
    pub ramp: Ramp,
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
}

/// The weight [`Ramp::weight`] gives a picture shown whole.
pub const WHOLE: u32 = 1 << 16;

impl Ramp {
    /// The level at the piece's frame `k`, a fraction: its numerator and
    /// denominator. Frame `i` of `n` is at `(i + 1/2) / n`.
    pub fn fraction(&self, k: i64) -> (i64, i64) {
        let rising = (2 * (self.at + k) + 1, 2 * self.of);
        if self.falling {
            (rising.1 - rising.0, rising.1)
        } else {
            rising
        }
    }

    /// The level at the piece's frame `k`, from 0 to 1.
    pub fn level(&self, k: i64) -> f64 {
        let (n, d) = self.fraction(k);
        n as f64 / d as f64
    }

    /// The level at the piece's frame `k` in sixty-five-thousandths
    /// ([`WHOLE`] is 1), rounded: what pictures are mixed with.
    pub fn weight(&self, k: i64) -> u32 {
        let (n, d) = self.fraction(k);
        let w = (i128::from(n) * i128::from(WHOLE) * 2 + i128::from(d)) / (2 * i128::from(d));
        w.clamp(0, i128::from(WHOLE)) as u32
    }

    /// The ramp `k` frames on.
    pub fn after(&self, k: i64) -> Self {
        Self {
            at: self.at + k,
            ..*self
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
        /// A cross fade over it (decision D50): its gain on top of
        /// `gain_db`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fade: Option<Fade>,
    },
    Silence {
        samples: i64,
    },
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
    /// The gain at share `x` of the way.
    pub fn gain(self, x: f64) -> f64 {
        match self {
            Self::Plus3Db => x.sqrt(),
            Self::ZeroDb => x,
            Self::Minus3Db => x * x.sqrt(),
        }
    }
}

impl Fade {
    /// The gain at the piece's sample `s`, 0 to 1: a rising one is silent
    /// at the fade's first sample, a falling one whole.
    pub fn gain(&self, s: i64) -> f64 {
        let x = ((self.at + s) as f64 / self.of as f64).clamp(0.0, 1.0);
        self.curve.gain(if self.falling { 1.0 - x } else { x })
    }

    /// It starts silent: rising from its first sample.
    pub fn starts_silent(&self) -> bool {
        !self.falling && self.at == 0
    }

    /// A piece of `samples` it is on ends silent: falling to its last.
    pub fn ends_silent(&self, samples: i64) -> bool {
        self.falling && self.at + samples >= self.of
    }
}

impl VideoPiece {
    pub fn frames(&self) -> i64 {
        match self {
            Self::Source { frames, .. } | Self::Black { frames } => *frames,
            Self::Mix { under, .. } => under.frames(),
        }
    }

    /// The pieces it shows, bottom up, each with the ramp of its level:
    /// itself, or a mix's `under` and `over`.
    pub fn layers(&self) -> Vec<(&VideoPiece, Option<Ramp>)> {
        match self {
            Self::Mix { under, over } => std::iter::once((under.as_ref(), None))
                .chain(over.iter().map(|o| (&o.piece, Some(o.ramp))))
                .collect(),
            _ => vec![(self, None)],
        }
    }

    /// [`layers`](Self::layers), to change.
    pub fn layers_mut(&mut self) -> Vec<&mut VideoPiece> {
        match self {
            Self::Mix { under, over } => std::iter::once(under.as_mut())
                .chain(over.iter_mut().map(|o| &mut o.piece))
                .collect(),
            _ => vec![self],
        }
    }

    /// The piece from its frame `k` on, `frames` long.
    pub fn part(&self, k: i64, frames: i64) -> VideoPiece {
        match self {
            Self::Source {
                media,
                path,
                origin,
                image,
                pace,
                ..
            } => {
                // A still's piece always reads its one picture from the start.
                let still = matches!(image, Some(Image::Still));
                Self::Source {
                    media: media.clone(),
                    path: path.clone(),
                    origin: *origin,
                    source_frame: self.position(if still { 0 } else { k }),
                    frames,
                    image: image.clone(),
                    pace: if still { *pace } else { pace.after(k) },
                }
            }
            Self::Black { .. } => Self::Black { frames },
            Self::Mix { under, over } => Self::Mix {
                under: Box::new(under.part(k, frames)),
                over: over
                    .iter()
                    .map(|o| Over {
                        piece: o.piece.part(k, frames),
                        ramp: o.ramp.after(k),
                    })
                    .collect(),
            },
        }
    }

    /// Whether `next` shows what comes after it, so the two are one piece.
    fn goes_on_as(&self, next: &VideoPiece) -> bool {
        match (self, next) {
            (Self::Black { .. }, Self::Black { .. }) => true,
            (
                Self::Source {
                    media,
                    source_frame,
                    frames,
                    pace,
                    ..
                },
                Self::Source {
                    media: m2,
                    source_frame: s2,
                    pace: p2,
                    ..
                },
            ) => {
                media == m2 && pace.is_normal() && p2.is_normal() && *source_frame + *frames == *s2
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
                    && over
                        .iter()
                        .zip(o2)
                        .all(|(a, b)| a.ramp.after(k) == b.ramp && a.piece.goes_on_as(&b.piece))
            }
            _ => false,
        }
    }

    /// Makes it `more` frames longer.
    fn lengthen(&mut self, more: i64) {
        match self {
            Self::Source { frames, .. } | Self::Black { frames } => *frames += more,
            Self::Mix { under, over } => {
                under.lengthen(more);
                for o in over {
                    o.piece.lengthen(more);
                }
            }
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
    /// Total audio samples (per channel).
    pub fn samples(&self) -> i64 {
        self.fps.frame_to_sample(self.frames, self.sample_rate)
    }
}

/// How a picture piece walks its source (decision D48): frame `k` of it
/// shows source frame `source_frame + offset(k)`, by the sampling rule
/// (export.md §2): at speed 2 every other frame, at 0.5 each twice, at -1
/// backward, at 0 the first held. Each frame shows the source frame its
/// stretch of the source starts in, counted forward: backward, the bottom
/// of the stretch, as editors show it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Pace {
    /// Source frames per output frame.
    pub speed: f64,
    /// Frames of its clip before the piece: where in its pace it starts.
    pub skip: i64,
}

impl Default for Pace {
    fn default() -> Self {
        Self::of(1.0, 0)
    }
}

impl Pace {
    pub fn of(speed: f64, skip: i64) -> Self {
        Self { speed, skip }
    }

    pub fn is_normal(&self) -> bool {
        self.speed == 1.0
    }

    /// Source frames from the piece's first to its frame `k`.
    pub fn offset(&self, k: i64) -> i64 {
        if self.is_normal() {
            return k;
        }
        // A billionth over: 3 frames at a third of the speed make one.
        let at = |n: i64| (n as f64 * self.speed + 1e-9).floor() as i64;
        // Backward, frame k's stretch ends where frame k + 1's starts.
        let back = i64::from(self.speed < 0.0);
        at(self.skip + k + back) - at(self.skip + back)
    }

    /// The pace `k` frames on.
    pub fn after(&self, k: i64) -> Self {
        Self::of(self.speed, self.skip + k)
    }

    /// How far into its first frame's source frame (0 to 1) that frame's
    /// stretch of the source starts: forward its start, backward its
    /// bottom.
    pub fn phase(&self) -> f64 {
        if self.is_normal() || self.speed == 0.0 {
            return 0.0;
        }
        let back = i64::from(self.speed < 0.0);
        let x = (self.skip + back) as f64 * self.speed;
        (x - (x + 1e-9).floor()).max(0.0)
    }

    /// Where its sound is `x` frames into it (between frames too), in source
    /// frames from its first frame's: forward from where that frame's
    /// stretch starts, backward from its top.
    pub fn sound(&self, x: f64) -> f64 {
        let top = if self.speed < 0.0 { -self.speed } else { 0.0 };
        self.phase() + top + x * self.speed
    }
}

impl VideoPiece {
    /// The source frame frame `k` of the piece shows (0 for black; a
    /// mix's, that of the picture under the others).
    pub fn position(&self, k: i64) -> i64 {
        match self {
            VideoPiece::Source {
                source_frame, pace, ..
            } => source_frame + pace.offset(k),
            VideoPiece::Black { .. } => 0,
            VideoPiece::Mix { under, .. } => under.position(k),
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
                ..
            },
        ) if media == m2
            && stream == st2
            && gain_db == g2
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

    /// How `media`'s pictures are read, if not as a video file's frames.
    pub fn media_image(&self, media: &str) -> Option<Image> {
        image_of(self.media(media)?)
    }
}

/// Pool item `id`, or that there is none.
fn found<'a>(m: Option<&'a Media>, id: &str) -> Result<&'a Media> {
    m.ok_or_else(|| Error::not_found(format!("no media {id}")))
}

/// [`Project::media_origin`] of `m`.
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

/// [`Project::media_image`] of `m`.
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

/// Whether `c` is a compound clip from another editor whose clips play.
pub(crate) fn plays_inside(c: &Clip) -> bool {
    c.compound.as_ref().is_some_and(|k| !k.tracks.is_empty())
}

/// `p` as it plays when it holds compound clips from another editor
/// (deviation X9): each replaced by the clips it holds, trimmed to the part
/// it shows, on tracks of their own just above its track with that track's
/// id, fader, mute and solo; those it holds in turn too. A disabled one
/// plays nothing.
fn expanded(p: &Project) -> Option<Project> {
    let holds = |p: &Project| p.tracks.iter().any(|t| t.clips.iter().any(plays_inside));
    if !holds(p) {
        return None;
    }
    let mut out = p.clone();
    // Compound clips inside compound clips, a few deep.
    for _ in 0..8 {
        if !holds(&out) {
            break;
        }
        let mut tracks = Vec::new();
        for t in &out.tracks {
            let mut host = t.clone();
            host.clips.retain(|c| !plays_inside(c));
            let mut inner = Vec::new();
            for c in t.clips.iter().filter(|c| plays_inside(c) && c.enabled) {
                let k = c.compound.as_ref().expect("plays inside");
                for n in k.tracks.iter().filter(|n| n.kind == t.kind) {
                    let mut v = n.clone();
                    v.id = t.id.clone();
                    v.muted |= t.muted;
                    v.solo = t.solo;
                    v.volume_db = t.volume_db;
                    v.video_enabled &= t.video_enabled;
                    v.transitions.clear();
                    v.clips = n.clips.iter().filter_map(|x| inside(x, c)).collect();
                    // Backward, its last clip comes first.
                    v.clips.sort_by_key(|y| y.start);
                    inner.push(v);
                }
            }
            tracks.push(host);
            tracks.extend(inner);
        }
        out.tracks = tracks;
    }
    Some(out)
}

/// Clip `x` of compound clip `c`, trimmed to the part `c` shows and placed
/// where it shows it; at `c`'s speed (decision D48), through its pace.
fn inside(x: &Clip, c: &Clip) -> Option<Clip> {
    if c.speed() != 1.0 {
        let (k0, k1) = shown_between(c, x.start, x.end())?;
        let mut y = x.clone();
        y.walk(c.source_at(c.start + k0) - x.start);
        (y.start, y.duration) = (c.start + k0, k1 - k0);
        y.speed = paced(c, x.speed.as_ref(), k0);
        if let Some(a) = y.audio.as_mut() {
            a.head = 0;
            a.tail = 0;
        }
        return Some(y);
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
    Some(y)
}

/// The frames of compound clip `c` (from its first) that show its
/// timeline's frames `[a, b)`, at its speed: none when it shows none of them.
pub(crate) fn shown_between(c: &Clip, a: i64, b: i64) -> Option<(i64, i64)> {
    let shows = |k: i64| (a..b).contains(&c.source_at(c.start + k));
    let k0 = (0..c.duration).find(|&k| shows(k))?;
    let k1 = (k0..c.duration).find(|&k| !shows(k)).unwrap_or(c.duration);
    Some((k0, k1))
}

/// The speed of a clip held by compound clip `c` at `own` speed, shown from
/// `c`'s frame `k`: the two speeds together. One at normal speed walks
/// `c`'s pace exactly.
pub(crate) fn paced(
    c: &Clip,
    own: Option<&crate::model::Speed>,
    k: i64,
) -> Option<crate::model::Speed> {
    let speed = c.speed() * own.and_then(crate::model::Speed::scalar).unwrap_or(1.0);
    (speed != 1.0).then(|| crate::model::Speed {
        effect: crate::ops::time_effect(own.or(c.speed.as_ref()), speed).into(),
        skip: if own.is_none() {
            c.pace().after(k).skip
        } else {
            0
        },
    })
}

/// Plans the timeline range `[a, b)`.
pub fn flatten(p: &Project, a: i64, b: i64) -> Result<RenderPlan> {
    flatten_lanes(p, a, b).map(|(plan, _)| plan)
}

/// The track an audio lane of [`flatten_lanes`] comes from, and its fader,
/// which the lane's gains include.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneTrack {
    pub track: String,
    pub volume_db: f64,
}

/// [`flatten`], with the track of each audio lane, in lane order: the
/// preview controls track faders apart from clip gains.
pub fn flatten_lanes(p: &Project, a: i64, b: i64) -> Result<(RenderPlan, Vec<LaneTrack>)> {
    if a < 0 || b <= a {
        return Err(Error::invalid(format!("nothing to render in {a}..{b}")));
    }
    let expanded = expanded(p);
    let p = expanded.as_ref().unwrap_or(p);
    let pool = p.pool();
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t - a, s.sample_rate);

    // Video: at every frame the highest video track with a visible clip
    // there shows; clips above cover the ones below.
    // A block from another editor plays as a gap (deviation X9); a compound
    // clip whose clips were found plays them instead (`expanded`).
    // A dissolve mixes the clips on each side of its cut; one into or out
    // of a gap shows what is under it there (decision D50).
    let visible = |t: &crate::model::Track, c: &Clip| {
        t.video_enabled && c.enabled && c.video && !c.is_block()
    };
    let tracks: Vec<_> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .collect();
    let dissolves: Vec<_> = tracks
        .iter()
        .map(|t| transitions::playing(p, t, false, transitions::dissolves))
        .collect();
    let mut cuts = vec![a, b];
    for (t, played) in tracks.iter().zip(&dissolves) {
        for c in t
            .clips
            .iter()
            .filter(|c| c.end() > a && c.start < b && visible(t, c))
        {
            cuts.push(c.start.max(a));
            cuts.push(c.end().min(b));
        }
        for d in played {
            cuts.extend([d.frames.0, d.frames.1].map(|f| f.clamp(a, b)));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    // Clip `c`'s picture over `[x, y)`.
    let picture = |c: &Clip, x: i64, y: i64| -> Result<VideoPiece> {
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
        })
    };
    let mut video = Vec::new();
    for w in cuts.windows(2) {
        let (x, y) = (w[0], w[1]);
        // Bottom up: the first is shown whole, the others over it.
        let mut stack: Vec<(VideoPiece, Option<Ramp>)> = Vec::new();
        for (t, played) in tracks.iter().zip(&dissolves) {
            let shows = |c: &&Clip| visible(t, c);
            match played.iter().find(|d| d.frames.0 <= x && x < d.frames.1) {
                Some(d) => {
                    let ramp = |falling| Ramp {
                        at: x - d.frames.0,
                        of: d.frames.1 - d.frames.0,
                        falling,
                    };
                    match (d.before.filter(shows), d.after.filter(shows)) {
                        (Some(f), Some(g)) => {
                            stack = vec![(picture(f, x, y)?, None)];
                            stack.push((picture(g, x, y)?, Some(ramp(false))));
                        }
                        (Some(f), None) => stack.push((picture(f, x, y)?, Some(ramp(true)))),
                        (None, Some(g)) => stack.push((picture(g, x, y)?, Some(ramp(false)))),
                        (None, None) => {}
                    }
                }
                None => {
                    if let Some(c) = t.clip_covering(x).map(|ci| &t.clips[ci]).filter(shows) {
                        stack = vec![(picture(c, x, y)?, None)];
                    }
                }
            }
        }
        let mut layers = stack.into_iter();
        let under = match layers.next() {
            Some((piece, None)) => piece,
            first => {
                layers = first
                    .into_iter()
                    .chain(layers)
                    .collect::<Vec<_>>()
                    .into_iter();
                VideoPiece::Black { frames: y - x }
            }
        };
        let over: Vec<Over> = layers
            .map(|(piece, ramp)| Over {
                piece,
                ramp: ramp.expect("only the first is shown whole"),
            })
            .collect();
        push_video(
            &mut video,
            if over.is_empty() {
                under
            } else {
                VideoPiece::Mix {
                    under: Box::new(under),
                    over,
                }
            },
        );
    }

    // Audio: lanes per heard track (mute, solo), with its fader. A cross
    // fade plays both clips' sounds through its frames, the one after the
    // cut on the track's other lane from then on (decision D50).
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
            // Counted from the timeline's start, so a plan starting inside
            // a fade gives each sample the gain the whole timeline's does.
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
                    let mut piece = source_audio(p, c, m, x, sample(y) - sample(x));
                    if let AudioPiece::Source {
                        gain_db, fade: f2, ..
                    } = &mut piece
                    {
                        *gain_db += track.volume_db;
                        *f2 = fade(f, x, falling);
                    }
                    push_audio(lane, piece);
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

    let plan = RenderPlan {
        fps: s.fps,
        width: s.width,
        height: s.height,
        sample_rate: s.sample_rate,
        channels: s.channels,
        frames: b - a,
        video,
        audio,
        range: (a, b),
        name: "timeline".into(),
    };
    Ok((plan, lanes))
}

/// Audio of clip `c` from timeline frame `x`, `samples` long. At its speed
/// (decision D48) the sound of the frames it shows, held frames silent;
/// backward from the top of the first frame's stretch.
fn source_audio(p: &Project, c: &Clip, m: &Media, x: i64, samples: i64) -> AudioPiece {
    let audio = c.audio.as_ref().expect("has audio");
    let (path, origin) = origin_of(m);
    let (fps, rate) = (p.settings.fps, p.settings.sample_rate);
    let speed = c.speed();
    let source_sample = if speed == 1.0 {
        fps.frame_to_sample(c.source_at(x), rate)
    } else if speed == 0.0 {
        return AudioPiece::Silence { samples };
    } else {
        let per_frame = f64::from(rate) / fps.as_f64();
        let since = fps.frame_to_sample(x, rate) - fps.frame_to_sample(c.start, rate);
        fps.frame_to_sample(c.source_in, rate)
            + (c.pace().sound(0.0) * per_frame + since as f64 * speed).round() as i64
    };
    AudioPiece::Source {
        media: c.media.clone(),
        path,
        stream: audio.stream,
        origin,
        source_sample,
        samples,
        gain_db: audio.gain_db,
        speed,
        fade: None,
    }
}

/// Plans one clip on its own: its video extent with its own audio (J/L
/// offsets, neighbors and dissolves are left out), for per-clip export.
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
    plan.name = c
        .name
        .clone()
        .or_else(|| p.media(&c.media).map(|m| stem(&m.name)))
        .unwrap_or_else(|| c.id.clone());
    Ok(plan)
}

/// Timeline frame `at` of clip `clip` alone, as export samples it, whatever
/// covers it on the tracks above and without its dissolves (the Trim Editor
/// shows the clip it names).
pub fn clip_frame(p: &Project, clip: &str, at: i64) -> Result<VideoPiece> {
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == clip);
        t.transitions.clear();
    }
    flatten(&alone, at, at + 1)?
        .video
        .into_iter()
        .next()
        .ok_or_else(|| Error::not_found(format!("no frame {at} of {clip}")))
}

/// Frame `at` of media item `media` (project frames from its first video
/// frame), as export samples it; black for media without video.
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
        },
        None => VideoPiece::Black { frames: 1 },
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

/// Plans whole media items laid end to end: the Source Tape (spec §4.1), or
/// one item for the Source Clip viewer. Media without video shows black and
/// media without audio is silent; empty media is left out.
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
                },
            );
        } else {
            push_video(&mut video, VideoPiece::Black { frames });
        }
        let samples = sample(t + frames) - sample(t);
        if m.info.audio.is_empty() {
            push_audio(&mut lane, AudioPiece::Silence { samples });
        } else {
            push_audio(
                &mut lane,
                AudioPiece::Source {
                    speed: 1.0,
                    media: id.clone(),
                    path,
                    stream: 0,
                    origin,
                    source_sample: 0,
                    samples,
                    gain_db: 0.0,
                    fade: None,
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
        range: (0, t),
        name: "tape".into(),
    };
    Ok((plan, entries))
}

/// One media range covered by a Source Tape span (decision D17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanPiece {
    pub media: String,
    /// Source frames `[start, end)`.
    pub range: (i64, i64),
}

/// The pieces of a half-open tape span: the tail of the first clip, every
/// whole clip in between, and the head of the last (decision D17).
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

/// Where a lone In or Out stays on its own clip (decision D5). Both marks
/// use the tape range as given, which may cross clips. `None` when neither
/// mark is set or a lone mark does not fall on a clip.
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

/// Both marks set and covering more than one clip (decision D17).
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
        // 25 fps project; m1 10 s with audio, m2 4 s with audio, m3 8 s silent.
        Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)])
    }

    fn req(f: &mut Fixture, method: &str, params: serde_json::Value) {
        f.ok(json!({"method": method, "params": params}));
    }

    /// A dissolve on V1's pictures from clip `from` to clip `to` (`None`:
    /// a gap), `before` frames before the cut and `after` after it.
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

    /// A cross fade on V1's sounds from clip `from` to clip `to` (`None`: a
    /// gap): a Cross Fade +3 dB in the editor metadata.
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

    /// The plan's sounds in short, lane by lane: `m1@0+86400` (media, first
    /// source sample, samples), `silence+86400`, a cross fade's piece with
    /// its curve and where in the fade it starts.
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
        // 1920 samples a frame.
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
        // A quarter of the way through +3 dB: the square root of a quarter.
        assert_eq!(transitions::heard_at(&p, t, &t.clips[1], 49), Some(0.5));
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

    /// The plan's pictures in short: `m1@45+10` (media, first source
    /// frame, frames), `black+10`, a mix as its pictures over the one under.
    fn shown(plan: &RenderPlan) -> Vec<String> {
        fn one(p: &VideoPiece) -> String {
            match p {
                VideoPiece::Source {
                    media,
                    source_frame,
                    frames,
                    ..
                } => format!("{media}@{source_frame}+{frames}"),
                VideoPiece::Black { frames } => format!("black+{frames}"),
                VideoPiece::Mix { under, over } => {
                    let over: Vec<String> = over
                        .iter()
                        .map(|o| {
                            let fall = if o.ramp.falling { " falling" } else { "" };
                            format!("{} at {}/{}{fall}", one(&o.piece), o.ramp.at, o.ramp.of)
                        })
                        .collect();
                    format!("{} over {}", over.join(", "), one(under))
                }
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
        };
        assert_eq!(ramp.fraction(0), (7, 8));
    }

    #[test]
    fn a_weight_is_the_level_in_sixty_five_thousandths_rounded() {
        let ramp = Ramp {
            at: 0,
            of: 3,
            falling: false,
        };
        // 65536 / 6 = 10922.67.
        assert_eq!(ramp.weight(0), 10923);
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
                VideoPiece::Black { frames } => ("black".into(), *frames),
                VideoPiece::Mix { .. } => ("mix".into(), v.frames()),
            })
            .collect();
        // c1 = m1 0..100; the lifted gap and disabled c3 merge into one black piece.
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
        // Decision D48: forward the frame a stretch starts in, backward the
        // one it starts in from the top; a piece cut later keeps the pairs.
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
