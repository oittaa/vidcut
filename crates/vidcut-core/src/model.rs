use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::time::{Rational, TimeContext, TimeSpec, format_timecode, label_frames};

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
    /// Subtitle tracks `S1…`, from the one nearest the video up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<SubtitleTrack>,
    pub next_id: NextIds,
    /// Media Pool bins, as `/`-separated paths below the top level
    /// (`Interviews`, `Interviews/Day 1`), sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bins: Vec<String>,
    /// Who speaks: names, the files' speakers joined as one person, and
    /// words given to another (`speakers.*`).
    #[serde(default, skip_serializing_if = "Speakers::is_empty")]
    pub speakers: Speakers,
}

/// Who speaks in the transcripts the project uses. A transcript numbers
/// its speakers; a speaker of a paid run over several files has one number
/// in all of them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Speakers {
    /// A person's name, by person (`speakers.list`). Unnamed, a person is
    /// `Speaker N`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
    /// The person a file's speaker is, by voice (`m3/0/2`: the media, its
    /// sound stream and the speaker's number), where it isn't the person its
    /// transcript gives.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, String>,
    /// Words given to another person, the last one first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assigned: Vec<Assigned>,
    /// A person's color, by person (`speakers.set`), which their cues are
    /// painted with as their subtitle track's `speaker_color` says.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
}

/// A file's words given to a person: those whose middle is from `start` to
/// `end`, in seconds of the file's sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Assigned {
    pub media: String,
    pub stream: u32,
    pub start: f64,
    pub end: f64,
    pub person: String,
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
    /// Filler words this project marks, per language code, each list in
    /// place of the preferences' or the built-in one (`project.set`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fillers: BTreeMap<String, Vec<String>>,
    /// The timeline color space (decision D50); left out when `auto`.
    #[serde(default, skip_serializing_if = "TimelineColors::is_auto")]
    pub timeline_colors: TimelineColors,
    /// The Retime Process of the clips that follow the project; left out
    /// when `auto`.
    #[serde(default, skip_serializing_if = "ProjectRetime::is_auto")]
    pub retime: ProjectRetime,
    /// The upscaler of Super Scale for the media that follow the project;
    /// left out when `auto`.
    #[serde(default, skip_serializing_if = "ProjectUpscaler::is_auto")]
    pub upscaler: ProjectUpscaler,
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
            fillers: BTreeMap::new(),
            timeline_colors: TimelineColors::Auto,
            retime: ProjectRetime::Auto,
            upscaler: ProjectUpscaler::Auto,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Format {
    pub fps: Option<String>,
    pub size: Option<(u32, u32)>,
    pub drop_frame: Option<bool>,
    pub start_tc: Option<TimeSpec>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

impl Format {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn settings(&self) -> crate::error::Result<Settings> {
        let mut s = Settings::with_format(self.fps.as_deref(), self.size)?;
        if let Some(d) = self.drop_frame {
            s.set_drop_frame(d)?;
        }
        if let Some(tc) = &self.start_tc {
            s.set_start_tc(tc)?;
        }
        if let Some(r) = self.sample_rate {
            s.sample_rate = r;
        }
        if let Some(c) = self.channels {
            s.channels = c;
        }
        Ok(s)
    }
}

impl Settings {
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

    pub fn set_fps(&mut self, fps: Rational) {
        self.relabel(|s| {
            s.fps = fps;
            s.drop_frame &= fps.has_drop_frame();
        });
    }

    fn relabel(&mut self, change: impl FnOnce(&mut Self)) {
        let (label, secs) = (self.timecode(0), self.fps.frames_to_secs(self.start_tc));
        change(self);
        self.start_tc = TimeSpec::parse(&label.replace(';', ":"))
            .ok()
            .and_then(|t| self.start_context().resolve(&t).ok())
            .unwrap_or_else(|| self.fps.secs_to_frames(secs));
    }

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

    pub fn counts_drop_frame(&self) -> bool {
        self.drop_frame && self.fps.has_drop_frame()
    }

    pub fn timecode(&self, frame: i64) -> String {
        format_timecode(frame, self.fps, self.start_tc, self.counts_drop_frame())
    }

    pub fn length_timecode(&self, frames: i64) -> String {
        format_timecode(frames, self.fps, 0, self.counts_drop_frame())
    }
}

pub fn check_side(name: &str, px: u32) -> crate::error::Result<()> {
    if px < 16 || !px.is_multiple_of(2) {
        return Err(crate::error::Error::invalid(format!(
            "{name} must be even and at least 16"
        )));
    }
    Ok(())
}

pub const PROXY_LONGEST: u32 = 960;

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
    /// How its pictures' colors are read: as the file's tags say, or as this
    /// says in their place, for a file whose tags are wrong.
    #[serde(default, skip_serializing_if = "ColorSpace::is_tagged")]
    pub color_space: ColorSpace,
    /// Super Scale: its pictures are enlarged with an AI upscaler or the
    /// classic one, as much as they need, before they are fitted. Absent
    /// while it is off.
    #[serde(default, skip_serializing_if = "MediaSuperScale::is_off")]
    pub super_scale: MediaSuperScale,
    /// Source frames left out of every edit of this media (`media.leave_out`,
    /// Delete in the Transcription window), each `[start, end)`, in order and
    /// apart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<(i64, i64)>,
    /// The words this project takes from the transcripts of each sound
    /// stream: which transcript, and the words changed in it
    /// (`media.transcripts.main`, `media.words.set`). Left out while the
    /// newest transcript is taken as it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<ChosenWords>,
    /// A subtitle file (SRT, WebVTT, TTML): what was read from it when it
    /// was added or relinked. It has no pictures or sound; its cues are read
    /// again when it is placed on a subtitle track (`subtitles.import` with
    /// `media`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitles: Option<SubtitleFile>,
}

/// The words a project takes from the transcripts of one sound stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChosenWords {
    pub stream: u32,
    /// The kept transcript that the words come from (a key from
    /// `media.transcripts.list`). Left out to take the newest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    /// Stretches whose words come from somewhere else than the main
    /// transcript, in order and apart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<WordsChange>,
}

/// Words that take the place of the main transcript's words from `start` to
/// `end`: each word whose middle is in that time goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WordsChange {
    /// Seconds of the file's sound.
    pub start: f64,
    pub end: f64,
    /// The words put in their place, in seconds of the file's sound.
    pub words: Vec<ChangedWord>,
    /// The kept transcript the words were taken from. Left out for words
    /// that were typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// One word of a [`WordsChange`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangedWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// It follows the word before it without a space.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub joined: bool,
}

/// A subtitle file in the Media Pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubtitleFile {
    pub format: crate::request::SubtitleFormat,
    /// The text encoding it was read in, such as UTF-8 or Windows-1252.
    pub encoding: String,
    /// How many cues it holds.
    pub cues: usize,
    /// The language it names, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Its first cue's text, without styling.
    pub first: String,
    /// What reading it found wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

pub fn uncovered(a: i64, b: i64, spans: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut spans = spans.to_vec();
    spans.sort_unstable();
    let mut out = Vec::new();
    let mut at = a;
    for (s, e) in spans {
        if e <= at || s >= b {
            continue;
        }
        if s > at {
            out.push((at, s));
        }
        at = e;
        if at >= b {
            break;
        }
    }
    if at < b {
        out.push((at, b));
    }
    out
}

/// Super Scale of a media item: on or off, and its upscaler.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MediaSuperScale {
    pub on: bool,
    pub sharpness: Sharpness,
    pub upscaler: MediaUpscaler,
}

impl MediaSuperScale {
    pub fn is_off(&self) -> bool {
        !self.on
    }
}

/// How Super Scale enlarges a picture, with the upscaler that runs on this
/// computer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SuperScale {
    pub on: bool,
    pub sharpness: Sharpness,
    pub upscaler: Upscaler,
}

impl SuperScale {
    pub fn is_off(&self) -> bool {
        !self.on
    }

    pub fn ai(&self) -> bool {
        self.on && self.upscaler != Upscaler::Classic
    }
}

/// The upscaler of Super Scale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Upscaler {
    /// The super-resolution scaler of macOS 26 and later, for pictures up
    /// to 1920x1080. It enlarges by a factor that macOS offers, and the
    /// result is fitted. It reads the frames of a clip in order from its
    /// first frame. macOS downloads its models only with `download`. The
    /// upscaler of Auto on macOS, where the app calls it AI.
    #[cfg_attr(target_os = "macos", default)]
    Macos,
    /// A diffusion network (FlashVSR+ Turbo) on the GPU, for SDR pictures:
    /// slow, with more detail. It reads the frames of a clip in order from
    /// its first frame. Its weights (7.0 GB) download at their first use. An
    /// export refuses media that it cannot take. Off on macOS for now.
    AiBest,
    /// A neural network (NanoVSR 5.4M) on the GPU, for SDR and HDR pictures.
    /// It reads 15 frames at a time. Its weights (25 MB) download at their
    /// first use. The upscaler of Auto elsewhere. Off on macOS for now.
    #[cfg_attr(not(target_os = "macos"), default)]
    AiFast,
    /// The classic upscalers, on the GPU.
    Classic,
}

/// The upscaler of a media item's Super Scale: the project's, or one of
/// its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaUpscaler {
    /// The upscaler of the project settings.
    #[default]
    Project,
    /// The super-resolution scaler of macOS: on macOS only.
    Macos,
    /// FlashVSR+ Turbo on the GPU: off on macOS for now.
    AiBest,
    /// NanoVSR 5.4M on the GPU: off on macOS for now.
    AiFast,
    /// The classic upscalers.
    Classic,
}

impl MediaUpscaler {
    pub fn upscaler(self) -> Option<Upscaler> {
        match self {
            Self::Project => None,
            Self::Macos => Some(Upscaler::Macos),
            Self::AiBest => Some(Upscaler::AiBest),
            Self::AiFast => Some(Upscaler::AiFast),
            Self::Classic => Some(Upscaler::Classic),
        }
    }
}

