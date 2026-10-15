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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Clip, Media, Project, TrackKind};
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
    },
    Silence {
        samples: i64,
    },
}

impl VideoPiece {
    pub fn frames(&self) -> i64 {
        match self {
            Self::Source { frames, .. } | Self::Black { frames } => *frames,
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
    /// The source frame frame `k` of the piece shows (0 for black).
    pub fn position(&self, k: i64) -> i64 {
        match self {
            VideoPiece::Source {
                source_frame, pace, ..
            } => source_frame + pace.offset(k),
            VideoPiece::Black { .. } => 0,
        }
    }
}

fn push_video(out: &mut Vec<VideoPiece>, piece: VideoPiece) {
    if piece.frames() <= 0 {
        return;
    }
    match (out.last_mut(), &piece) {
        (Some(VideoPiece::Black { frames }), VideoPiece::Black { frames: more }) => *frames += more,
        (
            Some(VideoPiece::Source {
                media,
                source_frame,
                frames,
                pace,
                ..
            }),
            VideoPiece::Source {
                media: m2,
                source_frame: s2,
                frames: f2,
                pace: p2,
                ..
            },
        ) if media == m2
            && pace.is_normal()
            && p2.is_normal()
            && *source_frame + *frames == *s2 =>
        {
            *frames += f2
        }
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
                ..
            }),
            AudioPiece::Source {
                media: m2,
                stream: st2,
                source_sample: s2,
                samples: n2,
                gain_db: g2,
                speed: sp2,
                ..
            },
        ) if media == m2
            && stream == st2
            && gain_db == g2
            && *speed == 1.0
            && *sp2 == 1.0
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

/// The pool's items by id, found once for a whole plan.
fn pool_of(p: &Project) -> HashMap<&str, &Media> {
    p.media.iter().map(|m| (m.id.as_str(), m)).collect()
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
fn plays_inside(c: &Clip) -> bool {
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
    let pool = pool_of(p);
    let s = &p.settings;
    let sample = |t: i64| s.fps.frame_to_sample(t - a, s.sample_rate);

    // Video: at every frame the highest video track with a visible clip
    // there shows; clips above cover the ones below.
    // A block from another editor plays as a gap (deviation X9); a compound
    // clip whose clips were found plays them instead (`expanded`).
    let visible = |t: &crate::model::Track, c: &Clip| {
        t.video_enabled && c.enabled && c.video && !c.is_block()
    };
    let tracks: Vec<_> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .collect();
    let mut cuts = vec![a, b];
    for t in &tracks {
        for c in t
            .clips
            .iter()
            .filter(|c| c.end() > a && c.start < b && visible(t, c))
        {
            cuts.push(c.start.max(a));
            cuts.push(c.end().min(b));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut video = Vec::new();
    for w in cuts.windows(2) {
        let (x, y) = (w[0], w[1]);
        let top = tracks.iter().rev().find_map(|t| {
            t.clip_covering(x)
                .map(|ci| &t.clips[ci])
                .filter(|c| visible(t, c))
        });
        match top {
            Some(c) => {
                let m = found(pool.get(c.media.as_str()).copied(), &c.media)?;
                let (path, origin) = origin_of(m);
                push_video(
                    &mut video,
                    VideoPiece::Source {
                        media: c.media.clone(),
                        path,
                        origin,
                        source_frame: c.source_at(x),
                        frames: y - x,
                        image: image_of(m),
                        pace: c.pace().after(x - c.start),
                    },
                );
            }
            None => push_video(&mut video, VideoPiece::Black { frames: y - x }),
        }
    }

    // Audio: one lane per heard track (mute, solo), with its fader.
    let any_solo = p.any_solo();
    let mut audio = Vec::new();
    let mut lanes = Vec::new();
    for track in &p.tracks {
        if !track.audible(any_solo) || track.clips.iter().all(|c| c.audio.is_none()) {
            continue;
        }
        let mut lane = Vec::new();
        let mut t = a;
        for c in &track.clips {
            let Some((ra, rb)) = c.audio_range() else {
                continue;
            };
            let (x, y) = (ra.max(a), rb.min(b));
            if y <= x {
                continue;
            }
            push_audio(
                &mut lane,
                AudioPiece::Silence {
                    samples: sample(x) - sample(t),
                },
            );
            let audio = c.audio.as_ref().expect("has audio");
            if audio.muted {
                push_audio(
                    &mut lane,
                    AudioPiece::Silence {
                        samples: sample(y) - sample(x),
                    },
                );
            } else {
                let m = found(pool.get(c.media.as_str()).copied(), &c.media)?;
                let mut piece = source_audio(p, c, m, x, sample(y) - sample(x));
                if let AudioPiece::Source { gain_db, .. } = &mut piece {
                    *gain_db += track.volume_db;
                }
                push_audio(&mut lane, piece);
            }
            t = y;
        }
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
    }
}

/// Plans one clip on its own: its video extent with its own audio (J/L
/// offsets and neighbors are left out), for per-clip export.
pub fn flatten_clip(p: &Project, clip: &str) -> Result<RenderPlan> {
    let c = p
        .clip(clip)
        .ok_or_else(|| Error::not_found(format!("no clip {clip}")))?
        .clone();
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == c.id);
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
/// covers it on the tracks above (the Trim Editor shows the clip it names).
pub fn clip_frame(p: &Project, clip: &str, at: i64) -> Result<VideoPiece> {
    let mut alone = p.clone();
    for t in &mut alone.tracks {
        t.clips.retain(|x| x.id == clip);
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
    let pool = pool_of(p);
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
