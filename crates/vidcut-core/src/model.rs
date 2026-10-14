//! The project model: settings, media pool, tracks and clips.
//!
//! The track model (spec §6): numbered tracks `V1…Vn`
//! hold combined video+audio clips, `A1…An` hold audio-only clips, and V1 is
//! magnetic while Ripple On is set. All positions and lengths are integer
//! frames at the project frame rate; `source_in` counts project frames from the
//! media's first video frame.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::{Rational, TimeContext, TimeSpec, format_timecode};

/// How long a still may last in the timeline, seconds: 17 h 40 min.
pub const STILL_LIMIT_SECS: f64 = 63_600.0;

/// A whole project: one timeline (deviation X2) plus its media pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    /// Increases with every change, including undo/redo. Mutations may pass
    /// `if_rev` to fail instead of clobbering a concurrent edit.
    pub rev: u64,
    /// When the project last changed (an edit, undo or redo), in
    /// milliseconds since 1970 UTC; `None` before its first change. For
    /// showing people when it was edited: scripts compare `rev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited: Option<u64>,
    pub settings: Settings,
    /// The media pool, in pool order.
    pub media: Vec<Media>,
    /// Video tracks `V1…` first, then audio tracks `A1…`.
    pub tracks: Vec<Track>,
    /// Timeline In/Out marks.
    pub marks: Marks,
    /// Source Tape In/Out (decision D17). Empty until the tape is marked.
    /// Frames are tape frames of `order`; a span on one clip is also that
    /// clip's source marks.
    #[serde(default, skip_serializing_if = "TapeMarks::is_empty")]
    pub tape_marks: TapeMarks,
    pub markers: Vec<Marker>,
    pub next_id: NextIds,
    /// Media Pool bins, as `/`-separated paths below the top level
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
    /// Track 1 ripples on insert, delete, move and trim (Ripple On).
    pub ripple_on: bool,
    /// Record timecode of timeline frame 0, in frames (deviation X7: 0, not 01:00:00:00).
    pub start_tc: i64,
    /// Record timecode counts drop-frame (decision D47): at 29.97 and 59.94,
    /// and nothing at other rates.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drop_frame: bool,
    /// Format not chosen yet: the first video media added sets fps and size.
    pub auto_format: bool,
    /// How long a still lasts when edited without a range or marks, and
    /// what the viewer shows of it, in seconds (decision D38).
    pub still_secs: f64,
    /// The preview plays a fresh proxy instead of the original picture
    /// (decision D41). Export, frames, analysis and OTIO ignore this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_proxies: bool,
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
            drop_frame: false,
            auto_format: true,
            still_secs: 5.0,
            use_proxies: false,
        }
    }
}

impl Settings {
    /// A new project's settings with the frame rate (`25`, `29.97`,
    /// `30000/1001`, any positive rate) and the picture size given. Either
    /// one fixes the format: the first video added then sets neither
    /// (decision D37).
    pub fn with_format(fps: Option<&str>, size: Option<(u32, u32)>) -> crate::error::Result<Self> {
        let mut s = Self::default();
        if let Some(f) = fps {
            s.fps = f
                .parse()
                .map_err(|e: crate::time::TimeError| crate::error::Error::usage(e.to_string()))?;
            s.auto_format = false;
        }
        if let Some((w, h)) = size {
            check_side("width", w)?;
            check_side("height", h)?;
            (s.width, s.height) = (w, h);
            s.auto_format = false;
        }
        Ok(s)
    }

    /// Counts record timecode drop-frame, or not (decision D47): only at
    /// 29.97 and 59.94. The start keeps its label.
    pub fn set_drop_frame(&mut self, on: bool) -> crate::error::Result<()> {
        if on && !self.fps.has_drop_frame() {
            return Err(crate::error::Error::invalid(format!(
                "drop-frame timecode is for 29.97 and 59.94 fps, not {}",
                self.fps.decimal()
            )));
        }
        self.relabel(|s| s.drop_frame = on);
        Ok(())
    }

    /// Changes the frame rate; drop-frame counting stays only where the
    /// new rate has it. The start keeps its label.
    pub fn set_fps(&mut self, fps: Rational) {
        self.relabel(|s| {
            s.fps = fps;
            s.drop_frame &= fps.has_drop_frame();
        });
    }