/// The project's upscaler of Super Scale, for the media that follow it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectUpscaler {
    /// The AI upscaler of the computer that plays or exports: the scaler of
    /// macOS on macOS, AI Fast elsewhere.
    #[default]
    Auto,
    /// The super-resolution scaler of macOS: on macOS only.
    Macos,
    /// FlashVSR+ Turbo on the GPU: off on macOS for now.
    AiBest,
    /// NanoVSR 5.4M on the GPU: off on macOS for now.
    AiFast,
    /// The classic upscalers.
    Classic,
}

impl ProjectUpscaler {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }

    pub fn upscaler(self) -> Upscaler {
        match self {
            Self::Auto => Upscaler::default(),
            Self::Macos => Upscaler::Macos,
            Self::AiBest => Upscaler::AiBest,
            Self::AiFast => Upscaler::AiFast,
            Self::Classic => Upscaler::Classic,
        }
    }
}

/// How much detail Super Scale gives the enlarged pictures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sharpness {
    Low,
    #[default]
    Medium,
    High,
}

/// How a media item's pictures' colors are read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    /// As the file's color tags say.
    #[default]
    Tagged,
    /// SDR in BT.709, as HD video is.
    Bt709,
    /// SDR in BT.601, as SD video is.
    Bt601,
    /// HDR in BT.2100 PQ, as HDR10 is.
    Pq,
    /// HDR in BT.2100 HLG.
    Hlg,
}

impl ColorSpace {
    pub fn is_tagged(&self) -> bool {
        *self == Self::Tagged
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Tagged => "tagged",
            Self::Bt709 => "bt709",
            Self::Bt601 => "bt601",
            Self::Pq => "pq",
            Self::Hlg => "hlg",
        }
    }

    pub fn hdr(self) -> Option<Hdr> {
        match self {
            Self::Pq => Some(Hdr::Pq),
            Self::Hlg => Some(Hdr::Hlg),
            _ => None,
        }
    }
}

/// The timeline color space. Every dissolve, wipe, placed picture, title
/// and subtitle mixes in it, and each export converts the mixed frame to
/// its own colors (decision D50).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimelineColors {
    /// HDR when more than half of the timeline's pictures are HDR: HLG when
    /// every HDR picture is HLG, else PQ. Otherwise SDR.
    #[default]
    Auto,
    /// SDR in BT.709: HDR pictures are tone-mapped.
    Sdr,
    /// HDR in BT.2100 PQ.
    Pq,
    /// HDR in BT.2100 HLG.
    Hlg,
}

impl TimelineColors {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

/// A playback proxy of a pool item: an easy-to-play file whose frames carry
/// the source's presentation timestamps (decision D41, deviation X18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Proxy {
    /// The proxy file's name. Each computer looks for it in its own proxy
    /// folder (preferences `proxies.folder`), so the project names no
    /// computer's folder.
    pub file: String,
    /// [`Media::size`] when the proxy was made.
    pub source_size: u64,
    /// [`Media::mtime_ns`] when the proxy was made. A different size or
    /// time means the source changed and the proxy is stale.
    pub source_mtime_ns: i64,
    pub width: u32,
    pub height: u32,
    /// Encoder name, e.g. `h264`.
    pub codec: String,
    /// [`Media::color_space`] when the proxy was made. A different one means
    /// the proxy is stale.
    #[serde(default, skip_serializing_if = "ColorSpace::is_tagged")]
    pub color_space: ColorSpace,
}

impl Proxy {
    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(&self.file)
    }
}

impl Media {
    pub fn video_as_read(&self) -> Option<VideoInfo> {
        let mut v = self.info.video.clone()?;
        if !self.color_space.is_tagged() {
            v.hdr = self.color_space.hdr();
            v.mastering = None;
            v.light = None;
            v.hdr10plus = false;
            v.dolby_vision = None;
        }
        Some(v)
    }

    pub fn is_still(&self) -> bool {
        self.info.video.as_ref().is_some_and(|v| v.still)
    }

    pub fn sequence_file(&self, k: u64) -> Option<PathBuf> {
        let s = self.sequence.as_ref()?;
        Some(s.file(self.path.parent().unwrap_or(Path::new("")), k))
    }

    pub fn fresh_proxy(&self) -> Option<&Proxy> {
        self.proxy.as_ref().filter(|p| {
            p.source_size == self.size
                && p.source_mtime_ns == self.mtime_ns
                && p.color_space == self.color_space
        })
    }
}

pub const FRAME_PAD_MAX: usize = 32;

pub fn padded_frame(n: u64, width: usize) -> String {
    format!("{n:0width$}", width = width.min(FRAME_PAD_MAX))
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
            let width = rest[..digits]
                .parse::<usize>()
                .unwrap_or(0)
                .min(FRAME_PAD_MAX);
            out.push_str(&number(width));
            rest = &rest[(digits + 1).min(rest.len())..];
        }
        out.push_str(rest);
        out
    }

    pub fn parts(&self) -> (String, usize, String) {
        let mut width = None;
        let marked = self.fill(|w| {
            width = Some(w);
            "\u{0}".into()
        });
        let (prefix, suffix) = marked.split_once('\u{0}').unwrap_or((&marked, ""));
        (prefix.to_string(), width.unwrap_or(0), suffix.to_string())
    }

    pub fn name(&self, n: u64) -> String {
        self.fill(|w| padded_frame(n, w))
    }

    pub fn display(&self) -> String {
        let last = self.first + self.count.saturating_sub(1);
        self.fill(|w| {
            format!(
                "[{}-{}]",
                padded_frame(self.first, w),
                padded_frame(last, w)
            )
        })
    }

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
    pub fn label_seconds(&self) -> f64 {
        let frames = label_frames(self.count, self.rate, self.drop_frame);
        frames as f64 / self.rate.timecode_base() as f64
    }

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
    /// The pictures have an alpha channel: they show the tracks below
    /// where they are transparent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alpha: bool,
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

/// What a media file says about itself, read from it again: its container
/// and every stream, with the tags they carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MediaDetails {
    /// The container's long name, such as `QuickTime / MOV`.
    pub format: String,
    /// Bits per second of the whole file, as the container says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<u64>,
    /// The container's tags, such as `creation_time`, `make`, `model` and
    /// `encoder`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    pub streams: Vec<StreamDetails>,
}

/// One stream of a media file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StreamDetails {
    /// Its index in the file, all kinds counted.
    pub index: u32,
    /// `video`, `audio`, `subtitle`, `data` or `attachment`.
    pub kind: String,
    pub codec: String,
    /// The codec's long name, such as `H.264 / AVC / MPEG-4 AVC / MPEG-4
    /// part 10`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub codec_name: String,
    /// The codec's profile, such as `High` or `Main 10`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<u64>,
    /// Pixel format, bits per component and chroma subsampling (`4:2:0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pix_fmt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroma: Option<String>,
    /// Color tags as the stream states them: primaries, transfer, matrix
    /// and range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primaries: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    /// The channel layout, such as `stereo` or `5.1(side)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_format: Option<String>,
    /// Its language tag, such as `eng`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
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
    /// Voice Isolation on the track's clips that have none of their own,
    /// as a clip's `voice_isolation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_isolation: Option<f64>,
    /// Ducking: the track dips where its sources are active (decision
    /// D76).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duck: Option<Duck>,
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

/// A track's ducking (decision D76): its sound dips by the Duck Amount
/// where any of its sources is active, with the same gain on every
/// channel. A value left out has its usual value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Duck {
    /// It dips the track. Off keeps the settings.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub on: bool,
    /// The tracks whose sound dips it (`V1`, `A2`), never its own. A source
    /// counts only where it is heard: not muted, and soloed while any
    /// track is.
    #[serde(default)]
    pub sources: Vec<String>,
    /// The Duck Amount: how far it dips, in dB, from 0 to 60. 2.7 unless
    /// set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_db: Option<f64>,
    /// How long before a source starts the full dip is reached, in ms,
    /// from 0 to 1000. 15 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookahead_ms: Option<f64>,
    /// The Rise Time: how long the gain takes to fall to the full dip, in
    /// ms, from 1 to 1000. 10 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rise_ms: Option<f64>,
    /// How long the full dip stays after a source stops, in ms, from 0 to
    /// 5000. A pause shorter than this keeps the dip. 150 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_ms: Option<f64>,
    /// The Recovery Time: how long the gain takes to come back after the
    /// hold, in ms, from 1 to 1000. 750 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_ms: Option<f64>,
    /// What makes a source active. `speech` unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detect: Option<DuckDetect>,
    /// With `detect` `level`: a source is active where its peak, after its
    /// clip gain and fader, is at or above this many dBFS, from -96 to 0.
    /// -42 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_db: Option<f64>,
}

fn on() -> bool {
    true
}

fn is_on(v: &bool) -> bool {
    *v
}

/// What makes a ducking source active (deviation X69).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DuckDetect {
    /// Speech that the speech detector (Silero VAD, on the CPU) hears in
    /// it. Room noise, breaths and bumps don't dip the track.
    #[default]
    Speech,
    /// Sound at or above `threshold_db`.
    Level,
}

/// A transition another editor put on an edit point, which vidcut keeps
/// while it fits there, shown as a cut (deviation X9). `otio::transitions`
/// places and writes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Transition {
    /// `t<N>`, never reused.
    pub id: String,
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
    pub otio: Kept,
}

/// How the pictures of a transition meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PictureTransition {
    /// A cross dissolve.
    #[default]
    Dissolve,
    /// One wipe, from `direction`.
    Wipe { direction: WipeDirection },
}

impl PictureTransition {
    pub fn is_dissolve(&self) -> bool {
        matches!(self, Self::Dissolve)
    }
}

