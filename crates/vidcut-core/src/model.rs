//! The project model: settings, media pool, tracks and clips.
//!
//! The track model follows the Cut page (spec §6): numbered tracks `V1…Vn`
//! hold combined video+audio clips, `A1…An` hold audio-only clips, and V1 is
//! magnetic while Ripple On is set. All positions and lengths are integer
//! frames at the project frame rate; `source_in` counts project frames from the
//! media's first video frame.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::{Rational, TimeContext};

/// Version of the project file format.
pub const FORMAT_VERSION: u32 = 1;

/// A whole project: one timeline (deviation X2) plus its media pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    /// File format version ([`FORMAT_VERSION`]).
    pub vidcut: u32,
    /// Increases with every change, including undo/redo. Mutations may pass
    /// `if_rev` to fail instead of clobbering a concurrent edit.
    pub rev: u64,
    pub settings: Settings,
    /// The media pool, in pool order.
    pub media: Vec<Media>,
    /// Video tracks `V1…` first, then audio tracks `A1…`.
    pub tracks: Vec<Track>,
    /// Timeline In/Out marks.
    #[serde(default)]
    pub marks: Marks,
    #[serde(default)]
    pub markers: Vec<Marker>,
    pub next_id: NextIds,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    pub fps: Rational,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Track 1 ripples on insert, delete, move and trim (the Cut page's Ripple On).
    #[serde(default = "yes")]
    pub ripple_on: bool,
    /// Record timecode of timeline frame 0, in frames (deviation X7: 0, not 01:00:00:00).
    #[serde(default)]
    pub start_tc: i64,
    /// Format not chosen yet: the first video media added sets fps and size.
    #[serde(default)]
    pub auto_format: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fps: Rational::new(30, 1).expect("valid"),
            width: 1920,
            height: 1080,
            sample_rate: 48_000,
            channels: 2,
            ripple_on: true,
            start_tc: 0,
            auto_format: true,
        }
    }
}

/// A file in the media pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Media {
    /// `m<N>`, never reused.
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    /// File size and modification time (Unix ns) when probed; used for cache
    /// keys and to notice changed or missing files.
    pub size: u64,
    pub mtime_ns: i64,
    pub info: MediaInfo,
    /// Source In/Out (spec §4.4; decision D6: remembered per clip). Edits
    /// without an explicit source range use them.
    #[serde(default, skip_serializing_if = "Marks::is_empty")]
    pub marks: Marks,
}

/// What the media layer found out about a file (stored so headless edits never
/// need to re-probe).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MediaInfo {
    /// Demuxer name(s), e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    pub format: String,
    /// Container duration in seconds.
    pub duration: f64,
    /// Container start time in seconds.
    pub start_time: f64,
    pub video: Option<VideoInfo>,
    pub audio: Vec<AudioInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VideoInfo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Sample aspect ratio `[num, den]`.
    pub sar: [i32; 2],
    pub pix_fmt: String,
    /// Average frame rate, snapped to a standard rate when within 0.1%.
    pub fps: Rational,
    /// Variable frame rate.
    #[serde(default)]
    pub vfr: bool,
    /// Raw timestamp of the first frame in seconds (`o` in the time map).
    pub first_pts: f64,
    /// Seconds from the first frame's start to the last frame's end.
    pub duration: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioInfo {
    /// Index among the file's audio streams (0 = first audio stream).
    pub index: u32,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Raw timestamp of the first sample in seconds.
    pub first_pts: f64,
    pub duration: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    /// Combined video+audio clips (`V1`, `V2`, …).
    Video,
    /// Audio-only clips (`A1`, `A2`, …).
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Track {
    /// `V<N>` or `A<N>`, by position.
    pub id: String,
    pub kind: TrackKind,
    /// Sorted by `start`, never overlapping.
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub locked: bool,
    /// Audio of this track is silent.
    #[serde(default)]
    pub muted: bool,
    /// Video of this track is shown (video tracks only).
    #[serde(default = "yes")]
    pub video_enabled: bool,
}

/// A clip on the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Clip {
    /// `c<N>`, never reused.
    pub id: String,
    /// Media id.
    pub media: String,
    /// First timeline frame.
    pub start: i64,
    /// Length in frames (of the video; the audio may differ by J/L offsets).
    pub duration: i64,
    /// Source position of the clip's first frame, in project frames from the
    /// media's first video frame.
    pub source_in: i64,
    /// Whether the clip shows video (false for audio-only clips).
    pub video: bool,
    /// The clip's audio, if it carries any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<ClipAudio>,
    /// Disabled clips are kept but not shown (the Cut page's D key).
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The audio part of a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipAudio {
    /// Index into the media's audio streams.
    pub stream: u32,
    /// Audio starts this many frames after the clip start (negative: before,
    /// i.e. a J cut).
    #[serde(default)]
    pub head: i64,
    /// Audio ends this many frames after the clip end (positive: an L cut).
    #[serde(default)]
    pub tail: i64,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default)]
    pub muted: bool,
}