    /// Makes `change` keeping the start timecode's label (deviation X7):
    /// `01:00:00:00` stays so, another frame count at another rate or
    /// counting. A label the new rate hasn't (frame 29 at 25 fps) keeps its
    /// time instead.
    fn relabel(&mut self, change: impl FnOnce(&mut Self)) {
        let (label, secs) = (self.timecode(0), self.fps.frames_to_secs(self.start_tc));
        change(self);
        // Read as the project counts now: `;` would count drop-frame still.
        self.start_tc = TimeSpec::parse(&label.replace(';', ":"))
            .ok()
            .and_then(|t| self.start_context().resolve(&t).ok())
            .unwrap_or_else(|| self.fps.secs_to_frames(secs));
    }

    /// How a start timecode is read: as the project counts timecode.
    fn start_context(&self) -> TimeContext {
        TimeContext {
            fps: self.fps,
            start_tc: 0,
            drop_frame: self.counts_drop_frame(),
            end: 0,
            mark_in: None,
            mark_out: None,
            playhead: None,
        }
    }

    /// Sets the record timecode of the first frame: `01:00:00:00`, or
    /// drop-frame `01:00:00;00` (deviation X7, decision D47).
    pub fn set_start_tc(&mut self, tc: &TimeSpec) -> crate::error::Result<()> {
        let frames = self.start_context().resolve(tc)?;
        if frames < 0 {
            return Err(crate::error::Error::invalid(
                "a start timecode is 00:00:00:00 or later",
            ));
        }
        self.start_tc = frames;
        Ok(())
    }

    /// Whether timecodes count drop-frame: set, at a rate that has it.
    pub fn counts_drop_frame(&self) -> bool {
        self.drop_frame && self.fps.has_drop_frame()
    }

    /// Timeline frame `frame` as record timecode (drop-frame when counted so).
    pub fn timecode(&self, frame: i64) -> String {
        format_timecode(frame, self.fps, self.start_tc, self.counts_drop_frame())
    }

    /// A length, or a source position, as timecode counted from zero.
    pub fn length_timecode(&self, frames: i64) -> String {
        format_timecode(frames, self.fps, 0, self.counts_drop_frame())
    }
}

/// A picture side the encoders take: even, at least 16 pixels.
pub fn check_side(name: &str, px: u32) -> crate::error::Result<()> {
    if px < 16 || !px.is_multiple_of(2) {
        return Err(crate::error::Error::invalid(format!(
            "{name} must be even and at least 16"
        )));
    }
    Ok(())
}

/// Default longest side of a generated proxy, in pixels (decision D41).
pub const PROXY_LONGEST: u32 = 960;

/// Even picture size that fits inside a square of side `longest` without
/// getting larger. `width` and `height` are the size as shown.
pub fn proxy_size(width: u32, height: u32, longest: u32) -> (u32, u32) {
    let longest = longest.max(2) & !1;
    let (w, h) = (width.max(2) & !1, height.max(2) & !1);
    if w.max(h) <= longest {
        return (w, h);
    }
    let scale = f64::from(longest) / f64::from(w.max(h));
    let even = |v: f64| (v.round() as u32).max(2) & !1;
    (even(f64::from(w) * scale), even(f64::from(h) * scale))
}

/// A file in the media pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Media {
    /// `m<N>`, never reused.
    pub id: String,
    /// Absolute. The project file stores it relative to the project's
    /// folder when the file is in or under it.
    pub path: PathBuf,
    pub name: String,
    /// File size and modification time (Unix ns) when probed; for an image
    /// sequence, the total size of its pictures and the newest of their
    /// times. Used for cache keys and to notice changed or missing files.
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
    /// A smaller copy the preview can play. Absent until one is generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Proxy>,
    /// Its Clip Color (spec §3), which its clips show unless they have
    /// their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// SHA-256 of the file when it was added or relinked
    /// ([`fingerprint`](crate::fingerprint)): of the whole file up to
    /// 10 MiB (its `sha256sum`), else of 1 MiB from its start, middle and
    /// end; for an image sequence, of its first, middle and last pictures'.
    /// Relinking checks the files it finds against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// A playback proxy of a pool item: an easy-to-play file whose frames carry
/// the source's presentation timestamps (decision D41, deviation X18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Proxy {
    /// The proxy file, absolute; stored like [`Media::path`].
    pub path: PathBuf,
    /// [`Media::size`] when the proxy was made.
    pub source_size: u64,
    /// [`Media::mtime_ns`] when the proxy was made. A different size or
    /// time means the source changed and the proxy is stale.
    pub source_mtime_ns: i64,
    pub width: u32,
    pub height: u32,
    /// Encoder name, e.g. `h264`.
    pub codec: String,
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

    /// The proxy, when one is stored and it was made from this file as
    /// probed (same size and modification time).
    pub fn fresh_proxy(&self) -> Option<&Proxy> {
        self.proxy
            .as_ref()
            .filter(|p| p.source_size == self.size && p.source_mtime_ns == self.mtime_ns)
    }
}