/// The edge a wipe starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WipeDirection {
    /// From the left edge toward the right.
    Left,
    /// From the right edge toward the left.
    Right,
    /// From the top edge toward the bottom.
    Up,
    /// From the bottom edge toward the top.
    Down,
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
    /// A title (decision D65): its text drawn over the picture of the
    /// tracks below it, on a video track. It has no media (`media` is
    /// empty) and no audio, and any length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Title>,
    /// Where its picture goes over the frame: moved, zoomed, turned,
    /// flipped and cropped. Left out, it fills the frame as it always has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<Picture>,
    /// A compound clip or nested timeline another editor made, carried
    /// (deviation X9): a block that plays as a gap, with no media and no
    /// audio. On an audio track it stands for the nested timeline's sound
    /// (`video` false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compound: Option<Compound>,
    /// Its speed, when not 100% forward: one speed, or sections between
    /// speed keyframes (decision D48). The clip plays and edits at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<Speed>,
    /// How it makes a frame that falls between two of its source frames:
    /// at a speed change, or where its media's frame rate isn't the
    /// timeline's. Left out at `project`, the Retime Process of the project.
    #[serde(default, skip_serializing_if = "ClipRetime::is_project")]
    pub retime: ClipRetime,
    /// Its own Clip Color (spec §3). Without one it shows its media's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Kept(Arc<serde_json::Value>);

impl Kept {
    pub fn make_mut(&mut self) -> &mut serde_json::Value {
        Arc::make_mut(&mut self.0)
    }

    pub fn into_value(self) -> serde_json::Value {
        Arc::try_unwrap(self.0).unwrap_or_else(|shared| (*shared).clone())
    }
}

impl std::ops::Deref for Kept {
    type Target = serde_json::Value;
    fn deref(&self) -> &serde_json::Value {
        &self.0
    }
}

impl From<serde_json::Value> for Kept {
    fn from(v: serde_json::Value) -> Self {
        Self(Arc::new(v))
    }
}

impl PartialEq for Kept {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Serialize for Kept {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Kept {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde_json::Value::deserialize(d).map(Self::from)
    }
}

impl JsonSchema for Kept {
    fn inline_schema() -> bool {
        serde_json::Value::inline_schema()
    }
    fn schema_name() -> Cow<'static, str> {
        serde_json::Value::schema_name()
    }
    fn schema_id() -> Cow<'static, str> {
        serde_json::Value::schema_id()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::Value::json_schema(generator)
    }
}

/// Retime Process: how a clip makes a frame that falls between two of its
/// source frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Retime {
    /// The source frame that the frame falls in.
    #[default]
    Nearest,
    /// The two source frames around it, mixed by how near each is.
    FrameBlend,
    /// A neural network (RIFE 4.25) on the GPU makes the frame from the
    /// motion between the two source frames. Its weights (25 MB) download at
    /// their first use.
    AiFast,
    /// A larger neural network (EMA-VFI) on the GPU, as AI Fast: slower, and
    /// nearer to the real frames. Its weights (264 MB) download at their
    /// first use.
    AiBest,
    /// The frame rate conversion of macOS (VideoToolbox) makes the frame
    /// from the motion between the two source frames. It is on macOS only,
    /// where the app calls it AI, and where AI Fast and AI Best are off.
    Macos,
}

impl Retime {
    pub const ALL: [Retime; 5] = [
        Retime::Nearest,
        Retime::FrameBlend,
        Retime::AiFast,
        Retime::AiBest,
        Retime::Macos,
    ];

    pub fn is_nearest(&self) -> bool {
        *self == Retime::Nearest
    }

    pub fn ai(&self) -> bool {
        matches!(self, Retime::AiFast | Retime::AiBest | Retime::Macos)
    }

    #[cfg(target_os = "macos")]
    pub fn ai_here() -> Self {
        Retime::Macos
    }

    #[cfg(not(target_os = "macos"))]
    pub fn ai_here() -> Self {
        Retime::AiFast
    }

    pub fn title(&self) -> &'static str {
        match self {
            Retime::Nearest => "Nearest",
            Retime::FrameBlend => "Frame Blend",
            Retime::AiFast => "AI Fast",
            Retime::AiBest => "AI Best",
            Retime::Macos => "AI",
        }
    }
}

/// A clip's Retime Process: the project's, or one of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipRetime {
    /// The Retime Process of the project settings.
    #[default]
    Project,
    /// The source frame that the frame falls in.
    Nearest,
    /// The two source frames around it, mixed by how near each is.
    FrameBlend,
    /// RIFE 4.25 on the GPU: off on macOS.
    AiFast,
    /// EMA-VFI on the GPU: off on macOS.
    AiBest,
    /// The frame rate conversion of macOS: on macOS only.
    Macos,
}

impl ClipRetime {
    pub fn is_project(&self) -> bool {
        *self == Self::Project
    }

    pub fn process(self) -> Option<Retime> {
        match self {
            Self::Project => None,
            Self::Nearest => Some(Retime::Nearest),
            Self::FrameBlend => Some(Retime::FrameBlend),
            Self::AiFast => Some(Retime::AiFast),
            Self::AiBest => Some(Retime::AiBest),
            Self::Macos => Some(Retime::Macos),
        }
    }

    pub fn title(&self) -> &'static str {
        self.process().map_or("Project Settings", |r| r.title())
    }
}

/// The project's Retime Process, for the clips that follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRetime {
    /// The AI of the computer that plays or exports for slow motion: the
    /// frame rate conversion of macOS on macOS, AI Fast elsewhere. Nearest
    /// for the rest: other speeds and other frame rates.
    #[default]
    Auto,
    /// The source frame that the frame falls in.
    Nearest,
    /// The two source frames around it, mixed by how near each is.
    FrameBlend,
    /// RIFE 4.25 on the GPU: off on macOS.
    AiFast,
    /// EMA-VFI on the GPU: off on macOS.
    AiBest,
    /// The frame rate conversion of macOS: on macOS only.
    Macos,
}

impl ProjectRetime {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }

    pub fn process(self) -> Option<Retime> {
        match self {
            Self::Auto => None,
            Self::Nearest => Some(Retime::Nearest),
            Self::FrameBlend => Some(Retime::FrameBlend),
            Self::AiFast => Some(Retime::AiFast),
            Self::AiBest => Some(Retime::AiBest),
            Self::Macos => Some(Retime::Macos),
        }
    }

    pub fn title(&self) -> &'static str {
        self.process().map_or("Auto", |r| r.title())
    }
}

/// A clip's speed (decision D48): sections that each play at one speed,
/// on the clip's retimed source. A speed keyframe starts each section
/// after the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Speed {
    /// The sections in order. A speed keyframe starts each one after the
    /// first.
    /// The first also covers the frames before it, and the last the frames
    /// after it.
    pub sections: Arc<[Section]>,
    /// Frames of the retimed source before the clip's first frame: the
    /// parts a split or trim leaves show the frames they showed.
    #[serde(default, skip_serializing_if = "is_zero_frames")]
    pub skip: i64,
    /// The OTIO time effect as read from another editor's file (deviation
    /// X9), written back while the clip keeps the speed that it gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Kept>,
}

/// A section of a clip's speed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Section {
    /// Its first frame on the retimed source.
    pub at: i64,
    /// The source position there, in source frames from the section list's
    /// source 0.
    pub from: f64,
    /// Source frames for each frame: 2.0 twice as fast, 0.5 half, -1.0
    /// backward, 0 a freeze frame.
    pub speed: f64,
    /// The ease of the speed keyframe at `at`: the frames before and after
    /// it in which the speed changes on a curve. Left out, the speed
    /// changes at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<[f64; 2]>,
}

fn is_zero_frames(n: &i64) -> bool {
    *n == 0
}

impl Speed {
    pub fn constant(speed: f64) -> Self {
        Self {
            sections: Arc::from([Section {
                at: 0,
                from: 0.0,
                speed,
                ease: None,
            }]),
            skip: 0,
            effect: None,
        }
    }

    pub fn scalar(&self) -> Option<f64> {
        match &*self.sections {
            [one] => Some(one.speed),
            _ => None,
        }
    }

    pub fn keyframes(&self) -> Vec<Keyframe> {
        self.sections
            .iter()
            .skip(1)
            .map(|s| Keyframe {
                frame: s.at,
                value: s.speed,
                in_handle: None,
                out_handle: None,
            })
            .collect()
    }

    pub fn label(&self) -> String {
        match self.scalar() {
            Some(0.0) => "freeze".to_string(),
            Some(x) => format!("{}%", (x * 100.0).round()),
            None => "ramp".to_string(),
        }
    }
}

/// A generator or title from another editor's file (deviation X9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Generator {
    /// The OTIO media reference as read: a `GeneratorReference`.
    pub reference: Kept,
}

/// A title: text drawn over the picture (decision D65). A field left out
/// keeps the usual look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Title {
    /// `text`, one block of text, `lower_third`, two lines at the bottom
    /// of the title-safe area, or `scroll`, lines that rise from below the
    /// picture to above it over the clip.
    #[serde(default, skip_serializing_if = "TitleKind::is_text")]
    pub kind: TitleKind,
    /// How a lower third or a scroll lines up: `left`, `middle` or `right`.
    /// Usual: left for a lower third, middle for a scroll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TitleAlign>,
    /// Its lines, separated by `\n`: two at most for a lower third. `<b>`,
    /// `<i>`, `<u>` and `<font color="…" face="…" size="…">` style the words
    /// between them apart from the rest, `size` a fraction of the picture
    /// height like the style's; `&lt;`, `&gt;` and `&amp;` stand for `<`,
    /// `>` and `&`.
    pub text: String,
    #[serde(default, skip_serializing_if = "TitleStyle::is_empty")]
    pub style: TitleStyle,
    /// Where it goes, as fractions of the picture's width and height from
    /// its top left: the middle of a `text` title's block (usual: `[0.5,
    /// 0.5]`), a lower third's bottom left, middle or right corner by its
    /// `align` (usual: 10%, 50% or 90% across and 88% down, so that its
    /// lower line's top sits about a fifth up the picture: `[0.1, 0.88]`,
    /// `[0.5, 0.88]` and `[0.9, 0.88]`). A scroll's `x` is where its lines'
    /// left edge, middle or right edge goes by its `align` (usual: 5%, 50%
    /// or 95% across); its `y` is not used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    /// How much larger its text is drawn, from 1/4 to 4, about the middle of
    /// a `text` title or the point where a lower third's lines meet; a
    /// scroll has none. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    /// Degrees it is turned counterclockwise, with its background and
    /// shadow, about the same point as `zoom`. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// How opaque the whole title is, from 0 to 1. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
}