impl Clip {
    /// First frame after the clip.
    pub fn end(&self) -> i64 {
        self.start + self.duration
    }

    /// Timeline range `[start, end)` of the clip's audio, if any.
    pub fn audio_range(&self) -> Option<(i64, i64)> {
        self.audio
            .as_ref()
            .map(|a| (self.start + a.head, self.end() + a.tail))
    }

    /// Source frame shown at timeline frame `t`.
    pub fn source_at(&self, t: i64) -> i64 {
        self.source_in + (t - self.start)
    }
}

/// In/Out marks (timeline or source); ranges are half-open, so `out` is the
/// first frame after the marked range.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Marks {
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub mark_in: Option<i64>,
    #[serde(rename = "out", default, skip_serializing_if = "Option::is_none")]
    pub mark_out: Option<i64>,
}

impl Marks {
    pub fn is_empty(&self) -> bool {
        self.mark_in.is_none() && self.mark_out.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Marker {
    /// `k<N>`, never reused.
    pub id: String,
    pub at: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_marker_color")]
    pub color: String,
    #[serde(default)]
    pub note: String,
}

fn default_marker_color() -> String {
    "blue".into()
}

/// Counters for new ids.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NextIds {
    pub media: u64,
    pub clip: u64,
    pub marker: u64,
}

impl Project {
    /// An empty project with the given settings and one track, V1.
    pub fn new(settings: Settings) -> Self {
        Self {
            vidcut: FORMAT_VERSION,
            rev: 0,
            settings,
            media: Vec::new(),
            tracks: vec![Track {
                id: "V1".into(),
                kind: TrackKind::Video,
                clips: Vec::new(),
                locked: false,
                muted: false,
                video_enabled: true,
            }],
            marks: Marks::default(),
            markers: Vec::new(),
            next_id: NextIds {
                media: 1,
                clip: 1,
                marker: 1,
            },
        }
    }

    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: &str) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn media(&self, id: &str) -> Option<&Media> {
        self.media.iter().find(|m| m.id == id)
    }

    /// Finds a clip by id: (track index, clip index).
    pub fn find_clip(&self, id: &str) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.clips.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    pub fn clip(&self, id: &str) -> Option<&Clip> {
        self.find_clip(id).map(|(t, c)| &self.tracks[t].clips[c])
    }

    /// End of the program: the latest clip end (video or audio) on any track.
    pub fn end(&self) -> i64 {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.audio_range().map_or(c.end(), |(_, e)| e.max(c.end())))
            .max()
            .unwrap_or(0)
    }

    /// Frames of `media` available from its first video frame (or first audio
    /// sample for audio-only media), at the project frame rate. A frame counts
    /// if it is at least 95% present, so container timestamp rounding (1 ms in
    /// Matroska) doesn't lose the last frame.
    pub fn media_frames(&self, media: &Media) -> i64 {
        let secs = match (&media.info.video, media.info.audio.first()) {
            (Some(v), _) => v.duration,
            (None, Some(a)) => a.duration,
            (None, None) => 0.0,
        };
        (secs * self.settings.fps.as_f64() + 0.05).floor().max(0.0) as i64
    }

    /// Context for resolving TimeSpecs against this project.
    pub fn time_context(&self, playhead: Option<i64>) -> TimeContext {
        TimeContext {
            fps: self.settings.fps,
            start_tc: self.settings.start_tc,
            end: self.end(),
            mark_in: self.marks.mark_in,
            mark_out: self.marks.mark_out,
            playhead,
        }
    }

    pub fn alloc_clip_id(&mut self) -> String {
        let id = format!("c{}", self.next_id.clip);
        self.next_id.clip += 1;
        id
    }

    pub fn alloc_media_id(&mut self) -> String {
        let id = format!("m{}", self.next_id.media);
        self.next_id.media += 1;
        id
    }

    pub fn alloc_marker_id(&mut self) -> String {
        let id = format!("k{}", self.next_id.marker);
        self.next_id.marker += 1;
        id
    }
}

impl Track {
    /// End of the last clip (video extent).
    pub fn end(&self) -> i64 {
        self.clips.last().map_or(0, Clip::end)
    }

    /// Index of the clip covering frame `t`.
    pub fn clip_at(&self, t: i64) -> Option<usize> {
        self.clips.iter().position(|c| c.start <= t && t < c.end())
    }

    /// Sorted, de-duplicated edit points: clip starts and ends, plus 0.
    pub fn edit_points(&self) -> Vec<i64> {
        let mut points: Vec<i64> = std::iter::once(0)
            .chain(self.clips.iter().flat_map(|c| [c.start, c.end()]))
            .collect();
        points.sort_unstable();
        points.dedup();
        points
    }

    /// Restores start order after edits.
    pub fn sort(&mut self) {
        self.clips.sort_by_key(|c| c.start);
    }
}
