//! The project model: settings, media pool, tracks and clips.
//!
//! The track model follows the Cut page (spec §6): numbered tracks `V1…Vn`
//! hold combined video+audio clips, `A1…An` hold audio-only clips, and V1 is
//! magnetic while Ripple On is set. All positions and lengths are integer
//! frames at the project frame rate; `source_in` counts project frames from the
//! media's first video frame.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::{Rational, TimeContext};

/// Version of the project file format.
pub const FORMAT_VERSION: u32 = 1;

/// How long a still may last in the timeline, seconds: 17 h 40 min, as in
/// Resolve (p. 1194).
pub const STILL_LIMIT_SECS: f64 = 63_600.0;

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
    pub marks: Marks,
    pub markers: Vec<Marker>,
    pub next_id: NextIds,
    /// Media Pool bins (p. 655), as `/`-separated paths below the top level
    /// (`Interviews`, `Interviews/Day 1`), sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    pub fps: Rational,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Track 1 ripples on insert, delete, move and trim (the Cut page's Ripple On).
    pub ripple_on: bool,
    /// Record timecode of timeline frame 0, in frames (deviation X7: 0, not 01:00:00:00).
    pub start_tc: i64,
    /// Format not chosen yet: the first video media added sets fps and size.
    pub auto_format: bool,
    /// How long a still lasts when edited without a range or marks, and
    /// what the viewer shows of it, in seconds: Resolve's standard still
    /// duration (p. 114; decision D38).
    pub still_secs: f64,
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
            still_secs: 5.0,
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
    /// The bin it is in (a path from [`Project::bins`]); empty: the top level.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bin: String,
    /// For a numbered image sequence, its files; `path` is the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<ImageSequence>,
}

impl Media {
    /// A single picture, shown for as long as its clips last.
    pub fn is_still(&self) -> bool {
        self.info.video.as_ref().is_some_and(|v| v.still)
    }

    /// The file of picture `k` of an image sequence.
    pub fn sequence_file(&self, k: u64) -> Option<PathBuf> {
        let s = self.sequence.as_ref()?;
        Some(s.file(self.path.parent().unwrap_or(Path::new("")), k))
    }
}

/// Numbered image files making one clip (p. 366): the pictures of a render
/// or a scan, one file per frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ImageSequence {
    /// The files' name with the number as FFmpeg's image2 demuxer reads it:
    /// `%06d` for six zero-padded digits, `%d` for unpadded ones, `%%` for
    /// a percent sign (`frame_%06d.png`).
    pub pattern: String,
    /// The first file's number, and how many files follow in a row.
    pub first: u64,
    pub count: u64,
}

impl ImageSequence {
    /// The pattern with its number written by `number` (given the padded
    /// width, 0 for none).
    fn fill(&self, mut number: impl FnMut(usize) -> String) -> String {
        let mut out = String::with_capacity(self.pattern.len() + 16);
        let mut rest = self.pattern.as_str();
        while let Some(i) = rest.find('%') {
            out.push_str(&rest[..i]);
            rest = &rest[i + 1..];
            if let Some(r) = rest.strip_prefix('%') {
                out.push('%');
                rest = r;
                continue;
            }
            let digits = rest.find('d').unwrap_or(0);
            out.push_str(&number(rest[..digits].parse().unwrap_or(0)));
            rest = &rest[(digits + 1).min(rest.len())..];
        }
        out.push_str(rest);
        out
    }

    /// The name around the number, and the width it is zero-padded to (0:
    /// not padded): OpenTimelineIO's `name_prefix`, `frame_zero_padding`
    /// and `name_suffix`.
    pub fn parts(&self) -> (String, usize, String) {
        let mut width = None;
        let marked = self.fill(|w| {
            width = Some(w);
            "\u{0}".into()
        });
        let (prefix, suffix) = marked.split_once('\u{0}').unwrap_or((&marked, ""));
        (prefix.to_string(), width.unwrap_or(0), suffix.to_string())
    }

    /// The name of the file numbered `n`.
    pub fn name(&self, n: u64) -> String {
        self.fill(|w| format!("{n:0w$}"))
    }

    /// The pool's name for it, as Resolve shows one: `frame_[000-119].png`.
    pub fn display(&self) -> String {
        let last = self.first + self.count.saturating_sub(1);
        self.fill(|w| format!("[{:0w$}-{last:0w$}]", self.first))
    }

    /// Picture `k`'s file, in `dir`.
    pub fn file(&self, dir: &Path, k: u64) -> PathBuf {
        dir.join(self.name(self.first + k))
    }
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
    pub vfr: bool,
    /// Raw timestamp of the first frame in seconds (`o` in the time map).
    pub first_pts: f64,
    /// Seconds from the first frame's start to the last frame's end.
    pub duration: f64,
    /// Clockwise rotation (0, 90, 180 or 270) that shows the stored pictures
    /// upright: phones store portrait video sideways with this.
    pub rotation: u16,
    /// For interlaced video, the field shown first; `None` when progressive.
    pub fields: Option<FieldOrder>,
    /// The HDR transfer; `None` for SDR video.
    pub hdr: Option<Hdr>,
    /// A single picture (a still image file): it lasts as long as an edit
    /// makes it, and `fps` and `duration` mean nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub still: bool,
}