/// What kind of title: one block of text, a lower third, or a scroll.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TitleKind {
    #[default]
    Text,
    LowerThird,
    Scroll,
}

impl TitleKind {
    pub fn is_text(&self) -> bool {
        *self == Self::Text
    }
}

/// Where a lower third sits at the bottom of the title-safe area, or a
/// scroll across the picture, and how their lines line up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TitleAlign {
    #[default]
    Left,
    Middle,
    Right,
}

/// How a title looks. A field left out keeps the usual look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct TitleStyle {
    /// Font family, used when that font is installed. Usual: the burned-in
    /// subtitles' font.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Text height as a fraction of the picture height, above 0 and at most
    /// 1/2. Usual: 96/1080 (about 0.089) for `text`, 0.06 for a lower third.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// `#rrggbb`, `#rgb` or a name such as `yellow`. Usual: white.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Outline color and thickness. Usual: black, a twentieth of the text
    /// height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<SubtitleOutline>,
    /// A box behind the whole block of text. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<SubtitleBackground>,
    /// A soft copy of the text behind it, over its background. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<TitleShadow>,
}

/// A title's drop shadow. A field left out keeps the usual look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct TitleShadow {
    /// `#rrggbb`, `#rgb` or a name such as `navy`. Usual: black.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// From 0 to 1. Usual: 0.75.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// `[x, y]`: how far it falls right and down, as fractions of the
    /// picture's width and height, each from -1/2 to 1/2. Usual: `[0.003,
    /// 0.005]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<[f64; 2]>,
    /// How soft its edge is, from 0 (sharp) to 100, where the blur's radius
    /// is about 3% of the picture height. Usual: 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
}

impl TitleShadow {
    pub const OPACITY: f64 = 0.75;
    pub const OFFSET: [f64; 2] = [0.003, 0.005];
    pub const BLUR: f64 = 20.0;
    pub const BLUR_PER_PICTURE: f64 = 3300.0;
    pub const GAIN: f64 = 1.24;

    pub fn ink(&self) -> [u8; 3] {
        paint(self.color.as_deref(), [0, 0, 0])
    }

    pub fn alpha(&self) -> f64 {
        self.opacity.unwrap_or(Self::OPACITY)
    }

    pub fn shift(&self) -> [f64; 2] {
        self.offset.unwrap_or(Self::OFFSET)
    }

    pub fn softness(&self) -> f64 {
        self.blur.unwrap_or(Self::BLUR)
    }
}

impl TitleStyle {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl Title {
    pub const SAFE: f64 = 0.9;
    pub const LOWER_IN: f64 = 0.1;
    pub const LOWER_DOWN: f64 = 0.88;

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            kind: TitleKind::Text,
            align: None,
            text: text.into(),
            style: TitleStyle::default(),
            position: None,
            zoom: None,
            rotation: None,
            opacity: None,
        }
    }

    pub fn align(&self) -> TitleAlign {
        match self.kind {
            TitleKind::Text => TitleAlign::Middle,
            TitleKind::LowerThird => self.align.unwrap_or_default(),
            TitleKind::Scroll => self.align.unwrap_or(TitleAlign::Middle),
        }
    }

    pub fn usual_position(kind: TitleKind, align: TitleAlign) -> [f64; 2] {
        let edge = (1.0 - Self::SAFE) / 2.0;
        match (kind, align) {
            (TitleKind::Text, _) => [0.5, 0.5],
            (TitleKind::LowerThird, TitleAlign::Left) => [Self::LOWER_IN, Self::LOWER_DOWN],
            (TitleKind::LowerThird, TitleAlign::Middle) => [0.5, Self::LOWER_DOWN],
            (TitleKind::LowerThird, TitleAlign::Right) => [1.0 - Self::LOWER_IN, Self::LOWER_DOWN],
            (TitleKind::Scroll, TitleAlign::Left) => [edge, 0.5],
            (TitleKind::Scroll, TitleAlign::Middle) => [0.5, 0.5],
            (TitleKind::Scroll, TitleAlign::Right) => [1.0 - edge, 0.5],
        }
    }

    pub fn place(&self) -> [f64; 2] {
        self.position
            .unwrap_or_else(|| Self::usual_position(self.kind, self.align()))
    }

    pub fn anchor(&self) -> [f64; 2] {
        let across = match self.align() {
            TitleAlign::Left => 0.0,
            TitleAlign::Middle => 0.5,
            TitleAlign::Right => 1.0,
        };
        match self.kind {
            TitleKind::Text => [0.5, 0.5],
            TitleKind::LowerThird => [across, 1.0],
            TitleKind::Scroll => [across, 0.0],
        }
    }

    pub fn usual_size(kind: TitleKind) -> f64 {
        match kind {
            TitleKind::Text | TitleKind::Scroll => 96.0 / 1080.0,
            TitleKind::LowerThird => 0.06,
        }
    }

    pub fn moves(&self) -> bool {
        self.kind == TitleKind::Scroll
    }

    pub fn text_fraction(&self) -> f64 {
        self.style.size.unwrap_or(Self::usual_size(self.kind))
    }

    pub fn ink(&self) -> [u8; 3] {
        paint(self.style.color.as_deref(), [255, 255, 255])
    }

    pub fn stroke(&self) -> Option<([u8; 3], f64)> {
        stroke_of(self.style.outline.as_ref())
    }

    pub fn backing(&self) -> Option<([u8; 3], f64)> {
        backing_of(self.style.background.as_ref())
    }

    pub fn wrap_width(&self, picture_width: f64, picture_height: f64) -> f64 {
        (picture_width * Self::SAFE).max(self.text_fraction() * picture_height)
    }

    pub fn scale(&self) -> f64 {
        self.zoom.unwrap_or(1.0)
    }

    pub fn turn(&self) -> f64 {
        self.rotation.unwrap_or(0.0)
    }

    pub fn opaque(&self) -> f64 {
        self.opacity.unwrap_or(1.0)
    }

    pub fn pivot(&self) -> [f64; 2] {
        let [x, y] = self.place();
        match self.kind {
            TitleKind::Text | TitleKind::Scroll => [x, y],
            TitleKind::LowerThird => [x, y - self.lower_line()],
        }
    }

    pub fn lower_line(&self) -> f64 {
        let lines = crate::rich::lines(&self.text);
        let base = self.text_fraction();
        let size = lines
            .last()
            .map_or(base, |line| crate::rich::largest(line, base));
        size * SubtitleStyle::LINE
    }

    pub fn label(&self) -> String {
        let text = crate::rich::plain(&self.text);
        let line = text.lines().find(|l| !l.trim().is_empty());
        line.map_or_else(|| "Title".to_string(), |l| l.trim().to_string())
    }
}