/// Numbered image files making one clip: the pictures of a render
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

    /// The pool's name for it: `frame_[000-119].png`.
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
    /// Where the file's own clock starts: a camera's start timecode, a sound
    /// recorder's time of day. Timeline interchange counts source positions
    /// from it (deviation X9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timecode: Option<StartTimecode>,
}

/// A media file's start timecode: `count` units at `rate` from midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartTimecode {
    /// Video frames (a recorded timecode, drop-frame counted out), or audio
    /// samples (a broadcast WAV's time reference).
    pub count: i64,
    /// The video's frame rate, or the sample rate.
    pub rate: Rational,
    /// The file counts its timecode drop-frame (`01:00:00;00`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drop_frame: bool,
}

impl StartTimecode {
    /// The start as a (possibly fractional) frame count at `fps`.
    pub fn frames_at(&self, fps: Rational) -> f64 {
        if self.rate == fps {
            return self.count as f64;
        }
        let n = i128::from(self.count) * i128::from(self.rate.den()) * i128::from(fps.num());
        let d = i128::from(self.rate.num()) * i128::from(fps.den());
        n as f64 / d as f64
    }
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
    /// The display the pictures were mastered on, as the file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mastering: Option<MasteringDisplay>,
    /// Content light levels, as the file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<LightLevel>,
    /// HDR10+ dynamic metadata (SMPTE ST 2094-40) on the frames.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hdr10plus: bool,
    /// Dolby Vision on the frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dolby_vision: Option<DolbyVision>,
}

/// The display HDR pictures were mastered on (SMPTE ST 2086): its color
/// volume.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MasteringDisplay {
    /// Red, green and blue primaries as CIE 1931 `[x, y]`.
    pub primaries: [[f64; 2]; 3],
    /// White point as CIE 1931 `[x, y]`.
    pub white: [f64; 2],
    /// Luminance range, cd/m².
    pub min_nits: f64,
    pub max_nits: f64,
}

/// Content light levels (CTA-861.3), cd/m²: the brightest pixel of all
/// frames, and the brightest frame on average.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LightLevel {
    pub max_cll: u32,
    pub max_fall: u32,
}

/// Dolby Vision in a stream: its profile (5, 8, 10, …) and the base layer's
/// signal compatibility (8.1: HDR10, 8.4: HLG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DolbyVision {
    pub profile: u8,
    pub compatibility: u8,
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
    /// While any track is soloed, only soloed tracks are heard (the mixer).
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
    /// Transitions on its edit points, carried from another editor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A transition another editor put on an edit point, which vidcut keeps
/// while it fits there, shown as a cut (deviation X9). `otio::transitions`
/// places and writes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Transition {
    /// Between the clips' sounds (an audio edit), not their pictures.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sound: bool,
    /// The clip before the cut; none for a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The clip after the cut; none for a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Frames it covers before the cut (OTIO's `in_offset`) and after it
    /// (`out_offset`).
    pub before: i64,
    pub after: i64,
    /// The OTIO `Transition` as read.
    pub otio: serde_json::Value,
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
    /// Disabled clips are kept but not shown (the D key).
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Effects on its picture, carried from another editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Effects>,
    /// A generator or title another editor made, carried (deviation X9):
    /// a block that plays as a gap. It has no media (`media` is empty) and
    /// no audio, and any length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<Generator>,
    /// A compound clip or nested timeline another editor made, carried
    /// (deviation X9): a block that plays as a gap, with no media and no
    /// audio. On an audio track it stands for the nested timeline's sound
    /// (`video` false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compound: Option<Compound>,
    /// A speed change another editor made, carried (deviation X9): the clip
    /// plays and edits at it (decision D48).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<Speed>,
    /// Its own Clip Color (spec §3). Without one it shows its media's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A speed change from another editor's file (deviation X9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Speed {
    /// The OTIO time effect as read: a `LinearTimeWarp` (`time_scalar` 2.0
    /// for twice the speed, -1.0 reversed) or a `FreezeFrame`.
    pub effect: serde_json::Value,
    /// Frames of its pace before its first frame (decision D48): the parts
    /// a split or trim leaves show the frames they showed.
    #[serde(default, skip_serializing_if = "is_zero_frames")]
    pub skip: i64,
}

fn is_zero_frames(n: &i64) -> bool {
    *n == 0
}