impl VideoInfo {
    /// The picture size as shown: square pixels (the sample aspect ratio
    /// applied to the width), rotated, even.
    pub fn display_size(&self) -> (u32, u32) {
        let sar = if self.sar[0] > 0 && self.sar[1] > 0 {
            f64::from(self.sar[0]) / f64::from(self.sar[1])
        } else {
            1.0
        };
        let w = ((f64::from(self.width) * sar).round() as u32).max(2) & !1;
        let h = self.height.max(2) & !1;
        if self.rotation % 180 == 90 {
            (h, w)
        } else {
            (w, h)
        }
    }
}

/// Which field of an interlaced frame is shown first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FieldOrder {
    Top,
    Bottom,
}

/// High dynamic range transfer functions (ITU-R BT.2100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Hdr {
    /// Perceptual quantizer (SMPTE ST 2084): HDR10, Dolby Vision.
    Pq,
    /// Hybrid log-gamma (ARIB STD-B67): broadcast and phone HDR.
    Hlg,
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
    /// `V<N>` or `A<N>`, by position: video tracks count up from V1, audio
    /// tracks from A1.
    pub id: String,
    pub kind: TrackKind,
    /// Sorted by `start`, never overlapping.
    pub clips: Vec<Clip>,
    pub locked: bool,
    /// Audio of this track is silent.
    pub muted: bool,
    /// While any track is soloed, only soloed tracks are heard (the mixer,
    /// p. 650).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    /// The mixer fader, in dB (decision D12's range, -60..24).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub volume_db: f64,
    /// Video of this track is shown (video tracks only).
    pub video_enabled: bool,
    /// A name shown in the track header and the mixer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
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
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A clip with the track it is on: what `edit.copy` returns and
/// `edit.paste` takes (paste puts clips back on their own tracks, p. 723).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackClip {
    pub track: String,
    #[serde(flatten)]
    pub clip: Clip,
}

/// The audio part of a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipAudio {
    /// Index into the media's audio streams.
    pub stream: u32,
    /// Audio starts this many frames after the clip start (negative: before,
    /// i.e. a J cut).
    pub head: i64,
    /// Audio ends this many frames after the clip end (positive: an L cut).
    pub tail: i64,
    pub gain_db: f64,
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
    pub name: String,
    pub color: String,
    pub note: String,
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
            tracks: vec![Track::new("V1", TrackKind::Video)],
            marks: Marks::default(),
            markers: Vec::new(),
            next_id: NextIds {
                media: 1,
                clip: 1,
                marker: 1,
            },
            bins: Vec::new(),
        }
    }

    /// Index of track `id` in [`Project::tracks`].
    pub fn track_index(&self, id: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// Indexes of the tracks of `kind`, bottom (V1, A1) first.
    pub fn tracks_of(&self, kind: TrackKind) -> Vec<usize> {
        (0..self.tracks.len())
            .filter(|&i| self.tracks[i].kind == kind)
            .collect()
    }

    /// Whether any track is soloed (then only soloed tracks are heard).
    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
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
    /// sample for audio-only media), at the project frame rate: what an edit
    /// takes without a range or marks, and what the viewer shows. A frame
    /// counts if it is at least 95% present, so container timestamp rounding
    /// (1 ms in Matroska) doesn't lose the last frame. A still lasts the
    /// project's still duration here (p. 114), though a clip may make it
    /// longer ([`media_limit`](Self::media_limit)).
    pub fn media_frames(&self, media: &Media) -> i64 {
        let secs = match (&media.info.video, media.info.audio.first()) {
            (Some(v), _) if v.still => self.settings.still_secs,
            (Some(v), _) => v.duration,
            (None, Some(a)) => a.duration,
            (None, None) => 0.0,
        };
        self.frames_in(secs)
    }

    /// The source frames clips of `media` may use: [`media_frames`]
    /// (Self::media_frames), or for a still up to 17 h 40 min (p. 1194).
    pub fn media_limit(&self, media: &Media) -> i64 {
        if media.is_still() {
            self.frames_in(STILL_LIMIT_SECS)
        } else {
            self.media_frames(media)
        }
    }

    /// Whole frames in `secs` at the project rate (95% of a frame counts).
    fn frames_in(&self, secs: f64) -> i64 {
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
    /// An empty track.
    pub fn new(id: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            id: id.into(),
            kind,
            clips: Vec::new(),
            locked: false,
            muted: false,
            solo: false,
            volume_db: 0.0,
            video_enabled: true,
            name: None,
        }
    }

    /// Whether the track's audio is heard, given whether any track is soloed.
    pub fn audible(&self, any_solo: bool) -> bool {
        !self.muted && (self.solo || !any_solo)
    }

    /// Index of the clip covering frame `t`, by binary search.
    pub fn clip_covering(&self, t: i64) -> Option<usize> {
        let i = self.clips.partition_point(|c| c.start <= t);
        (i > 0 && t < self.clips[i - 1].end()).then(|| i - 1)
    }

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