/// Where a clip's picture goes over the frame: footage, a still, an image
/// sequence or a compound clip. A field left out is at its usual value; at
/// all of them the picture is fitted into the frame. The tracks below show
/// wherever it doesn't cover the frame: in the bars around a picture of
/// another shape, around it, where it is cropped and through its soft edges
/// and its opacity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Picture {
    /// Where the middle of the whole picture goes before the zoom and the
    /// rotation, `[x, y]` as fractions of the frame from its top left, each
    /// from -100 to 100. With an anchor away from the middle, the zoom and
    /// the rotation then move the middle about the anchor. Usual: `[0.5,
    /// 0.5]`. With `position_keys`, the place at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    /// Position keyframes, one for X and Y (decision D75). `value` is X.
    /// `position_y` is Y, one number for each keyframe, in the same order.
    /// A handle value moves X. Y keeps the handle frames and a value
    /// offset of 0. Left out when the position has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_keys: Vec<Keyframe>,
    /// Y of each `position_keys` entry. Left out with those keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_y: Vec<f64>,
    /// How much larger it is drawn across and down, about its anchor, `[x,
    /// y]`, each from 1/100 to 100. Usual: `[1, 1]`. With `zoom_keys`, the
    /// zoom at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<[f64; 2]>,
    /// Zoom keyframes, one for X and Y (decision D75). `value` is X.
    /// `zoom_y` is Y, one number for each keyframe, in the same order.
    /// A handle value moves X. Y keeps the handle frames and a value
    /// offset of 0. Left out when the zoom has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zoom_keys: Vec<Keyframe>,
    /// Y of each `zoom_keys` entry. Left out with those keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zoom_y: Vec<f64>,
    /// Degrees it is turned counterclockwise about its anchor, from -3600 to
    /// 3600. Usual: 0. With `rotation_keys`, the angle at a frame is that
    /// curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// Rotation keyframes, in degrees (decision D75). Left out when the
    /// rotation has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotation_keys: Vec<Keyframe>,
    /// The point of the picture that the zoom and the rotation work about,
    /// `[x, y]` as fractions of the picture from its top left, as it shows
    /// after the flips, each from -100 to 100. A Dynamic Zoom does not move
    /// it. Usual: `[0.5, 0.5]`, the middle. With `anchor_keys`, the anchor
    /// point at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<[f64; 2]>,
    /// Anchor point keyframes, one for X and Y (decision D92). `value` is
    /// X. `anchor_y` is Y, one number for each keyframe, in the same
    /// order. A handle value moves X. Y keeps the handle frames and a
    /// value offset of 0. Left out when the anchor point has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchor_keys: Vec<Keyframe>,
    /// Y of each `anchor_keys` entry. Left out with those keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchor_y: Vec<f64>,
    /// Pitch about the middle of the picture, in its own axes, before the
    /// zoom and the rotation, from -1.5 to 1.5. Positive puts the top
    /// farther away. Usual: 0. With `pitch_keys`, the pitch at a frame is
    /// that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    /// Pitch keyframes, from -1.5 to 1.5 (decision D93). Left out when the
    /// pitch has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pitch_keys: Vec<Keyframe>,
    /// Yaw about the middle of the picture, in its own axes, before the
    /// zoom and the rotation, from -1.5 to 1.5. Positive puts the left
    /// closer. Usual: 0. With `yaw_keys`, the yaw at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
    /// Yaw keyframes, from -1.5 to 1.5 (decision D93). Left out when the
    /// yaw has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub yaw_keys: Vec<Keyframe>,
    /// Mirrored left to right.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip_horizontal: bool,
    /// Mirrored top to bottom.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip_vertical: bool,
    /// What is cut off each side, as fractions of the source picture; the
    /// rest stays where it was. A side with keyframes takes its curve at
    /// the frame (`crop_left_keys` and the three beside it).
    #[serde(default, skip_serializing_if = "Crop::is_none")]
    pub crop: Crop,
    /// Crop Left keyframes, as fractions of the width (decision D75).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crop_left_keys: Vec<Keyframe>,
    /// Crop Right keyframes, as fractions of the width.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crop_right_keys: Vec<Keyframe>,
    /// Crop Top keyframes, as fractions of the height.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crop_top_keys: Vec<Keyframe>,
    /// Crop Bottom keyframes, as fractions of the height.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crop_bottom_keys: Vec<Keyframe>,
    /// How soft the edges of the crop are, from -100 to 100. Above 0, each
    /// cropped edge fades out over what it cuts off. Below 0, all four edges
    /// fade in over the picture, cropped or not. The fade is |softness| / 400
    /// of the width of the picture fitted into the frame, before the zoom.
    /// Usual: 0. With `crop_softness_keys`, the softness at a frame is that
    /// curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop_softness: Option<f64>,
    /// Crop Softness keyframes, from -100 to 100 (decision D75).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crop_softness_keys: Vec<Keyframe>,
    /// A slow push or pull and pan across the clip, from a start box to an
    /// end box. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_zoom: Option<DynamicZoom>,
    /// How it mixes with the tracks below. Usual: `normal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composite_mode: Option<CompositeMode>,
    /// How much of it shows over the tracks below, from 0 (none) to 1 (all
    /// of it): the mix of its composite mode, faded over them. Usual: 1.
    /// With `opacity_keys`, the opacity at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Opacity keyframes, from 0 to 1 (decision D75).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opacity_keys: Vec<Keyframe>,
}

/// Dynamic Zoom: the part of the picture that fills its place goes from
/// the `start` box on the clip's first frame to the `end` box a frame
/// after its last, along the `ease`. The picture is drawn larger about the
/// box's middle, which goes to the picture's middle, after the flips and
/// before the zoom, rotation and position. Turned on, it is a pull out: a
/// start box 0.8 of the picture and an end box all of it, both in the
/// middle, `linear`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DynamicZoom {
    pub start: ZoomBox,
    pub end: ZoomBox,
    /// How it speeds up and slows down. Usual: `linear`.
    #[serde(default, skip_serializing_if = "Ease::is_linear")]
    pub ease: Ease,
}

/// A box of a Dynamic Zoom, the picture's shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ZoomBox {
    /// Its middle, `[x, y]` as fractions of the picture from its top left,
    /// as it shows after the flips, each from -100 to 100: `[0.5, 0.5]` is
    /// the picture's middle.
    pub center: [f64; 2],
    /// Its width and height as a fraction of the picture's, from 1/100 to
    /// 100.
    pub size: f64,
}

/// How a Dynamic Zoom goes from its start box to its end box, with `t` the
/// share of the clip played, from 0 to 1. On a keyframe, the handles it
/// gets: flat, a third of the way to the keyframe before (In) or after
/// (Out).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// At one speed: `t`. On a keyframe: no handles.
    #[default]
    Linear,
    /// Slow at first: `t²`. On a keyframe: an In handle.
    EaseIn,
    /// Slow at the end: `1 − (1 − t)²`. On a keyframe: an Out handle.
    EaseOut,
    /// Slow at both ends: `3t² − 2t³`. On a keyframe: both handles.
    EaseInOut,
}

impl Ease {
    pub const ALL: [Ease; 4] = [Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::EaseInOut];

    pub fn is_linear(&self) -> bool {
        *self == Self::Linear
    }

    pub fn of(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Self::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

impl Default for DynamicZoom {
    fn default() -> Self {
        Self {
            start: ZoomBox {
                center: Picture::MIDDLE,
                size: 0.8,
            },
            end: ZoomBox {
                center: Picture::MIDDLE,
                size: 1.0,
            },
            ease: Ease::Linear,
        }
    }
}

impl DynamicZoom {
    pub fn at(&self, t: f64) -> ZoomBox {
        let e = self.ease.of(t);
        let (a, b) = (self.start, self.end);
        let mix = |x: f64, y: f64| x + (y - x) * e;
        ZoomBox {
            center: [mix(a.center[0], b.center[0]), mix(a.center[1], b.center[1])],
            size: mix(a.size, b.size),
        }
    }

    pub fn swapped(&self) -> Self {
        Self {
            start: self.end,
            end: self.start,
            ease: self.ease,
        }
    }

    pub fn closest(&self) -> f64 {
        self.start.size.min(self.end.size)
    }
}

/// What is cut off each side of a picture, as fractions of its width
/// (`left`, `right`) and height (`top`, `bottom`), each from 0 up to what
/// leaves some of it. Usual: 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Crop {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub left: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub right: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub top: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bottom: f64,
}

impl Crop {
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }

    pub fn sides(&self) -> [f64; 4] {
        [self.left, self.right, self.top, self.bottom]
    }
}

/// How a picture mixes with the tracks below it, on each channel of their
/// code values from 0 (black) to 1 (white): `a` is the picture, `b` is
/// what is below it. Results above 1 and below 0 are kept for the opacity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompositeMode {
    /// `a`.
    #[default]
    Normal,
    /// `a + b`.
    Add,
    /// `b − a`.
    Subtract,
    /// `|a − b|`.
    Difference,
    /// `a × b`.
    Multiply,
    /// `1 − (1 − a)(1 − b)`.
    Screen,
    /// `2ab` where `b` is up to 0.5, else `1 − 2(1 − a)(1 − b)`.
    Overlay,
    /// `2ab` where `a` is up to 0.5, else `1 − 2(1 − a)(1 − b)`.
    Hardlight,
    /// `b − (1 − 2a)b(1 − b)` where `a` is up to 0.5, else `b + (2a − 1)(d
    /// − b)`, with `d` = `√b`, or `((16b − 12)b + 4)b` where `b` is up to
    /// 0.25.
    Softlight,
    /// The lower of `a` and `b`.
    Darken,
    /// The higher of `a` and `b`.
    Lighten,
    /// `b / (1 − a)`, up to 1; 1 where `a` is 1.
    ColorDodge,
    /// `1 − (1 − b) / a`, from 0; 0 where `a` is 0.
    ColorBurn,
    /// `a + b − 2ab`.
    Exclusion,
    /// The hue of `a`, with the colorfulness and the luma (BT.709) of `b`.
    Hue,
    /// The colorfulness of `a`, with the hue and the luma (BT.709) of `b`.
    Saturation,
    /// The chroma of `a`, with the luma (BT.601) of `b`.
    Color,
    /// The luma (BT.601) of `a`, with the chroma of `b`.
    Luminosity,
    /// `b / a`, up to 1.
    Divide,
    /// `a + b`, as `add`.
    LinearDodge,
    /// `a + b − 1`.
    LinearBurn,
    /// `b + 2a − 1`.
    LinearLight,
    /// `color_burn` by `2a` where `a` is up to 0.5, else `color_dodge` by
    /// `2a − 1`.
    VividLight,
    /// The lower of `b` and `2a` where `a` is up to 0.5, else the higher of
    /// `b` and `2a − 1`.
    PinLight,
    /// 1 where `a + b` is 1 or more, else 0.
    HardMix,
    /// `a` where the sum of its channels is more than that of `b`, else `b`.
    LighterColor,
    /// `a` where the sum of its channels is less than that of `b`, else `b`.
    DarkerColor,
}

impl CompositeMode {
    pub const ALL: [CompositeMode; 27] = [
        Self::Normal,
        Self::Add,
        Self::Subtract,
        Self::Difference,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Hardlight,
        Self::Softlight,
        Self::Darken,
        Self::Lighten,
        Self::ColorDodge,
        Self::ColorBurn,
        Self::Exclusion,
        Self::Hue,
        Self::Saturation,
        Self::Color,
        Self::Luminosity,
        Self::Divide,
        Self::LinearDodge,
        Self::LinearBurn,
        Self::LinearLight,
        Self::VividLight,
        Self::PinLight,
        Self::HardMix,
        Self::LighterColor,
        Self::DarkerColor,
    ];

    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as u32
    }

    pub fn of_code(code: u32) -> Option<Self> {
        Self::ALL.get(usize::try_from(code).ok()?).copied()
    }
}