impl Speed {
    /// How fast: 2.0 twice, -1.0 reversed, 0 frozen; none for a time effect
    /// without a scalar.
    pub fn scalar(&self) -> Option<f64> {
        self.effect["time_scalar"].as_f64()
    }

    /// As shown: `200%`, `-100%`, `freeze`, or the effect's name.
    pub fn label(&self) -> String {
        match self.scalar() {
            Some(0.0) => "freeze".to_string(),
            Some(x) => format!("{}%", (x * 100.0).round()),
            None => self.effect["OTIO_SCHEMA"]
                .as_str()
                .and_then(|s| s.split('.').next())
                .unwrap_or("speed")
                .to_string(),
        }
    }
}

/// A generator or title from another editor's file (deviation X9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Generator {
    /// The OTIO media reference as read: a `GeneratorReference`.
    pub reference: serde_json::Value,
}

/// A compound clip or nested timeline from another editor's file
/// (deviation X9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Compound {
    /// The nested OTIO `Stack` as read, without its source range and clip
    /// effects (the clip's own).
    pub stack: serde_json::Value,
    /// Its length in project frames: the clip's source range stays within.
    pub frames: i64,
    /// The clips it holds as vidcut plays them, on tracks of their own,
    /// frames from its first: its stack's video tracks for a picture block,
    /// audio tracks for a sound block. None when their media weren't found:
    /// it plays as a gap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
}

/// Clip effects another editor applied, which vidcut keeps without showing
/// or playing them (deviation X9): the OTIO effects of a clip's picture or
/// of its sound. `otio::effects` reads, cuts and writes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Effects {
    /// The source frame their keyframe times count from: the first of the
    /// picture or sound when they were read. Keyframes stay on their source
    /// frames through trims, slips and splits.
    pub from: i64,
    /// OTIO `Effect`s as read, keyframe times and fade lengths in project
    /// frames.
    pub list: Vec<serde_json::Value>,
}

/// A clip with the track it is on: what `edit.copy` returns and `edit.paste`
/// takes (paste puts clips back on their own tracks).
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
    /// Effects on its sound, carried from another editor. The clip volume
    /// is `gain_db`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Effects>,
}

impl Clip {
    /// Whether it is a generator or title from another editor, without
    /// media.
    pub fn is_generator(&self) -> bool {
        self.generator.is_some()
    }

    /// Whether it is a block from another editor without media: a
    /// generator or a compound clip.
    pub fn is_block(&self) -> bool {
        self.generator.is_some() || self.compound.is_some()
    }

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

    /// How it walks its source (decision D48): its speed, from where in its
    /// pace it starts.
    pub fn pace(&self) -> crate::render::Pace {
        crate::render::Pace::of(self.speed(), self.speed.as_ref().map_or(0, |s| s.skip))
    }

    /// Source frame shown at timeline frame `t`: at its speed (decision
    /// D48), the one its stretch of the source starts in.
    pub fn source_at(&self, t: i64) -> i64 {
        self.source_in + self.pace().offset(t - self.start)
    }

    /// Where its sound is in its source at timeline frame `t` (frames,
    /// between frames too), at its speed (decision D48).
    pub fn sound_at(&self, t: f64) -> f64 {
        self.source_in as f64 + self.pace().sound(t - self.start as f64)
    }

    /// Its content moves on `k` frames of its pace (back when negative):
    /// each frame shows what the one `k` later showed. A slip; with its
    /// start and length (`move_head`), a head trim, a split's second part,
    /// a roll's or slide's incoming clip.
    pub fn walk(&mut self, k: i64) {
        self.source_in += self.pace().offset(k);
        if let Some(s) = self.speed.as_mut() {
            s.skip += k;
        }
    }

    /// Its head moves `k` frames later (earlier when negative), every frame
    /// after it showing what it showed.
    pub fn move_head(&mut self, k: i64) {
        self.start += k;
        self.duration -= k;
        self.walk(k);
    }

    /// The source frames its picture shows: the lowest, and one past the
    /// highest.
    pub fn shown(&self) -> (i64, i64) {
        let (a, b) = (self.source_at(self.start), self.source_at(self.end() - 1));
        (a.min(b), a.max(b) + 1)
    }

    /// The source its sound plays (frames, low and high): none without
    /// sound, or held.
    pub fn sound_span(&self) -> Option<(f64, f64)> {
        let (a, b) = self.audio_range()?;
        if self.speed() == 0.0 {
            return None;
        }
        let (x, y) = (self.sound_at(a as f64), self.sound_at(b as f64));
        Some((x.min(y), x.max(y)))
    }