/// Which of a picture's tools: `transform` (its position, zoom, rotation,
/// anchor, pitch, yaw and flips), `crop` (its sides and softness),
/// `dynamic_zoom` or `composite` (its composite mode and opacity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PictureTool {
    Transform,
    Crop,
    #[serde(rename = "dynamic_zoom")]
    DynamicZoom,
    Composite,
}

impl Picture {
    pub const MIDDLE: [f64; 2] = [0.5, 0.5];

    pub fn is_usual(&self) -> bool {
        *self == Self::default()
    }

    pub fn place(&self) -> [f64; 2] {
        self.position.unwrap_or(Self::MIDDLE)
    }

    pub fn scale(&self) -> [f64; 2] {
        self.zoom.unwrap_or([1.0, 1.0])
    }

    pub fn turn(&self) -> f64 {
        self.rotation.unwrap_or(0.0)
    }

    pub fn pivot(&self) -> [f64; 2] {
        self.anchor.unwrap_or(Self::MIDDLE)
    }

    pub fn pitch(&self) -> f64 {
        self.pitch.unwrap_or(0.0)
    }

    pub fn yaw(&self) -> f64 {
        self.yaw.unwrap_or(0.0)
    }

    pub fn softness(&self) -> f64 {
        self.crop_softness.unwrap_or(0.0)
    }

    pub fn opaque(&self) -> f64 {
        self.opacity.unwrap_or(1.0)
    }

    pub fn mode(&self) -> CompositeMode {
        self.composite_mode.unwrap_or_default()
    }
}

/// A compound clip or nested timeline from another editor's file
/// (deviation X9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Compound {
    /// The nested OTIO `Stack` as read, without its source range and clip
    /// effects (the clip's own).
    pub stack: Kept,
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
    /// The frame their keyframe times count from, on the axis of the
    /// keyframes (`Keyframe`'s `frame`): the first of the picture or sound
    /// when they were read. Keyframes stay on their frames through trims,
    /// slips and splits.
    pub from: i64,
    /// OTIO `Effect`s as read, keyframe times and fade lengths in project
    /// frames.
    pub list: Vec<Kept>,
}

/// A value of a clip at one of its source frames (decision D81). Before
/// the first keyframe, the value is the first one's; after the last, the
/// last one's. Between two keyframes, it follows the cubic Bezier curve
/// through the first, its Out handle, the second's In handle and the
/// second. A missing handle is on its keyframe, so without handles the
/// change is a straight line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Keyframe {
    /// The source frame, counted as a clip's `source_in` is, or on a clip
    /// with a speed, its frame on the retimed source, counted as the speed's
    /// `skip` is. The keyframe stays on its content through trims, slips
    /// and splits.
    pub frame: i64,
    /// The value there: the volume in dB, from -60 to 24.
    pub value: f64,
    /// The In handle, which shapes the change before the keyframe.
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub in_handle: Option<Handle>,
    /// The Out handle, which shapes the change after the keyframe.
    #[serde(rename = "out", default, skip_serializing_if = "Option::is_none")]
    pub out_handle: Option<Handle>,
}

/// A Bezier handle of a keyframe: a point `frames` and `value` from it.
/// An In handle is before its keyframe (`frames` at most 0), an Out handle
/// after it. The curve takes a handle no farther than the next keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Handle {
    pub frames: f64,
    pub value: f64,
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
    /// The volume in dB, from -60 to 24, while the clip has no volume
    /// keyframes.
    pub gain_db: f64,
    /// The volume keyframes (decision D81), in the order of their frames,
    /// one at most on a frame. With them, the volume is their curve.
    /// Left out when the clip has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gain_keys: Vec<Keyframe>,
    pub muted: bool,
    /// Effects on its sound, carried from another editor. The clip volume
    /// is `gain_db` and `gain_keys`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Effects>,
    /// Voice Isolation's Amount, from 1 to 100: that much of the voice the
    /// model hears in the sound, mixed with the rest of the sound. 0 keeps
    /// it off, also when the clip's track has it on. Left out, the clip
    /// follows its track's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_isolation: Option<f64>,
}

impl Clip {
    pub fn shown_color<'a>(&'a self, media: Option<&'a Media>) -> Option<&'a str> {
        self.color
            .as_deref()
            .or_else(|| media.and_then(|m| m.color.as_deref()))
    }

    pub fn is_generator(&self) -> bool {
        self.generator.is_some() || self.title.is_some()
    }

    pub fn is_block(&self) -> bool {
        self.is_generator() || self.compound.is_some()
    }

    pub fn end(&self) -> i64 {
        self.start + self.duration
    }

    pub fn audio_range(&self) -> Option<(i64, i64)> {
        self.audio
            .as_ref()
            .map(|a| (self.start + a.head, self.end() + a.tail))
    }

    pub fn pace(&self) -> crate::render::Pace {
        match &self.speed {
            None => crate::render::Pace::default(),
            Some(s) => crate::render::Pace::mapped(s.sections.clone(), s.skip),
        }
    }

    pub fn source_at(&self, t: i64) -> i64 {
        match &self.speed {
            None => self.source_in + t - self.start,
            Some(_) => self.source_in + self.pace().offset(t - self.start),
        }
    }

    pub fn runs(&self, a: i64, b: i64) -> Vec<(i64, i64)> {
        let pace = self.pace();
        let mut out = Vec::new();
        let mut x = a;
        while x < b {
            let y = pace.steady(x - self.start).map_or(b, |n| (x + n).min(b));
            out.push((x, y));
            x = y;
        }
        out
    }

    pub fn sound_at(&self, t: f64) -> f64 {
        self.source_in as f64 + self.pace().sound(t - self.start as f64)
    }

    pub fn walk(&mut self, k: i64) {
        self.source_in += self.pace().offset(k);
        if let Some(s) = self.speed.as_mut() {
            s.skip += k;
        }
    }

    pub fn move_head(&mut self, k: i64) {
        self.start += k;
        self.duration -= k;
        self.walk(k);
    }

    pub fn shown(&self) -> (i64, i64) {
        let (mut lo, mut hi) = (i64::MAX, i64::MIN);
        for (a, b) in self.runs(self.start, self.end()) {
            for s in [self.source_at(a), self.source_at(b - 1)] {
                (lo, hi) = (lo.min(s), hi.max(s));
            }
        }
        (lo, hi + 1)
    }

    pub fn sound_span(&self) -> Option<(f64, f64)> {
        let (a, b) = self.audio_range()?;
        if self.constant_speed().is_some() {
            if self.speed_at(self.start) == 0.0 {
                return None;
            }
            let (x, y) = (self.sound_at(a as f64), self.sound_at(b as f64));
            return Some((x.min(y), x.max(y)));
        }
        let pace = self.pace();
        let base = self.source_in as f64;
        let mut span: Option<(f64, f64)> = None;
        for (x, y) in self.runs(a, b) {
            let k = x - self.start;
            if pace.speed_at(k) == 0.0 {
                continue;
            }
            for t in [x, y] {
                let s = base + pace.sound_in(k, (t - self.start) as f64);
                span = Some(span.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))));
            }
        }
        span
    }

    pub fn room(&self, tail: bool, have: Range<i64>) -> i64 {
        let at = |n: i64| {
            let t = if tail {
                self.end() - 1 + n
            } else {
                self.start - n
            };
            have.contains(&self.source_at(t))
        };
        let span = (have.end - have.start) as f64;
        let pace = self.pace();
        let (reach, edge) = if tail {
            let last = pace.map.last().expect("a section");
            ((last.at - pace.skip - self.duration + 1).max(0), last.speed)
        } else {
            let reach = pace.map.get(1).map_or(0, |s| (pace.skip - s.at + 1).max(0));
            (reach, pace.map[0].speed)
        };
        if edge == 0.0 && (reach == 0 || at(reach)) {
            return i64::MAX / 4;
        }
        let slow = pace
            .map
            .iter()
            .map(|s| s.speed.abs())
            .filter(|&v| v > 0.0)
            .fold(f64::INFINITY, f64::min);
        let far = ((span + 2.0) / slow).min((i64::MAX / 4) as f64);
        let (mut lo, mut hi) = (0, (far as i64 + 2).saturating_add(reach));
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

    pub fn content_at(&self, t: i64) -> i64 {
        match &self.speed {
            None => self.source_in + t - self.start,
            Some(s) => s.skip + t - self.start,
        }
    }

    pub fn in_step(&self, other: &Clip) -> bool {
        match (&self.speed, &other.speed) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.sections == b.sections
                    && other.source_in == self.source_in + self.pace().offset(b.skip - a.skip)
            }
            _ => false,
        }
    }

    pub fn constant_speed(&self) -> Option<f64> {
        match &self.speed {
            None => Some(1.0),
            Some(s) => s.scalar(),
        }
    }

    pub fn speed_at(&self, t: i64) -> f64 {
        match &self.speed {
            None => 1.0,
            Some(s) => match s.scalar() {
                Some(x) => x,
                None => self.pace().speed_at(t - self.start),
            },
        }
    }

    pub fn is_frozen(&self) -> bool {
        self.constant_speed() == Some(0.0)
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

/// How a subtitle track looks burned in, and in an IMSC1 file. A field
/// left out keeps the usual look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SubtitleStyle {
    /// Font family. The burn-in uses it when that font is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Text height as a fraction of the picture height, above 0 and at most
    /// 1/2. Usual: 0.055.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// `#rrggbb`, `#rgb` or a name such as `yellow`. Usual: white.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Outline color and thickness. Usual: black, a twentieth of the text
    /// height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<SubtitleOutline>,
    /// A box behind each line of text. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<SubtitleBackground>,
    /// Inset from the left and right, as a fraction of the picture width,
    /// and from the top or bottom for a cue placed there, as a fraction of
    /// the height. Usual: wrapped at 90% of the width, the block centered
    /// at 80%, 50% or 20% of the height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// The font's bold face. Usual: its regular one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    /// Every letter drawn as a capital (`upper`) or a small letter
    /// (`lower`), whatever the text has. Usual: as the text has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<LetterCase>,
    /// A soft copy of the text behind it. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<TitleShadow>,
    /// The words shown as they are spoken, by the cues' word times. A cue
    /// without word times shows all at once. Usual: all at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<SubtitleWords>,
    /// What a cue's color, its own or its speaker's, paints: `text`, all of
    /// it but the words colored in it; `spoken`, the spoken word, or all of
    /// a cue whose words show at once; or `none`. Usual: `spoken` with
    /// `words`, else `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_color: Option<SpeakerPaint>,
    /// `left`, `center` or `right`. Usual: `center`. On a cue, left out
    /// keeps the track's alignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<SubtitleAlign>,
    /// Where the block's alignment point sits, `[x, y]` as fractions of
    /// the picture from its top left, each from 0 to 1. Usual: the cue's
    /// place. On a cue, left out keeps the track's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
}

/// What a cue's own or its speaker's color paints.
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpeakerPaint {
    Text,
    Spoken,
    None,
}

/// How the lines of a subtitle sit in their block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleAlign {
    Left,
    Center,
    Right,
}

/// Letters drawn all as capitals or all as small letters.
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LetterCase {
    Upper,
    Lower,
}

/// How a cue's words show as they are spoken. A word is spoken from its
/// start until the next word starts, the last one until the cue ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SubtitleWords {
    /// Usual: `highlight`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<WordsShow>,
    /// The spoken word's color: `#rrggbb`, `#rgb` or a name such as
    /// `yellow`. Usual: `#ffd400` for `highlight`, the text's color
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// How large the spoken word starts, as a multiple of the text's size,
    /// from 1 to 2: it eases back to the text's size within 0.2 s, about
    /// its own middle, over the words beside it. Usual: 1, no pop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pop: Option<f64>,
}

/// Which of a cue's words show while it is spoken.
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WordsShow {
    /// Every word, the spoken one in its color.
    #[default]
    Highlight,
    /// Each word from when it is spoken, the others keeping their room.
    Reveal,
    /// Only the spoken word.
    Word,
}

/// An outline around a subtitle track's or a title's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SubtitleOutline {
    /// `#rrggbb`, `#rgb` or a name. Usual: black.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Thickness as a fraction of the text height, from 0 to 1/10. Usual:
    /// 0.05. `0` draws none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
}

/// A box behind a subtitle track's or a title's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubtitleBackground {
    /// `#rrggbb`, `#rgb` or a name.
    pub color: String,
    /// From 0 to 1. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
}

impl SubtitleStyle {
    pub const LINE: f64 = 1.31;
    pub const PADDING: f64 = 0.15;

    pub fn is_empty(&self) -> bool {
        self.font.is_none()
            && self.size.is_none()
            && self.color.is_none()
            && self.outline.is_none()
            && self.background.is_none()
            && self.margin.is_none()
            && self.bold.is_none()
            && self.case.is_none()
            && self.shadow.is_none()
            && self.words.is_none()
            && self.speaker_color.is_none()
            && self.align.is_none()
            && self.position.is_none()
    }

    pub fn over(&self, cue: &Self) -> Self {
        Self {
            font: cue.font.clone().or_else(|| self.font.clone()),
            size: cue.size.or(self.size),
            color: cue.color.clone().or_else(|| self.color.clone()),
            outline: cue.outline.clone().or_else(|| self.outline.clone()),
            background: cue.background.clone().or_else(|| self.background.clone()),
            margin: cue.margin.or(self.margin),
            bold: cue.bold.or(self.bold),
            case: cue.case.or(self.case),
            shadow: cue.shadow.clone().or_else(|| self.shadow.clone()),
            words: cue.words.clone().or_else(|| self.words.clone()),
            speaker_color: cue.speaker_color.or(self.speaker_color),
            align: cue.align.or(self.align),
            position: cue.position.or(self.position),
        }
    }

    pub fn line_align(&self) -> SubtitleAlign {
        self.align.unwrap_or(SubtitleAlign::Center)
    }

    pub fn for_cue(&self, cue: Option<&Self>) -> Self {
        match cue {
            Some(over) => self.over(over),
            None => self.clone(),
        }
    }

    pub fn paints(&self) -> SpeakerPaint {
        match (self.speaker_color, &self.words) {
            (Some(p), _) => p,
            (None, Some(_)) => SpeakerPaint::Spoken,
            (None, None) => SpeakerPaint::Text,
        }
    }

    pub fn text_fraction(&self) -> f64 {
        self.size.unwrap_or(0.055)
    }

    pub fn ink(&self) -> [u8; 3] {
        paint(self.color.as_deref(), [255, 255, 255])
    }

    pub fn stroke(&self) -> Option<([u8; 3], f64)> {
        stroke_of(self.outline.as_ref())
    }

    pub fn backing(&self) -> Option<([u8; 3], f64)> {
        backing_of(self.background.as_ref())
    }

    pub fn inset(&self) -> f64 {
        self.margin.unwrap_or(0.05)
    }

    pub fn wrap_width(&self, picture_width: f64, picture_height: f64) -> f64 {
        (picture_width * (1.0 - 2.0 * self.inset())).max(self.text_fraction() * picture_height)
    }

    pub fn anchor(&self, place: Place) -> (f64, f64) {
        match (self.margin, place) {
            (Some(m), Place::Top) => (m, 0.0),
            (Some(m), Place::Bottom) => (1.0 - m, 1.0),
            (_, Place::Middle) => (0.5, 0.5),
            (None, Place::Top) => (0.2, 0.5),
            (None, Place::Bottom) => (0.8, 0.5),
        }
    }

    pub fn block_top(&self, place: Place, picture_height: f64, lines: f64) -> f64 {
        let (at, above) = self.anchor(place);
        picture_height * at - lines * above
    }

    pub fn block_box(
        &self,
        place: Place,
        (picture_w, picture_h): (f64, f64),
        wrap: f64,
        lines: f64,
    ) -> (f64, f64) {
        let (at, above) = self.anchor(place);
        match self.position {
            Some([x, y]) => {
                let across = match self.line_align() {
                    SubtitleAlign::Left => 0.0,
                    SubtitleAlign::Center => 0.5,
                    SubtitleAlign::Right => 1.0,
                };
                (picture_w * x - wrap * across, picture_h * y - lines * above)
            }
            None => ((picture_w - wrap) / 2.0, picture_h * at - lines * above),
        }
    }
}

fn paint(hex: Option<&str>, usual: [u8; 3]) -> [u8; 3] {
    hex.and_then(crate::subtitles::rgb).unwrap_or(usual)
}

fn stroke_of(o: Option<&SubtitleOutline>) -> Option<([u8; 3], f64)> {
    let size = o.and_then(|o| o.size).unwrap_or(0.05);
    (size > 0.0).then(|| (paint(o.and_then(|o| o.color.as_deref()), [0, 0, 0]), size))
}

fn backing_of(b: Option<&SubtitleBackground>) -> Option<([u8; 3], f64)> {
    let b = b?;
    Some((paint(Some(&b.color), [0, 0, 0]), b.opacity.unwrap_or(1.0)))
}

/// A subtitle track: cues shown over the picture, one language or use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubtitleTrack {
    /// `S<N>`, by place: S1 is the one nearest the video.
    pub id: String,
    /// A name shown in the track header and used in exported file names;
    /// `Subtitle N` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its language, as a code such as `en` or `pt-BR`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Shown in the viewer. One subtitle track at a time is.
    pub shown: bool,
    pub locked: bool,
    /// How it looks burned in and in an IMSC1 file.
    #[serde(default, skip_serializing_if = "SubtitleStyle::is_empty")]
    pub style: SubtitleStyle,
    /// Sorted by `start`; cues may overlap.
    pub cues: Vec<Cue>,
}

/// One word of a cue. `at` is left out when the word has no start frame.
/// `style` is left out when the word keeps the cue's style. A field left
/// out of `style` keeps the cue's style.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct CueWord {
    /// The frame it starts on, counted from the cue's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    /// Its own style. The whole value is left out when it keeps the cue's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<SubtitleStyle>,
}

impl CueWord {
    pub fn at(frame: i64) -> Self {
        Self {
            at: Some(frame),
            style: None,
        }
    }

    pub fn from_times(times: &[i64]) -> Vec<Self> {
        times.iter().copied().map(Self::at).collect()
    }
}

/// One subtitle: its text over a span of the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cue {
    /// `q<N>`, never reused.
    pub id: String,
    pub start: i64,
    /// Frames, at least one.
    pub duration: i64,
    /// Its lines, separated by `\n`. `<i>`, `<b>`, `<u>` and `<font
    /// color="#rrggbb">` mark italic, bold, underlined and colored words.
    pub text: String,
    /// Where it shows on the picture: the bottom unless set.
    #[serde(default, skip_serializing_if = "Place::is_bottom")]
    pub place: Place,
    /// Its words, in order. Left out when no word has a start frame or a
    /// style. A word with no `at` has no start frame. A word with no
    /// `style` keeps the cue's style.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<CueWord>,
    /// The person saying it, as `speakers.list` gives them. Create
    /// Subtitles from Audio sets it when it tells the speakers apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// Its own color, `#rrggbb`, instead of its speaker's, painted as the
    /// track's `speaker_color` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Its own style. A field left out keeps the track's style. The whole
    /// value is left out when the cue keeps the track's style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<SubtitleStyle>,
}

/// Where a cue shows on the picture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    #[default]
    Bottom,
    Middle,
    Top,
}

impl Place {
    pub fn is_bottom(&self) -> bool {
        *self == Self::Bottom
    }
}

impl Cue {
    pub fn end(&self) -> i64 {
        self.start + self.duration
    }