    /// Frames its head can move earlier, or its tail later (`tail`), with
    /// its picture inside source frames `0..frames`; unbounded held.
    pub fn room(&self, tail: bool, frames: i64) -> i64 {
        let speed = self.speed();
        if speed == 0.0 {
            return i64::MAX / 4;
        }
        let at = |n: i64| {
            let t = if tail {
                self.end() - 1 + n
            } else {
                self.start - n
            };
            (0..frames).contains(&self.source_at(t))
        };
        // The last that fits, found between two that bracket it.
        let far = ((frames as f64 + 2.0) / speed.abs()).min((i64::MAX / 4) as f64);
        let (mut lo, mut hi) = (0, far as i64 + 2);
        if !at(lo) {
            return 0;
        }
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if at(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Where timeline frame `t` falls in its content: source frames at
    /// normal speed, else frames of its pace (decision D48), which the
    /// parts a split or trim leaves agree on.
    pub fn content_at(&self, t: i64) -> i64 {
        match &self.speed {
            None => self.source_in + t - self.start,
            Some(s) => s.skip + t - self.start,
        }
    }

    /// Whether `other` walks the same source as it does, the same way, and
    /// in step: a part of it after splits and trims.
    pub fn in_step(&self, other: &Clip) -> bool {
        match (&self.speed, &other.speed) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.effect == b.effect
                    && other.source_in == self.source_in + self.pace().offset(b.skip - a.skip)
            }
            _ => false,
        }
    }

    /// Source frames per timeline frame: its speed change's (2.0 twice as
    /// fast, -1.0 backward, 0 a held frame), else 1.
    pub fn speed(&self) -> f64 {
        self.speed.as_ref().and_then(Speed::scalar).unwrap_or(1.0)
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

/// Source Tape In/Out (decision D17). `in` and `out` are tape frames of
/// `order` (the media ids in tape order when the marks were set). The range
/// is half-open, so `out` is the first frame after it (D16).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TapeMarks {
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub mark_in: Option<i64>,
    #[serde(rename = "out", default, skip_serializing_if = "Option::is_none")]
    pub mark_out: Option<i64>,
    /// Media ids in the tape order these frames refer to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
}

impl TapeMarks {
    pub fn is_empty(&self) -> bool {
        self.mark_in.is_none() && self.mark_out.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Marker {
    /// `k<N>`, never reused.
    pub id: String,
    pub at: i64,
    /// Length in frames: one, unless set or imported (spec D40). The
    /// timeline shows a marker as a point either way.
    #[serde(default = "one_frame", skip_serializing_if = "is_one_frame")]
    pub duration: i64,
    pub name: String,
    pub color: String,
    pub note: String,
}

fn one_frame() -> i64 {
    1
}

fn is_one_frame(frames: &i64) -> bool {
    *frames == 1
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
            rev: 0,
            edited: None,
            settings,
            media: Vec::new(),
            tracks: vec![Track::new("V1", TrackKind::Video)],
            marks: Marks::default(),
            tape_marks: TapeMarks::default(),
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

    /// The Clip Color clip `c` shows: its own, else its media's.
    pub fn clip_color<'a>(&'a self, c: &'a Clip) -> Option<&'a str> {
        c.color
            .as_deref()
            .or_else(|| self.media(&c.media).and_then(|m| m.color.as_deref()))
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
    /// project's still duration here, though a clip may make it
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
    /// (Self::media_frames), or for a still up to 17 h 40 min.
    pub fn media_limit(&self, media: &Media) -> i64 {
        if media.is_still() {
            self.frames_in(STILL_LIMIT_SECS)
        } else {
            self.media_frames(media)
        }
    }

    /// The source frames clip `c` may use: its media's, a compound's
    /// length; none for a generator, which has any length, or a clip whose
    /// media is gone.
    pub fn source_frames(&self, c: &Clip) -> Option<i64> {
        match (&c.compound, &c.generator) {
            (Some(k), _) => Some(k.frames),
            (None, Some(_)) => None,
            (None, None) => self.media(&c.media).map(|m| self.media_limit(m)),
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
            drop_frame: self.settings.counts_drop_frame(),
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
            transitions: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::Settings;
    use crate::time::TimeSpec;

    #[test]
    fn turning_drop_frame_off_keeps_the_start_label() {
        let mut s = Settings::with_format(Some("29.97"), None).unwrap();
        s.set_drop_frame(true).unwrap();
        s.set_start_tc(&TimeSpec::parse("01:00:00;00").unwrap())
            .unwrap();
        assert_eq!(s.start_tc, 107_892);
        s.set_drop_frame(false).unwrap();
        assert_eq!(s.start_tc, 108_000);
    }
}