    pub fn has_times(&self) -> bool {
        self.words.iter().any(|w| w.at.is_some())
    }

    pub fn times(&self) -> Vec<i64> {
        if self.has_times() {
            self.words.iter().map(|w| w.at.unwrap_or(0)).collect()
        } else {
            Vec::new()
        }
    }
}

impl SubtitleTrack {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            language: None,
            shown: false,
            locked: false,
            style: SubtitleStyle::default(),
            cues: Vec::new(),
        }
    }

    pub fn display_name(&self) -> String {
        match &self.name {
            Some(n) if !n.is_empty() => n.clone(),
            _ => format!("Subtitle {}", self.id.trim_start_matches('S')),
        }
    }

    pub fn sort(&mut self) {
        self.cues.sort_by_key(|c| (c.start, c.end()));
    }
}

/// Counters for new ids.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NextIds {
    pub media: u64,
    pub clip: u64,
    pub marker: u64,
    pub transition: u64,
    #[serde(default = "first_id")]
    pub cue: u64,
}

fn first_id() -> u64 {
    1
}

impl Project {
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
            subtitles: Vec::new(),
            next_id: NextIds {
                media: 1,
                clip: 1,
                marker: 1,
                transition: 1,
                cue: 1,
            },
            bins: Vec::new(),
            speakers: Default::default(),
        }
    }

    pub fn track_index(&self, id: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    pub fn tracks_of(&self, kind: TrackKind) -> Vec<usize> {
        (0..self.tracks.len())
            .filter(|&i| self.tracks[i].kind == kind)
            .collect()
    }

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

    pub fn pool(&self) -> HashMap<&str, &Media> {
        self.media.iter().map(|m| (m.id.as_str(), m)).collect()
    }

    pub fn find_clip(&self, id: &str) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.clips.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    pub fn clip(&self, id: &str) -> Option<&Clip> {
        self.find_clip(id).map(|(t, c)| &self.tracks[t].clips[c])
    }

    pub fn clip_color<'a>(&'a self, c: &'a Clip) -> Option<&'a str> {
        c.shown_color(self.media(&c.media))
    }

    pub fn end(&self) -> i64 {
        let clips = self
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.audio_range().map_or(c.end(), |(_, e)| e.max(c.end())));
        let fades = self.tracks.iter().flat_map(|t| {
            t.transitions
                .iter()
                .filter(|x| x.to.is_none())
                .filter_map(|x| {
                    let c = t.clips.iter().find(|c| Some(&c.id) == x.from.as_ref())?;
                    let edge = if x.sound { c.audio_range()?.1 } else { c.end() };
                    Some(edge + x.after)
                })
        });
        clips.chain(fades).max().unwrap_or(0)
    }

    pub fn media_frames(&self, media: &Media) -> i64 {
        let secs = match (&media.info.video, media.info.audio.first()) {
            (Some(v), _) if v.still => self.settings.still_secs,
            (Some(v), _) => v.duration,
            (None, Some(a)) => a.duration,
            (None, None) => 0.0,
        };
        self.frames_in(secs)
    }

    pub fn media_limit(&self, media: &Media) -> i64 {
        if media.is_still() {
            self.frames_in(STILL_LIMIT_SECS)
        } else {
            self.media_frames(media)
        }
    }

    pub fn media_range(&self, media: &Media, c: &Clip) -> Range<i64> {
        let frames = self.media_limit(media);
        if !media.is_still() {
            return 0..frames;
        }
        let (lo, hi) = c.shown();
        let slack = frames - (hi - lo);
        lo - slack..hi + slack
    }

    pub fn source_range(&self, c: &Clip) -> Option<Range<i64>> {
        match &c.compound {
            Some(k) => Some(0..k.frames),
            None if c.is_generator() => None,
            None => self.media(&c.media).map(|m| self.media_range(m, c)),
        }
    }

    fn frames_in(&self, secs: f64) -> i64 {
        (secs * self.settings.fps.as_f64() + 0.05).floor().max(0.0) as i64
    }

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

    pub fn alloc_transition_id(&mut self) -> String {
        let id = format!("t{}", self.next_id.transition);
        self.next_id.transition += 1;
        id
    }

    pub fn alloc_cue_id(&mut self) -> String {
        let id = format!("q{}", self.next_id.cue);
        self.next_id.cue += 1;
        id
    }

    pub fn subtitle_index(&self, id: &str) -> Option<usize> {
        self.subtitles.iter().position(|t| t.id == id)
    }

    pub fn subtitle_track(&self, id: &str) -> Option<&SubtitleTrack> {
        self.subtitles.iter().find(|t| t.id == id)
    }

    pub fn find_cue(&self, id: &str) -> Option<(usize, usize)> {
        self.subtitles
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.cues.iter().position(|c| c.id == id).map(|ci| (ti, ci)))
    }

    pub fn cue(&self, id: &str) -> Option<&Cue> {
        self.find_cue(id).map(|(t, c)| &self.subtitles[t].cues[c])
    }
}

impl Track {
    pub fn new(id: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            id: id.into(),
            kind,
            clips: Vec::new(),
            locked: false,
            muted: false,
            solo: false,
            volume_db: 0.0,
            voice_isolation: None,
            duck: None,
            video_enabled: true,
            name: None,
            transitions: Vec::new(),
        }
    }

    pub fn has_sound(&self) -> bool {
        self.kind == TrackKind::Audio || self.clips.iter().any(|c| c.audio.is_some())
    }

    pub fn audible(&self, any_solo: bool) -> bool {
        !self.muted && (self.solo || !any_solo)
    }

    pub fn clip_covering(&self, t: i64) -> Option<usize> {
        let i = self.clips.partition_point(|c| c.start <= t);
        (i > 0 && t < self.clips[i - 1].end()).then(|| i - 1)
    }

    pub fn end(&self) -> i64 {
        self.clips.last().map_or(0, Clip::end)
    }

    pub fn clip_at(&self, t: i64) -> Option<usize> {
        self.clips.iter().position(|c| c.start <= t && t < c.end())
    }

    pub fn edit_points(&self) -> Vec<i64> {
        let mut points: Vec<i64> = std::iter::once(0)
            .chain(self.clips.iter().flat_map(|c| [c.start, c.end()]))
            .collect();
        points.sort_unstable();
        points.dedup();
        points
    }

    pub fn sort(&mut self) {
        self.clips.sort_by_key(|c| c.start);
    }
}

pub(crate) fn float_bits(v: f64) -> u64 {
    (v + 0.0).to_bits()
}

fn floats_bits(v: Option<f64>) -> Option<u64> {
    v.map(float_bits)
}

fn pair_bits(v: Option<[f64; 2]>) -> Option<[u64; 2]> {
    v.map(|[a, b]| [float_bits(a), float_bits(b)])
}

impl Hash for Title {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let Title {
            kind,
            align,
            text,
            style,
            position,
            zoom,
            rotation,
            opacity,
        } = self;
        (kind, align, text, style).hash(h);
        pair_bits(*position).hash(h);
        (
            floats_bits(*zoom),
            floats_bits(*rotation),
            floats_bits(*opacity),
        )
            .hash(h);
    }
}

impl Eq for Title {}

impl Hash for TitleStyle {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let TitleStyle {
            font,
            size,
            color,
            outline,
            background,
            shadow,
        } = self;
        (font, floats_bits(*size), color, outline, background, shadow).hash(h);
    }
}

impl Eq for TitleStyle {}

impl Hash for TitleShadow {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let TitleShadow {
            color,
            opacity,
            offset,
            blur,
        } = self;
        (
            color,
            floats_bits(*opacity),
            pair_bits(*offset),
            floats_bits(*blur),
        )
            .hash(h);
    }
}

impl Eq for TitleShadow {}

impl Hash for SubtitleStyle {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let SubtitleStyle {
            font,
            size,
            color,
            outline,
            background,
            margin,
            bold,
            case,
            shadow,
            words,
            speaker_color,
            align,
            position,
        } = self;
        (font, floats_bits(*size), color, outline, background).hash(h);
        (
            floats_bits(*margin),
            bold,
            case,
            shadow,
            words,
            speaker_color,
            align,
            pair_bits(*position),
        )
            .hash(h);
    }
}

impl Eq for SubtitleStyle {}

impl Hash for SubtitleOutline {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let SubtitleOutline { color, size } = self;
        (color, floats_bits(*size)).hash(h);
    }
}

impl Eq for SubtitleOutline {}

impl Hash for SubtitleBackground {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let SubtitleBackground { color, opacity } = self;
        (color, floats_bits(*opacity)).hash(h);
    }
}

impl Eq for SubtitleBackground {}

impl Hash for SubtitleWords {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let SubtitleWords { show, color, pop } = self;
        (show, color, floats_bits(*pop)).hash(h);
    }
}

impl Eq for SubtitleWords {}

#[cfg(test)]
mod tests {
    use super::{FRAME_PAD_MAX, ImageSequence, Kept, Settings, padded_frame};
    use crate::time::TimeSpec;

    #[test]
    fn kept_data_is_written_as_the_json_it_holds() {
        let kept = Kept::from(serde_json::json!({"OTIO_SCHEMA": "Transition.1"}));
        assert_eq!(
            serde_json::to_string(&kept).unwrap(),
            r#"{"OTIO_SCHEMA":"Transition.1"}"#
        );
    }

    #[test]
    fn a_huge_frame_pad_stays_within_the_cap() {
        assert_eq!(padded_frame(7, FRAME_PAD_MAX + 1_000).len(), FRAME_PAD_MAX);
        let s = ImageSequence {
            pattern: "%0999999999d.png".into(),
            first: 1,
            count: 1,
        };
        assert_eq!(s.name(1).len(), FRAME_PAD_MAX + ".png".len());
        assert_eq!(s.parts().1, FRAME_PAD_MAX);
    }

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
