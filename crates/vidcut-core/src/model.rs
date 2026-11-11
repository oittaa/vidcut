use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UnicodeSegmentation;

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
    /// The compound clips in the Media Pool. Left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compounds: Vec<PooledCompound>,
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
    /// Synced groups: pool items that line up whole files (`multicam.create`).
    /// Left out while there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<SyncGroup>,
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
    /// Record timecode counts drop-frame (decision D47): at 29.97, 59.94 and
    /// 119.88, and nothing at other rates.
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
    /// What the render cache makes for the viewer; left out when `off`.
    #[serde(default, skip_serializing_if = "RenderCache::is_off")]
    pub render_cache: RenderCache,
    /// The `.cube` file that the whole timeline goes through after its
    /// subtitles, before each output's colors. Left out when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_lut: Option<PathBuf>,
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
            render_cache: RenderCache::Off,
            output_lut: None,
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
    pub const MOST_CHANNELS: u16 = 24;

    pub fn with_format(fps: Option<&str>, size: Option<(u32, u32)>) -> crate::error::Result<Self> {
        let mut s = Self::default();
        if let Some(f) = fps {
            s.fps = f
                .parse()
                .map_err(|e: crate::time::TimeError| crate::error::Error::usage(e.to_string()))?;
            s.auto_format = false;
        }
        if let Some((w, h)) = size {
            (s.width, s.height) = timeline_size(w, h);
            s.auto_format = false;
        }
        Ok(s)
    }

    pub fn set_drop_frame(&mut self, on: bool) -> crate::error::Result<()> {
        if on && !self.fps.has_drop_frame() {
            return Err(crate::error::Error::invalid(format!(
                "drop-frame timecode is for 29.97, 59.94 and 119.88 fps, not {}",
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

pub const TIMELINE_SIDE: (u32, u32) = (256, 32768);

pub fn timeline_size(width: u32, height: u32) -> (u32, u32) {
    let (least, most) = TIMELINE_SIDE;
    if width < least || height < least {
        (least, least)
    } else if width > most || height > most {
        (most, most)
    } else {
        (width, height)
    }
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

/// A pool item that holds cameras of one moment: rows of files, lined up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SyncGroup {
    /// `g<N>`, never reused.
    pub id: String,
    pub name: String,
    /// The bin it is in (a path from [`Project::bins`]); empty: the top level.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bin: String,
    /// The timecode of its first frame, as a frame count from 00:00:00:00.
    pub start: i64,
    /// Its rows, in camera order. A file can be in more than one group. The
    /// newest group that holds it counts.
    pub rows: Vec<GroupRow>,
    /// The files that the row rule or a drag took off its rows. They are not
    /// in the group, and the Sync Clips icon does not count them as new files.
    /// Left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
}

impl SyncGroup {
    pub fn clips(&self) -> impl Iterator<Item = &GroupClip> {
        self.rows.iter().flat_map(|r| &r.clips)
    }

    pub fn at_of(&self, media: &str) -> Option<i64> {
        self.clips()
            .find(|c| c.media == media)
            .map(GroupClip::start)
    }
}

/// A row of a synced group. The row keeps its files when a camera name
/// changes later, until Auto Sync reads the names again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroupRow {
    /// The camera name of its files. Left out for a file with no camera name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// The sync lock: a drag or a nudge in the Sync Clips window does not
    /// move a file of this row.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// Its files, in time order. They do not overlap.
    pub clips: Vec<GroupClip>,
}

/// The visible part of a file on a row of a synced group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroupClip {
    pub media: String,
    /// The frame of the group at which the first visible frame lies.
    pub at: i64,
    /// The frames of far gaps that the group closes before this part. `at`
    /// plus `hidden` is the part's place in real time. Left out at 0.
    #[serde(default, skip_serializing_if = "is_zero_frames")]
    pub hidden: i64,
    /// The frames that the file loses at its start. Left out at 0.
    #[serde(default, skip_serializing_if = "is_zero_frames")]
    pub head: i64,
    /// The frames that the file loses at its end. Left out at 0.
    #[serde(default, skip_serializing_if = "is_zero_frames")]
    pub tail: i64,
}

impl GroupClip {
    pub fn start(&self) -> i64 {
        self.at - self.head
    }
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
    /// The name of the camera that recorded it: text, such as a letter or a
    /// number. Left out until `media.set` gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// Date Created, as `YYYY-MM-DD`. Left out until `media.set` gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// Its scene: text, such as a number. Left out until `media.set` gives
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// Its shot: text, such as a letter or a number. Left out until
    /// `media.set` gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot: Option<String>,
    /// Its take: text, such as a number. Left out until `media.set` gives
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<String>,
    /// A note on the clip. Left out until `media.set` gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
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
    /// The `.cube` file that its pictures go through just after their
    /// conversion into the timeline color space. Each of its clips on the
    /// timeline gets it. Left out when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_lut: Option<PathBuf>,
    /// How the alpha channel of its pictures is read. Left out while it is
    /// `straight`.
    #[serde(default, skip_serializing_if = "AlphaMode::is_straight")]
    pub alpha_mode: AlphaMode,
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
    /// For a subclip (`media.subclip`), the part of the file it is. Its
    /// source frames count from the start of that part. Left out for a
    /// whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subclip: Option<Subclip>,
    /// The markers on this source clip, at its source frames (`markers.add`
    /// with `media`), in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
}

/// The part of a file that a subclip is, in seconds from the file's first
/// frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Subclip {
    pub start: f64,
    pub end: f64,
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
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
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
    /// its first frame. Its weights (7.0 GB) download at their first use. The
    /// viewer and an export refuse a picture that it cannot take, where the
    /// picture needs enlarging. Off on macOS.
    AiSlow,
    /// A neural network (NanoVSR 5.4M) on the GPU, for SDR and HDR pictures.
    /// It reads 15 frames at a time. Its weights (25 MB) download at their
    /// first use. The upscaler of Auto elsewhere. Off on macOS.
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
    /// FlashVSR+ Turbo on the GPU: off on macOS.
    AiSlow,
    /// NanoVSR 5.4M on the GPU: off on macOS.
    AiFast,
    /// The classic upscalers.
    Classic,
}

impl MediaUpscaler {
    pub fn upscaler(self) -> Option<Upscaler> {
        match self {
            Self::Project => None,
            Self::Macos => Some(Upscaler::Macos),
            Self::AiSlow => Some(Upscaler::AiSlow),
            Self::AiFast => Some(Upscaler::AiFast),
            Self::Classic => Some(Upscaler::Classic),
        }
    }
}

/// What the render cache makes: the frames on disk that the viewer plays
/// in place of the frames that are slow to make. Exports do not use it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderCache {
    /// No render cache.
    #[default]
    Off,
    /// The parts that are slow to make: the clips set to `on`, the effects
    /// with `render_cache`, slow media, speed changes, Retime Processes, AI
    /// Super Scale, transitions, opacity and composite modes.
    Smart,
    /// The clips set to `on`, the effects with `render_cache`, and the
    /// kinds that the preferences turn on. A track with `render_cache` off
    /// gives none of its speed changes, Retime Processes, transitions,
    /// opacity and composite modes.
    User,
}

impl RenderCache {
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Smart => "Smart",
            Self::User => "User",
        }
    }
}

/// A clip's Render Cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClipCache {
    /// As the mode of the project gives.
    #[default]
    Auto,
    /// Rendered in the Smart mode and in the User mode.
    On,
    /// Its slow media and AI Super Scale are not rendered. Speed changes,
    /// Retime Processes, transitions, opacity and composite modes still
    /// are.
    Off,
}

impl ClipCache {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::On => "On",
            Self::Off => "Off",
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
    /// FlashVSR+ Turbo on the GPU: off on macOS.
    AiSlow,
    /// NanoVSR 5.4M on the GPU: off on macOS.
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
            Self::AiSlow => Upscaler::AiSlow,
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

/// How the angles of a multicam clip line up: by their start timecodes, so
/// that the In or the Out marks of the items meet, or by their sound.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AngleSync {
    #[default]
    Timecode,
    In,
    Out,
    Sound,
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

/// How the alpha channel of a picture is read, over the picture below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMode {
    /// The alpha is left out: the picture is opaque.
    None,
    /// Color + below x (1 - alpha), at most 1.
    Premultiplied,
    /// Below + (color - below) x alpha.
    #[default]
    Straight,
    /// Below + (color - below) x (1 - alpha).
    Invert,
}

impl AlphaMode {
    pub const ALL: [Self; 4] = [
        Self::None,
        Self::Premultiplied,
        Self::Straight,
        Self::Invert,
    ];

    pub fn is_straight(&self) -> bool {
        *self == Self::Straight
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Premultiplied => "Premultiplied",
            Self::Straight => "Straight",
            Self::Invert => "Invert",
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
    /// folder (preferences `playback_files.folder`), so the project names no
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
    pub fn opaque(&self) -> bool {
        self.info
            .video
            .as_ref()
            .is_some_and(|v| !v.alpha || self.alpha_mode == AlphaMode::None)
    }

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

    pub fn skipped(&self) -> f64 {
        self.subclip.map_or(0.0, |s| s.start)
    }

    pub fn origin(&self) -> f64 {
        let first = self
            .info
            .video
            .as_ref()
            .map(|v| v.first_pts)
            .or_else(|| self.info.audio.first().map(|a| a.first_pts))
            .unwrap_or(0.0);
        first + self.skipped()
    }

    pub fn timecode_at(&self, fps: Rational) -> f64 {
        let file = self.info.timecode.map_or(0.0, |tc| tc.frames_at(fps));
        file + self.skipped() * fps.as_f64()
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
    /// The mixer fader, in dB, from -100 to 10. -100 stands for the fader's
    /// bottom, minus infinity: the track is silent.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub volume_db: f64,
    /// Voice Isolation on the track's clips that have none of their own.
    /// Left out, it is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_isolation: Option<VoiceIsolation>,
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
    /// Off, the render cache's User mode renders none of its speed
    /// changes, Retime Processes, transitions, opacity and composite modes
    /// (video tracks only). Usual: on.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub render_cache: bool,
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
    /// The Duck Amount: how far it dips, in dB, from 0 to 18. 2.7 unless
    /// set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_db: Option<f64>,
    /// How long before a source starts the full dip is reached, in ms,
    /// from 0 to 500. 15 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookahead_ms: Option<f64>,
    /// The Rise Time: how long the gain takes to fall to the full dip, in
    /// ms, from 0 to 1000. 10 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rise_ms: Option<f64>,
    /// How long the full dip stays after a source stops, in ms, from 0 to
    /// 1000. A pause shorter than this keeps the dip. 150 unless set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_ms: Option<f64>,
    /// The Recovery Time: how long the gain takes to come back after the
    /// hold, in ms, from 0 to 1000. 750 unless set.
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PictureTransition {
    /// A cross dissolve.
    #[default]
    Dissolve,
    /// A dissolve through `color` (`#rrggbb`, black if left out): the
    /// outgoing picture fades to it, then the incoming picture fades in.
    DipToColor {
        #[serde(default, with = "hex_rgb")]
        #[schemars(with = "String")]
        color: [u8; 3],
    },
    /// One wipe, from `direction`.
    Wipe { direction: WipeDirection },
    /// The outgoing picture morphs into the incoming picture, which hides a
    /// jump cut.
    SmoothCut {
        #[serde(default, skip_serializing_if = "SmoothMode::is_better")]
        mode: SmoothMode,
        #[serde(default, skip_serializing_if = "SmoothProcess::is_ai")]
        process: SmoothProcess,
        #[serde(flatten)]
        course: Course,
    },
    /// The incoming picture pushes the outgoing picture out of the frame.
    Push {
        preset: PushPreset,
        #[serde(flatten)]
        motion: Motion,
    },
    /// The incoming picture slides in over the outgoing picture, or with
    /// `out` the outgoing picture slides out over the incoming picture.
    Slide {
        preset: SlidePreset,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        out: bool,
        #[serde(flatten)]
        motion: Motion,
    },
}

/// The way the pictures of a Push go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushPreset {
    /// The pictures move right: the incoming picture comes in from the left.
    Left,
    /// The pictures move left: the incoming picture comes in from the right.
    Right,
    /// The pictures move up: the incoming picture comes in from the bottom.
    Up,
    /// The pictures move down: the incoming picture comes in from the top.
    Down,
}

/// The edge or the corner that a Slide comes in from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlidePreset {
    /// From the left edge toward the right.
    LeftRight,
    /// From the right edge toward the left.
    RightLeft,
    /// From the bottom edge toward the top.
    BottomUp,
    /// From the top edge toward the bottom.
    TopDown,
    /// From the top-left corner toward the bottom-right.
    TopLeft,
    /// From the bottom-right corner toward the top-left.
    BottomRight,
}

impl PushPreset {
    pub const ALL: [PushPreset; 4] = [Self::Left, Self::Right, Self::Up, Self::Down];
}

impl SlidePreset {
    pub const ALL: [SlidePreset; 6] = [
        Self::LeftRight,
        Self::RightLeft,
        Self::BottomUp,
        Self::TopDown,
        Self::TopLeft,
        Self::BottomRight,
    ];
}

/// The values of a Push or a Slide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Motion {
    /// A band of `color` on the moving edge, as a part of the shorter side
    /// of the frame, from 0 to 0.25.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub border: f64,
    /// The band fades one picture into the other and shows no color.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub feather: bool,
    /// The color of the band, `#rrggbb`. White if left out.
    #[serde(default = "white", with = "hex_rgb", skip_serializing_if = "is_white")]
    #[schemars(with = "String")]
    pub color: [u8; 3],
    /// A blur along the move, from 0 to 1.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub motion_blur: f64,
    #[serde(flatten)]
    pub course: Course,
}

impl Motion {
    pub const PLAIN: Motion = Motion {
        border: 0.0,
        feather: false,
        color: WHITE,
        motion_blur: 0.0,
        course: Course::PLAIN,
    };
}

/// How a Push, a Slide or a Smooth Cut goes from the outgoing picture to
/// the incoming picture over its frames.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Course {
    /// How the transition speeds up and slows down.
    #[serde(default, skip_serializing_if = "MotionEase::is_none")]
    pub ease: MotionEase,
    /// With a `custom` ease, the keys of the transition: from 0, the
    /// outgoing picture, to 1, the incoming picture, at frames from the
    /// start of the transition, in order and no two on one frame. Without
    /// keys, the transition stays at `progress`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<Keyframe>,
    /// With a `custom` ease and no keys, the transition at each frame, from
    /// 0 to 1.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub progress: f64,
}

impl Course {
    pub const PLAIN: Course = Course {
        ease: MotionEase::None,
        curve: Vec::new(),
        progress: 0.0,
    };
}

/// Which frames a Smooth Cut morphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmoothMode {
    /// The outgoing picture stops at the first frame of the transition, and
    /// the incoming picture at its last frame. The morph goes from one still
    /// to the other.
    Faster,
    /// Both clips play on, as in a cross dissolve, and each frame morphs the
    /// outgoing frame into the incoming frame. Thus the motion of the
    /// subject stays.
    #[default]
    Better,
}

impl SmoothMode {
    pub const ALL: [SmoothMode; 2] = [Self::Faster, Self::Better];

    pub fn is_better(&self) -> bool {
        *self == Self::Better
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Faster => "Faster",
            Self::Better => "Better",
        }
    }
}

/// How a Smooth Cut makes each frame from the outgoing and the incoming
/// picture (deviation X99).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SmoothProcess {
    /// The AI of the computer that plays or exports: the frame rate
    /// conversion of macOS on macOS, AI Fast (RIFE 4.25) elsewhere.
    #[default]
    Ai,
    /// EMA-VFI on the GPU: slower, and nearer to the real frames. Off on
    /// macOS.
    AiSlow,
    /// The two pictures, mixed by how far the transition is.
    FrameBlend,
}

impl SmoothProcess {
    pub fn is_ai(&self) -> bool {
        *self == Self::Ai
    }

    pub fn here() -> &'static [SmoothProcess] {
        if Retime::ai_here() == Retime::Macos {
            &[Self::Ai, Self::FrameBlend]
        } else {
            &[Self::Ai, Self::AiSlow, Self::FrameBlend]
        }
    }

    pub fn retime(self) -> Retime {
        match self {
            Self::Ai => Retime::ai_here(),
            Self::AiSlow => Retime::AiSlow,
            Self::FrameBlend => Retime::FrameBlend,
        }
    }

    pub fn title(&self) -> &'static str {
        self.retime().title()
    }
}

impl Default for Motion {
    fn default() -> Self {
        Self::PLAIN
    }
}

pub const WHITE: [u8; 3] = [255, 255, 255];

pub const MAX_BORDER: f64 = 0.25;

fn white() -> [u8; 3] {
    WHITE
}

fn is_white(c: &[u8; 3]) -> bool {
    *c == WHITE
}

/// How a Push, a Slide or a Smooth Cut goes over its frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MotionEase {
    /// At one speed.
    #[default]
    None,
    /// Slow at the start.
    In,
    /// Slow at the end.
    Out,
    /// Slow at the start and at the end.
    InOut,
    /// As the keys of `curve` give.
    Custom,
}

impl MotionEase {
    pub const ALL: [MotionEase; 5] = [Self::None, Self::In, Self::Out, Self::InOut, Self::Custom];

    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}

pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

mod hex_rgb {
    pub fn serialize<S: serde::Serializer>(c: &[u8; 3], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::hex(*c))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[u8; 3], D::Error> {
        let text = <String as serde::Deserialize>::deserialize(d)?;
        crate::subtitles::rgb(&text).ok_or_else(|| serde::de::Error::custom("a color is #rrggbb"))
    }
}

impl PictureTransition {
    pub fn is_dissolve(&self) -> bool {
        matches!(self, Self::Dissolve)
    }

    pub fn motion(&self) -> Option<&Motion> {
        match self {
            Self::Push { motion, .. } | Self::Slide { motion, .. } => Some(motion),
            _ => None,
        }
    }

    pub fn motion_mut(&mut self) -> Option<&mut Motion> {
        match self {
            Self::Push { motion, .. } | Self::Slide { motion, .. } => Some(motion),
            _ => None,
        }
    }

    pub fn course(&self) -> Option<&Course> {
        match self {
            Self::SmoothCut { course, .. } => Some(course),
            k => k.motion().map(|m| &m.course),
        }
    }

    pub fn course_mut(&mut self) -> Option<&mut Course> {
        match self {
            Self::SmoothCut { course, .. } => Some(course),
            k => k.motion_mut().map(|m| &mut m.course),
        }
    }
}

/// The edge a wipe starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
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
    /// A generator: a picture made with no file. It has no
    /// media (`media` is empty) and no audio, and any length.
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
    /// A compound clip: made from timeline clips (`edit.compound`), or a
    /// compound clip or nested timeline from another editor's file (deviation
    /// X9). It is a block with no media and no audio of its own. On an audio
    /// track it stands for the sound of what it holds (`video` false).
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
    /// Its Render Cache: `on` renders it in both modes. `off` keeps its
    /// slow media and AI Super Scale out of both. Left out at `auto`.
    #[serde(default, skip_serializing_if = "ClipCache::is_auto")]
    pub render_cache: ClipCache,
    /// Its motion paths: where things in its picture are on its source
    /// frames. Only a clip of footage, a still or an image sequence has
    /// them. Left out when it has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub motion: Vec<MotionPath>,
}

/// A motion path of a clip: where one area of its picture is on its source
/// frames. A face, a number plate or another thing in the picture has one.
/// A path that `motion.track` made holds its place: before its first sample
/// and after its last, the area is where that sample puts it. Another path
/// lasts from its first sample to its last: before and after them it has no
/// area. Between two samples the area moves in a straight line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MotionPath {
    /// `p<N>`, its own among the paths of the clip.
    pub id: String,
    /// What it follows, for example `Face 1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The kind of thing that `motion.find` found for this path. Left out on
    /// a path that it did not make.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    /// On, `motion.track` made this path. A later track of the same area
    /// then writes its samples again. Left out when off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tracked: bool,
    /// The area on source frames: at least one sample, one for a frame, in
    /// the order of the frames.
    pub samples: Arc<Vec<PathSample>>,
}

/// The ways that `motion.track` goes from its first frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackWay {
    /// Backward and forward.
    #[default]
    Both,
    /// Only forward.
    Forward,
    /// Only backward.
    Backward,
}

impl TrackWay {
    pub fn backward(self) -> bool {
        self != Self::Forward
    }

    pub fn forward(self) -> bool {
        self != Self::Backward
    }
}

/// A kind of thing that `motion.find` finds in a picture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// The faces of people. A small neural network (YuNet) on the CPU finds
    /// them. Its file (233 kB) downloads at its first use.
    #[default]
    Face,
    /// Lines of text: signs, captions, documents and screens. A small
    /// neural network (PP-OCRv5) on the CPU finds them. Its file (4.8 MB)
    /// downloads at its first use.
    Text,
}

impl Subject {
    pub const ALL: [Self; 2] = [Self::Face, Self::Text];

    pub fn name(self) -> &'static str {
        match self {
            Self::Face => "face",
            Self::Text => "text",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Face => "Face",
            Self::Text => "Text",
        }
    }

    pub fn many(self) -> &'static str {
        match self {
            Self::Face => "Faces",
            Self::Text => "Text",
        }
    }

    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// The area of a motion path on one source frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PathSample {
    /// The source frame, counted as a clip's `source_in` is, at any speed of
    /// the clip.
    pub frame: i64,
    /// The middle of the area, `[x, y]` as fractions of the source picture
    /// from its top left, each from -10 to 10.
    pub center: [f64; 2],
    /// The width and the height of the area, as fractions of the source
    /// picture, each from 0 to 10. A point has no size: `[0, 0]`.
    pub size: [f64; 2],
    /// Degrees the area is turned counterclockwise, from -100000 to 100000.
    /// Usual: 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
)]
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
    AiSlow,
    /// The frame rate conversion of macOS (VideoToolbox) makes the frame
    /// from the motion between the two source frames. It is on macOS only,
    /// where the app calls it AI, and where AI Fast and AI Slow are off.
    Macos,
}

impl Retime {
    pub const ALL: [Retime; 5] = [
        Retime::Nearest,
        Retime::FrameBlend,
        Retime::AiFast,
        Retime::AiSlow,
        Retime::Macos,
    ];

    pub fn is_nearest(&self) -> bool {
        *self == Retime::Nearest
    }

    pub fn ai(&self) -> bool {
        matches!(self, Retime::AiFast | Retime::AiSlow | Retime::Macos)
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
            Retime::AiSlow => "AI Slow",
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
    AiSlow,
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
            Self::AiSlow => Some(Retime::AiSlow),
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
    AiSlow,
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
            Self::AiSlow => Some(Retime::AiSlow),
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
    pub const RANGE: std::ops::RangeInclusive<f64> = 0.0001..=1000.0;

    pub fn freezes(speed: f64) -> bool {
        (0.0..*Self::RANGE.start()).contains(&speed)
    }

    pub fn percent(speed: f64) -> f64 {
        let percent = speed * 100.0;
        let places = if percent.abs() < 1.0 { 100.0 } else { 1.0 };
        (percent * places).round() / places
    }

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

/// A kind of generator that vidcut draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeneratorKind {
    SolidColor,
    FourColorGradient,
    #[serde(rename = "10_step")]
    TenStep,
    #[serde(rename = "100mv_steps")]
    HundredMvSteps,
    GreyScale,
    YcbcrRamp,
    EbuColorBar,
    SmpteColorBar,
    Bt2111HlgNarrow,
    Bt2111PqFull,
    Bt2111PqNarrow,
    Window,
}

/// The Level of an EBU Color Bar: the percent of its colored bars, `75` or
/// `100`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
pub enum EbuLevel {
    #[default]
    #[serde(rename = "75")]
    Percent75,
    #[serde(rename = "100")]
    Percent100,
}

impl EbuLevel {
    pub const ALL: [Self; 2] = [Self::Percent75, Self::Percent100];
}

/// A picture that a rule draws from the size of the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Chart {
    #[serde(rename = "10_step")]
    TenStep,
    #[serde(rename = "100mv_steps")]
    HundredMvSteps,
    GreyScale,
    YcbcrRamp,
    EbuColorBar(EbuLevel),
    SmpteColorBar,
    Bt2111HlgNarrow,
    Bt2111PqFull,
    Bt2111PqNarrow,
    Window,
}

/// A Four Color Gradient: the colors of its corners and the center where they
/// meet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Gradient {
    /// Center X: -1 is the left edge, 0 the middle and 1 the right edge.
    /// `center_x_keys` replace it while they are there.
    pub center_x: f64,
    /// The keyframes of Center X, on the source frames of the clip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub center_x_keys: Vec<Keyframe>,
    /// Center Y: -1 is the top edge, 0 the middle and 1 the bottom edge.
    /// `center_y_keys` replace it while they are there.
    pub center_y: f64,
    /// The keyframes of Center Y, on the source frames of the clip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub center_y_keys: Vec<Keyframe>,
    /// The upper left color: `#rrggbb`.
    #[serde(with = "hex_rgb")]
    #[schemars(with = "String")]
    pub upper_left: [u8; 3],
    /// The upper right color: `#rrggbb`.
    #[serde(with = "hex_rgb")]
    #[schemars(with = "String")]
    pub upper_right: [u8; 3],
    /// The lower left color: `#rrggbb`.
    #[serde(with = "hex_rgb")]
    #[schemars(with = "String")]
    pub lower_left: [u8; 3],
    /// The lower right color: `#rrggbb`.
    #[serde(with = "hex_rgb")]
    #[schemars(with = "String")]
    pub lower_right: [u8; 3],
}

/// What a generator makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Generator {
    /// One color over the whole picture.
    SolidColor {
        /// `#rrggbb`.
        #[serde(with = "hex_rgb")]
        #[schemars(with = "String")]
        color: [u8; 3],
    },
    /// Four colors that mix from the corners of the picture.
    FourColorGradient(Gradient),
    /// Eleven grey bars, from black to white.
    #[serde(rename = "10_step")]
    TenStep,
    /// Eight grey steps, from black to white.
    #[serde(rename = "100mv_steps")]
    HundredMvSteps,
    /// A grey ramp, from black at the left to white at the right.
    GreyScale,
    /// A ramp in each color channel, across the picture.
    YcbcrRamp,
    /// Eight bars: white, six colors at the `level`, and black.
    EbuColorBar { level: EbuLevel },
    /// Seven bars at 75%, with a row of reversed bars, a ramp and a row with
    /// white and near-black patches under them.
    SmpteColorBar,
    /// The BT.2111 bars, in the narrow-range code values of HLG.
    Bt2111HlgNarrow,
    /// The BT.2111 bars, in the full-range code values of PQ.
    Bt2111PqFull,
    /// The BT.2111 bars, in the narrow-range code values of PQ.
    Bt2111PqNarrow,
    /// A white box over the middle half of the width and the height, on black.
    Window,
    /// A generator, a title or a clip with no media from another editor's
    /// file that vidcut doesn't draw (deviation X9): a block that plays as a
    /// gap. On an audio track, a clip with no media stands for its sound
    /// (`video` false).
    Carried {
        /// The OTIO media reference as read: a `GeneratorReference`, or a
        /// `MissingReference` for a clip with no media. The `vidcut` metadata
        /// of a `MissingReference` holds the `channels` of its sound and the
        /// `link` group of the file.
        reference: Kept,
    },
}

impl Generator {
    pub const SOLID_COLOR: &str = "Solid Color";
    pub const FOUR_COLOR_GRADIENT: &str = "Four Color Gradient";
    pub const TEN_STEP: &str = "10 Step";
    pub const HUNDRED_MV_STEPS: &str = "100mV Steps";
    pub const GREY_SCALE: &str = "Grey Scale";
    pub const YCBCR_RAMP: &str = "YCbCr Ramp";
    pub const EBU_COLOR_BAR: &str = "EBU Color Bar";
    pub const SMPTE_COLOR_BAR: &str = "SMPTE Color Bar";
    pub const BT2111_HLG_NARROW: &str = "BT.2111 Color Bar HLG Narrow";
    pub const BT2111_PQ_FULL: &str = "BT.2111 Color Bar PQ Full";
    pub const BT2111_PQ_NARROW: &str = "BT.2111 Color Bar PQ Narrow";
    pub const WINDOW: &str = "Window";

    pub fn draws(&self) -> bool {
        !matches!(self, Self::Carried { .. })
    }

    pub fn chart(&self) -> Option<Chart> {
        match self {
            Self::TenStep => Some(Chart::TenStep),
            Self::HundredMvSteps => Some(Chart::HundredMvSteps),
            Self::GreyScale => Some(Chart::GreyScale),
            Self::YcbcrRamp => Some(Chart::YcbcrRamp),
            Self::EbuColorBar { level } => Some(Chart::EbuColorBar(*level)),
            Self::SmpteColorBar => Some(Chart::SmpteColorBar),
            Self::Bt2111HlgNarrow => Some(Chart::Bt2111HlgNarrow),
            Self::Bt2111PqFull => Some(Chart::Bt2111PqFull),
            Self::Bt2111PqNarrow => Some(Chart::Bt2111PqNarrow),
            Self::Window => Some(Chart::Window),
            Self::SolidColor { .. } | Self::FourColorGradient(_) | Self::Carried { .. } => None,
        }
    }

    pub fn is_offline(&self) -> bool {
        matches!(self, Self::Carried { reference }
            if reference["OTIO_SCHEMA"]
                .as_str()
                .is_some_and(|s| s.starts_with("MissingReference")))
    }
}

/// The motion path that a title follows: a path of another clip of the same
/// timeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Follow {
    /// The clip that has the path, for example `c3`.
    pub clip: String,
    /// The id of the path on that clip, for example `p2`.
    pub path: String,
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
    /// How it lines up: `left`, `middle` or `right`. On a `text` title, this
    /// sets each line in the block, which is as wide as its widest line.
    /// Usual: middle for a `text` title, left for a lower third and a scroll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TitleAlign>,
    /// The point that goes to its `position`: `top_left`, `top`, `top_right`,
    /// `left`, `middle`, `right`, `bottom_left`, `bottom` or `bottom_right`.
    /// On a `text` title, a point of its block. Usual: `middle`. On a lower
    /// third, a point of its lower line. Usual: the end of the line's top edge
    /// by `align`. On a scroll, a point of a text box as wide as the picture.
    /// Usual: `middle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<TitleAnchor>,
    /// Its lines, separated by `\n`: two at most for a lower third. `<b>`,
    /// `<i>`, `<u>`, `<o>` (a line over), `<s>` (a line through), `<sup>`,
    /// `<sub>` and `<font color="…" face="…" size="…" spacing="…"
    /// tracking="…" case="…">` style the words between them apart from the
    /// rest, `size` a fraction of the picture height like the style's, `spacing`
    /// the line spacing of their line, `tracking` and `case` like the style's;
    /// `&lt;`, `&gt;` and `&amp;` stand for `<`, `>` and `&`.
    pub text: String,
    #[serde(default, skip_serializing_if = "TitleStyle::is_empty")]
    pub style: TitleStyle,
    /// Where it goes, as fractions of the picture's width and height from
    /// its top left, from -4 to 4 across and from -3 to 5 down: the `anchor`
    /// of a `text` title's block (usual: `[0.5, 0.5]`), the `anchor` of a lower
    /// third's lower line (usual: 10%, 50% or 90% across by its `align` and
    /// 78% down: `[0.1, 0.78]`, `[0.5, 0.78]` and `[0.9, 0.78]`). A scroll's
    /// `x` is where its lines' left edge, middle or right edge goes by its
    /// `align` (usual: 1%, 50% or 99% across), with an `anchor` in the middle
    /// column; its `y` moves only a `shape`. With `position_keys`, the place
    /// at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    /// Position keyframes, one for X and Y, on the clip's source frames.
    /// `value` is X. `position_y` is Y, one number for each keyframe, in the
    /// same order. Their own `in_handle` and `out_handle` stay empty:
    /// `position_handles` bends the path. Left out when the position has no
    /// keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_keys: Vec<Keyframe>,
    /// Y of each `position_keys` entry. Left out with those keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_y: Vec<f64>,
    /// The handles of the `position_keys` that have any, in order of frame.
    /// `keyframes.set` with `ease` or an OTIO import gives them. A scroll has
    /// none. Left out when no keyframe has a handle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_handles: Vec<PositionHandles>,
    /// How much larger its text is drawn, from 1/4 to 4, about the `position`
    /// of a `text` title or a lower third's line; a scroll has none. Usual: 1.
    /// With `zoom_keys`, the zoom at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    /// Zoom keyframes, on the clip's source frames. Left out when the zoom
    /// has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zoom_keys: Vec<Keyframe>,
    /// Degrees it is turned counterclockwise, with its background and
    /// shadow, about the same point as `zoom`. Usual: 0. With
    /// `rotation_keys`, the angle at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// Rotation keyframes, on the clip's source frames. Left out when the
    /// rotation has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotation_keys: Vec<Keyframe>,
    /// How opaque the whole title is, from 0 to 1. Usual: 1. With
    /// `opacity_keys`, the opacity at a frame is that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Opacity keyframes, on the clip's source frames. Left out when the
    /// opacity has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opacity_keys: Vec<Keyframe>,
    /// A lower third's upper line, when it goes apart from the lower line.
    /// `position`, `zoom` and `rotation` and their keyframes above are then
    /// the lower line's alone. `anchor` above is always the lower line's. Left
    /// out when the two lines go together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upper: Option<TitleLine>,
    /// The motion path that it follows (`motion.follow`). Each `position` of
    /// the title then counts from the middle of the thing on that path:
    /// `[0.5, 0.5]` puts the `anchor` on that middle. Before the first sample
    /// of the path and after its last, the thing is where that sample has it.
    /// Before and after the clip of the path, it is where the first or the
    /// last frame of that clip has it. A scroll follows no path. Left out
    /// when it follows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows: Option<Follow>,
}

/// One line of a lower third: `upper` or `lower`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LowerThirdLine {
    Upper,
    Lower,
}

/// The place, anchor, zoom and rotation of a lower third's upper line. The
/// fields work as the title's fields with the same names. `position` is this
/// line's `anchor`, and `zoom` and `rotation` work about it. The usual `anchor`
/// is the left, middle or right end of this line's bottom edge, by the title's
/// `align`. With no `upper`, this point is 2% of the picture height below the
/// lower line's point.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TitleLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<TitleAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_keys: Vec<Keyframe>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_y: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub position_handles: Vec<PositionHandles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zoom_keys: Vec<Keyframe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotation_keys: Vec<Keyframe>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlePart {
    Whole,
    Shape,
    Backing,
    Upper,
    Lower,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitleBlock {
    pub part: TitlePart,
    pub anchor: [f64; 2],
    pub pivot: [f64; 2],
    pub edge: f64,
    pub zoom: f64,
    pub turn: f64,
}

/// The handles of one position keyframe of a title, as fractions of the
/// picture from that keyframe, `[x, y]`. Between two keyframes the title
/// moves on a cubic path. Its inner points are the first keyframe plus its
/// `out_handle` and the second keyframe plus its `in_handle`. The title moves
/// at one speed along the path, with the length in fractions of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PositionHandles {
    /// The source frame of the keyframe in `position_keys`.
    pub frame: i64,
    /// The handle toward the keyframe before it. Left out when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_handle: Option<[f64; 2]>,
    /// The handle toward the keyframe after it. Left out when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_handle: Option<[f64; 2]>,
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

/// How a title's lines line up. It also sets where a lower third sits at the
/// bottom of the title-safe area, and a scroll across the picture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TitleAlign {
    #[default]
    Left,
    Middle,
    Right,
}

impl TitleAlign {
    pub fn fraction(self) -> f64 {
        match self {
            Self::Left => 0.0,
            Self::Middle => 0.5,
            Self::Right => 1.0,
        }
    }
}

/// The point of a title's or a subtitle's text that goes to its position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TitleAnchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Middle,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl TitleAnchor {
    pub fn fractions(self) -> [f64; 2] {
        let across = match self {
            Self::TopLeft | Self::Left | Self::BottomLeft => 0.0,
            Self::Top | Self::Middle | Self::Bottom => 0.5,
            Self::TopRight | Self::Right | Self::BottomRight => 1.0,
        };
        let down = match self {
            Self::TopLeft | Self::Top | Self::TopRight => 0.0,
            Self::Left | Self::Middle | Self::Right => 0.5,
            Self::BottomLeft | Self::Bottom | Self::BottomRight => 1.0,
        };
        [across, down]
    }

    pub fn at([across, down]: [f64; 2]) -> Self {
        const ALL: [[TitleAnchor; 3]; 3] = [
            [
                TitleAnchor::TopLeft,
                TitleAnchor::Top,
                TitleAnchor::TopRight,
            ],
            [TitleAnchor::Left, TitleAnchor::Middle, TitleAnchor::Right],
            [
                TitleAnchor::BottomLeft,
                TitleAnchor::Bottom,
                TitleAnchor::BottomRight,
            ],
        ];
        let index = |v: f64| ((v * 2.0).round() as usize).min(2);
        ALL[index(down)][index(across)]
    }
}

/// How a title looks. A field left out keeps the usual look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct TitleStyle {
    /// Font family, used when that font is installed. Usual: the burned-in
    /// subtitles' font.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Text height as a fraction of the picture height, from 1/1080 to
    /// 4096/1080. Usual: 96/1080 for `text`, 108/1080 for `scroll`, and for a
    /// `lower_third` 76/1080 on its lower line and 48/1080 on its upper line.
    /// A size set here goes on both lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Pixels of the timeline added below each line, from -4096 to 4096.
    /// Below the last line, it only makes the block taller. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_spacing: Option<f64>,
    /// Space after each letter, from -4096 to 4096. One unit is 1/100 of the
    /// width of the font's letter X, and 1/200 below 0. Usual: 0, and 11 on
    /// the upper line of a lower third.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking: Option<f64>,
    /// How its letters draw: `mixed` (as typed), `upper`, `lower`, `small`
    /// (small letters as small capitals) or `title` (each word starts with a
    /// capital). Usual: `mixed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<TitleCase>,
    /// `#rrggbb`, `#rgb` or a name such as `yellow`. Usual: white.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Outline color and width. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<SubtitleOutline>,
    /// A box behind the whole block of text. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<SubtitleBackground>,
    /// The size and place of the `background`, which it needs for its color
    /// and opacity. Usual: a box that fits the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<TitleShape>,
    /// A soft copy of the text behind it, over its background. An offset of
    /// 0 draws nothing. Usual: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<TitleShadow>,
}

/// A title's background at its own size and place. It keeps them for any
/// text and any zoom. It moves and turns with the title's `position` and
/// `rotation`, a lower third's with its lower line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TitleShape {
    /// Its width and height, as fractions of the picture's width and height,
    /// from 0 to 2. A side of 0 draws no box.
    pub size: [f64; 2],
    /// Its middle from the title's `position`, as fractions of the picture,
    /// `[x, y]` to the right and down, from -4 to 4. Usual: `[0, 0]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    /// The radius of its corners, as a fraction of the picture's height, from
    /// 0 to 1. No radius passes half of a side. Usual: 1/27.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner: Option<f64>,
    /// The color of its outline: `#rrggbb`, `#rgb` or a name. It is not
    /// changed by the background's opacity. Usual: black.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline_color: Option<String>,
    /// The width of its outline in pixels of the timeline, from 0 to 30. Half
    /// of it is outside the box. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline_width: Option<f64>,
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
    /// picture's width and height, each from -4 to 4. Usual: `[0, 0]`, which
    /// draws no shadow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<[f64; 2]>,
    /// How soft its edge is, from 1 to 100, where the blur's radius is
    /// about 3% of the picture height. Usual: 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    /// `true` draws no shadow and keeps these values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<bool>,
}

impl TitleShadow {
    pub const OPACITY: f64 = 0.75;
    pub const OFFSET: [f64; 2] = [0.0, 0.0];
    pub const BLUR: f64 = 20.0;
    pub const BLUR_PER_PICTURE: f64 = 3370.0;

    pub fn is_off(&self) -> bool {
        self.off == Some(true)
    }

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

pub fn size_fits(v: f64, most: u32) -> bool {
    v.is_finite() && (1.0 / 1080.0..=f64::from(most) / 1080.0).contains(&v)
}

impl Title {
    pub const MOST_SIZE: u32 = 4096;
    pub const MOST_SPACING: f64 = 4096.0;
    pub const TRACKING_SHARE: f64 = 100.0;
    pub const TRACKING_SHARE_LESS: f64 = 200.0;
    pub const TRACKING_FILE: f64 = 0.85;
    pub const PIXEL_STEPS: f64 = 64.0;
    pub const SHIFTED_SHARE: [f64; 2] = [2.0, 3.0];
    pub const WIDTH_SLACK: f64 = 1e-3;
    pub const BOX_MOST: f64 = 2.0;
    pub const BOX_WIDTH: f64 = 0.9;
    pub const BOX_WHOLE: f64 = 1e-6;
    pub const BOX_CORNER: f64 = 1.0 / 27.0;
    pub const BOX_MOST_OUTLINE: f64 = 30.0;
    pub const LINE: f64 = 1.31;
    pub const PADDING: f64 = 0.15;
    pub const SAFE: f64 = 0.9;
    pub const LOWER_IN: f64 = 0.1;
    pub const LOWER_DOWN: f64 = 0.78;
    pub const UPPER_DROP: f64 = 0.02;
    pub const SCROLL_IN: f64 = 0.01;
    pub const SCROLL_LINE: f64 = 4.0 / 3.0;
    pub const SCROLL_LEAD: f64 = 10.0;
    pub const UPPER_SIZE: f64 = 48.0 / 1080.0;
    pub const UPPER_TRACKING: f64 = 11.0;
    pub const ACROSS: (f64, f64) = (-Picture::REACH, Picture::REACH);
    pub const DOWN: (f64, f64) = (1.0 - Picture::REACH, 1.0 + Picture::REACH);

    pub fn spacing_fits(v: f64) -> bool {
        v.is_finite() && v.abs() <= Self::MOST_SPACING
    }

    pub fn places([x, y]: [f64; 2]) -> bool {
        (Self::ACROSS.0..=Self::ACROSS.1).contains(&x) && (Self::DOWN.0..=Self::DOWN.1).contains(&y)
    }

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            kind: TitleKind::Text,
            align: None,
            anchor: None,
            text: text.into(),
            style: TitleStyle::default(),
            position: None,
            position_keys: Vec::new(),
            position_y: Vec::new(),
            position_handles: Vec::new(),
            zoom: None,
            zoom_keys: Vec::new(),
            rotation: None,
            rotation_keys: Vec::new(),
            opacity: None,
            opacity_keys: Vec::new(),
            upper: None,
            follows: None,
        }
    }

    pub fn align(&self) -> TitleAlign {
        match self.kind {
            TitleKind::Text => self.align.unwrap_or(TitleAlign::Middle),
            TitleKind::LowerThird | TitleKind::Scroll => self.align.unwrap_or_default(),
        }
    }

    pub fn usual_position(kind: TitleKind, align: TitleAlign) -> [f64; 2] {
        match (kind, align) {
            (TitleKind::Text, _) => [0.5, 0.5],
            (TitleKind::LowerThird, TitleAlign::Left) => [Self::LOWER_IN, Self::LOWER_DOWN],
            (TitleKind::LowerThird, TitleAlign::Middle) => [0.5, Self::LOWER_DOWN],
            (TitleKind::LowerThird, TitleAlign::Right) => [1.0 - Self::LOWER_IN, Self::LOWER_DOWN],
            (TitleKind::Scroll, TitleAlign::Left) => [Self::SCROLL_IN, 0.5],
            (TitleKind::Scroll, TitleAlign::Middle) => [0.5, 0.5],
            (TitleKind::Scroll, TitleAlign::Right) => [1.0 - Self::SCROLL_IN, 0.5],
        }
    }

    pub fn place(&self) -> [f64; 2] {
        self.position
            .unwrap_or_else(|| Self::usual_position(self.kind, self.align()))
    }

    pub fn usual_anchor(kind: TitleKind, align: TitleAlign, part: TitlePart) -> TitleAnchor {
        match (kind, part, align) {
            (TitleKind::LowerThird, TitlePart::Upper, TitleAlign::Left) => TitleAnchor::BottomLeft,
            (TitleKind::LowerThird, TitlePart::Upper, TitleAlign::Middle) => TitleAnchor::Bottom,
            (TitleKind::LowerThird, TitlePart::Upper, TitleAlign::Right) => {
                TitleAnchor::BottomRight
            }
            (TitleKind::LowerThird, _, TitleAlign::Left) => TitleAnchor::TopLeft,
            (TitleKind::LowerThird, _, TitleAlign::Middle) => TitleAnchor::Top,
            (TitleKind::LowerThird, _, TitleAlign::Right) => TitleAnchor::TopRight,
            (TitleKind::Text | TitleKind::Scroll, _, _) => TitleAnchor::Middle,
        }
    }

    pub fn anchor_of(&self, part: TitlePart) -> [f64; 2] {
        self.anchor
            .unwrap_or_else(|| Self::usual_anchor(self.kind, self.align(), part))
            .fractions()
    }

    pub fn usual_size(kind: TitleKind) -> f64 {
        match kind {
            TitleKind::Text => 96.0 / 1080.0,
            TitleKind::Scroll => 108.0 / 1080.0,
            TitleKind::LowerThird => 76.0 / 1080.0,
        }
    }

    fn usual_upper(&self, text: &str) -> String {
        let size = match self.style.size {
            Some(_) => String::new(),
            None => format!(" size=\"{}\"", Self::UPPER_SIZE),
        };
        let tracking = match self.style.tracking {
            Some(_) => String::new(),
            None => format!(" tracking=\"{}\"", Self::UPPER_TRACKING),
        };
        if size.is_empty() && tracking.is_empty() {
            text.to_string()
        } else {
            format!("<font{size}{tracking}>{text}</font>")
        }
    }

    pub fn sized_text(&self) -> std::borrow::Cow<'_, str> {
        match (self.kind, self.text.split_once('\n')) {
            (TitleKind::LowerThird, Some((upper, lower))) => {
                format!("{}\n{lower}", self.usual_upper(upper)).into()
            }
            _ => self.text.as_str().into(),
        }
    }

    pub fn moves(&self) -> bool {
        self.kind == TitleKind::Scroll
    }

    pub fn text_fraction(&self) -> f64 {
        self.style.size.unwrap_or(Self::usual_size(self.kind))
    }

    fn tracking_share(tracking: f64) -> f64 {
        if tracking < 0.0 {
            Self::TRACKING_SHARE_LESS
        } else {
            Self::TRACKING_SHARE
        }
    }

    pub fn tracking_pixels(tracking: f64, letter: f64) -> f64 {
        let pixels = tracking * letter / Self::tracking_share(tracking);
        (pixels * Self::PIXEL_STEPS).trunc() / Self::PIXEL_STEPS
    }

    pub fn tracking_from_file(pixels: f64) -> f64 {
        pixels * Self::tracking_share(pixels) / (Self::TRACKING_SHARE * Self::TRACKING_FILE)
    }

    pub fn tracking_to_file(tracking: f64) -> f64 {
        tracking * Self::TRACKING_SHARE * Self::TRACKING_FILE / Self::tracking_share(tracking)
    }

    pub fn whole_size(text: f64, scale: f64) -> f64 {
        (text / scale + Self::WIDTH_SLACK).round().max(1.0) * scale
    }

    pub fn shifted_size(text: f64, scale: f64) -> f64 {
        let [part, whole] = Self::SHIFTED_SHARE;
        ((text / scale).round() * part / whole).floor().max(1.0) * scale
    }

    pub fn small_size(text: f64, scale: f64) -> f64 {
        ((text / scale).round() * TitleCase::SMALL_CAPS + Self::WIDTH_SLACK)
            .round()
            .max(1.0)
            * scale
    }

    pub fn underline_drop(thickness: f64) -> f64 {
        (thickness / 3.0 + 0.5).ceil() + 0.5
    }

    pub fn line_end(from_block: f64, scale: f64) -> f64 {
        (from_block / scale + Self::WIDTH_SLACK).floor() * scale
    }

    pub fn seated_left(&self, across: f64, middle: f64, widest: f64, wrap: f64, pixel: f64) -> f64 {
        let lined = self.align().fraction();
        let width = (widest / pixel + Self::WIDTH_SLACK).floor();
        ((middle / pixel).round() - across * width) * pixel + lined * (widest - wrap)
    }

    pub fn line_gap(&self, heights: [f64; 3], text: f64, pixel: f64, zoom: f64) -> f64 {
        match self.kind {
            TitleKind::Scroll => {
                (heights[0] + heights[1]) * Self::SCROLL_LINE * text + Self::SCROLL_LEAD * pixel
            }
            TitleKind::Text | TitleKind::LowerThird => {
                let size = (text / zoom / pixel).round();
                let line: f64 = heights
                    .iter()
                    .map(|part| (part * size * Self::PIXEL_STEPS).floor() / Self::PIXEL_STEPS)
                    .sum();
                line.ceil() * zoom * pixel
            }
        }
    }

    pub fn ink(&self) -> [u8; 3] {
        paint(self.style.color.as_deref(), [255, 255, 255])
    }

    pub fn stroke(&self) -> Option<TextStroke> {
        stroke_of(self.style.outline.as_ref(), 0.0, [0, 0, 0])
    }

    pub fn backing(&self) -> Option<([u8; 3], f64)> {
        backing_of(self.style.background.as_ref())
    }

    pub fn shape(&self) -> Option<&TitleShape> {
        self.backing().and(self.style.shape.as_ref())
    }

    pub fn shape_pixels(&self, pivot: [f64; 2], (width, height): (f64, f64)) -> Option<[f64; 4]> {
        let s = self.shape()?;
        let [ox, oy] = s.center.unwrap_or([0.0, 0.0]);
        let across = (s.size[0] * width + Self::BOX_WHOLE).floor();
        let down = (s.size[1] * height + Self::BOX_WHOLE).floor();
        let left = ((pivot[0] + ox) * width - across / 2.0 + 0.5).floor();
        let top = ((pivot[1] + oy) * height - down / 2.0 + 0.5).floor();
        Some([left, top, across, down])
    }

    pub fn shape_past_edge(&self, (width, height): (u32, u32)) -> bool {
        let (w, h) = (f64::from(width), f64::from(height));
        let pivot = self.block(TitlePart::Shape).pivot;
        let Some([left, top, across, down]) = self.shape_pixels(pivot, (w, h)) else {
            return false;
        };
        let ring = self.shape_outline().1 / 2.0;
        left - ring < 0.0 || top - ring < 0.0 || left + across + ring > w || top + down + ring > h
    }

    pub fn shape_outline(&self) -> ([u8; 3], f64) {
        let s = self.style.shape.as_ref();
        (
            paint(s.and_then(|s| s.outline_color.as_deref()), [0, 0, 0]),
            s.and_then(|s| s.outline_width).unwrap_or(0.0),
        )
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

    pub fn lower_line(&self) -> f64 {
        let lines = crate::rich::lines(&self.text);
        let base = self.text_fraction();
        let size = lines
            .last()
            .map_or(base, |line| crate::rich::largest(line, base));
        size * Self::LINE
    }

    pub fn line(&self) -> TitleLine {
        TitleLine {
            anchor: self.anchor,
            position: self.position,
            position_keys: self.position_keys.clone(),
            position_y: self.position_y.clone(),
            position_handles: self.position_handles.clone(),
            zoom: self.zoom,
            zoom_keys: self.zoom_keys.clone(),
            rotation: self.rotation,
            rotation_keys: self.rotation_keys.clone(),
        }
    }

    pub fn upper_with_lower(&self) -> TitleLine {
        let dropped = |y: f64| crate::otio::effects::rounded(y + Self::UPPER_DROP);
        let [x, y] = self.place();
        TitleLine {
            anchor: None,
            position: Some([x, dropped(y)]),
            position_y: self.position_y.iter().map(|y| dropped(*y)).collect(),
            ..self.line()
        }
    }

    pub fn part_lines(&mut self) {
        if self.upper.is_none() {
            self.upper = Some(self.upper_with_lower());
        }
    }

    pub fn swap_lines(&mut self) {
        let Some(mut u) = self.upper.take() else {
            return;
        };
        std::mem::swap(&mut self.anchor, &mut u.anchor);
        std::mem::swap(&mut self.position, &mut u.position);
        std::mem::swap(&mut self.position_keys, &mut u.position_keys);
        std::mem::swap(&mut self.position_y, &mut u.position_y);
        std::mem::swap(&mut self.position_handles, &mut u.position_handles);
        std::mem::swap(&mut self.zoom, &mut u.zoom);
        std::mem::swap(&mut self.zoom_keys, &mut u.zoom_keys);
        std::mem::swap(&mut self.rotation, &mut u.rotation);
        std::mem::swap(&mut self.rotation_keys, &mut u.rotation_keys);
        self.upper = Some(u);
    }

    pub fn upper_view(&self) -> Title {
        let mut t = self.clone();
        t.part_lines();
        t.swap_lines();
        let [x, y] = Self::usual_position(self.kind, self.align());
        t.position.get_or_insert([x, y + Self::UPPER_DROP]);
        t
    }

    pub fn moved(&mut self, [dx, dy]: [f64; 2]) {
        let [x, y] = self.place();
        let [ux, uy] = Self::usual_position(self.kind, self.align());
        self.position = Some([x + dx, y + dy]);
        self.position_keys.iter_mut().for_each(|k| k.value += dx);
        self.position_y.iter_mut().for_each(|v| *v += dy);
        if let Some(upper) = self.upper.as_mut() {
            let [x, y] = upper.position.unwrap_or([ux, uy + Self::UPPER_DROP]);
            upper.position = Some([x + dx, y + dy]);
            upper.position_keys.iter_mut().for_each(|k| k.value += dx);
            upper.position_y.iter_mut().for_each(|v| *v += dy);
        }
    }

    pub fn each_line(&mut self, mut change: impl FnMut(&mut Title)) {
        change(self);
        if self.upper.is_some() {
            self.swap_lines();
            change(self);
            self.swap_lines();
        }
    }

    fn block(&self, part: TitlePart) -> TitleBlock {
        let place = self.place();
        TitleBlock {
            part,
            anchor: self.anchor_of(part),
            pivot: if part == TitlePart::Shape && self.kind == TitleKind::Scroll {
                [place[0] + 0.5 - self.align().fraction(), place[1]]
            } else {
                place
            },
            edge: if self.kind == TitleKind::Scroll {
                1.0
            } else {
                place[1]
            },
            zoom: if part == TitlePart::Shape {
                1.0
            } else {
                self.scale()
            },
            turn: self.turn(),
        }
    }

    pub fn blocks(&self) -> Vec<TitleBlock> {
        let mut out = Vec::new();
        if self.shape().is_some() {
            out.push(self.block(TitlePart::Shape));
        }
        if self.kind != TitleKind::LowerThird {
            out.push(self.block(TitlePart::Whole));
            return out;
        }
        if self.backing().is_some() && self.shape().is_none() {
            out.push(self.block(TitlePart::Backing));
        }
        out.push(self.block(TitlePart::Lower));
        out.push(self.upper_view().block(TitlePart::Upper));
        out
    }

    pub fn block_text(&self, part: TitlePart) -> std::borrow::Cow<'_, str> {
        let lines = crate::rich::lines(&self.text);
        let none = crate::rich::Look::default();
        match part {
            TitlePart::Whole | TitlePart::Backing | TitlePart::Shape => self.sized_text(),
            TitlePart::Lower => lines
                .last()
                .map_or(String::new(), |l| crate::rich::write(l, &none))
                .into(),
            TitlePart::Upper if lines.len() < 2 => "".into(),
            TitlePart::Upper => self
                .usual_upper(&crate::rich::write(&lines[0], &none))
                .into(),
        }
    }

    pub fn label(&self) -> String {
        let text = crate::rich::plain(&self.text);
        let line = text.lines().find(|l| !l.trim().is_empty());
        line.map_or_else(|| "Title".to_string(), |l| l.trim().to_string())
    }
}

/// Where a clip's picture goes over the frame, and the effects on it:
/// footage, a still, an image sequence, a compound clip or a generator,
/// which has no effects. A field left out is at its usual value; at all of
/// them the picture is fitted into the frame. The tracks below show wherever
/// it doesn't cover the frame: in the bars around a picture of another shape,
/// around it, where it is cropped and through its soft edges and its opacity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Picture {
    /// Where the middle of the whole picture goes before the zoom and the
    /// rotation, `[x, y]` as fractions of the frame from its top left, each
    /// from -3.5 to 4.5. With an anchor away from the middle, the zoom and
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
    /// y]`, each from 0 to 100. At 0, it does not show. Usual: `[1, 1]`.
    /// With `zoom_keys`, the zoom at a frame is that curve.
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
    /// Degrees it is turned counterclockwise about its anchor, from -100000
    /// to 100000. Usual: 0. With `rotation_keys`, the angle at a frame is
    /// that curve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// Rotation keyframes, in degrees (decision D75). Left out when the
    /// rotation has no keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotation_keys: Vec<Keyframe>,
    /// The point of the picture that the zoom and the rotation work about,
    /// `[x, y]` as fractions of the picture from its top left, as it shows
    /// after the flips, each from -3.5 to 4.5. A Dynamic Zoom does not move
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
    /// Turned off, the position, zoom, rotation, anchor, pitch, yaw and
    /// flips stay on the clip and do nothing. Usual: on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transform_off: bool,
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
    /// Turned off, the crop and its softness stay on the clip and do
    /// nothing. Usual: on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crop_off: bool,
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
    /// The effects on the picture, in the order they work. Left out when it
    /// has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

/// An effect on a clip's picture. It works on the picture as its transform
/// and its crop put it in the frame, with the clear area around it, before
/// its composite mode and its opacity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Effect {
    /// Off, the effect stays on the clip and does nothing. Usual: on.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub on: bool,
    #[serde(flatten)]
    pub kind: EffectKind,
    /// How much of the effect shows, from 0 (the picture as it comes in)
    /// to 1 (all of the effect). Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<f64>,
    /// On, the effect also works on the picture's transparency. Off, the
    /// picture keeps its own shape, and the clear area around it counts as
    /// black. Usual: on.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub use_alpha: bool,
    /// On, the render cache renders the frames of the effect, in its Smart
    /// mode and in its User mode. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub render_cache: bool,
    /// The keyframes of its values, on source frames. A value with keyframes
    /// follows them, and its plain value returns when the last one goes.
    /// Left out when it has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<EffectKeys>,
    /// Where the effect works. Left out, it works on the whole picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<EffectArea>,
}

/// The part of a picture that an effect works in: an ellipse or a box inside
/// a base box. The base box is the whole source picture, or with `follows`
/// the area of a motion path of the clip at each frame. The area stays on
/// the content of the picture through its transform, its crop and its flips.
/// A field left out is at its usual value. Its `center`, `size`, `rotation`
/// and `softness` can have keyframes, in the `keys` of the effect. Each of
/// them then follows its keyframes at a frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EffectArea {
    /// Its shape. Usual: `ellipse`.
    #[serde(default, skip_serializing_if = "AreaShape::is_ellipse")]
    pub shape: AreaShape,
    /// Its middle, `[x, y]` as fractions of the base box from its top left,
    /// each from -10 to 10. Usual: `[0.5, 0.5]`, the middle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    /// Its width and height as fractions of the base box, each from 0 to
    /// 10. Usual: `[1, 1]`, as wide and as high as the base box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 2]>,
    /// Degrees it is turned counterclockwise on the source picture, from
    /// -100000 to 100000. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// How soft its edge is, from 0 (a hard edge) to 1: the share of the way
    /// from its edge to its middle over which the effect fades in. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub softness: Option<f64>,
    /// On, the effect works outside the area. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invert: bool,
    /// The corners of a `polygon`, in their order: each `[x, y]` as
    /// fractions of the area's own box from its top left, each from -10 to
    /// 10, from 3 to 128 of them. Left out, the four corners of that box.
    /// Another shape does not use them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f64; 2]>,
    /// The id of a motion path of the same clip, as `p1`, or a subject, as
    /// `face`: then each path of the clip that `motion.find` made for that
    /// subject. The base box is then the area of a path at each frame,
    /// turned as the path turns it: with a `size` of `[1.2, 1.2]`, the area
    /// is the path's box, a fifth larger. With several paths, the effect
    /// works in the area of each. It works in none before a path starts and
    /// after it ends. Left out, the base box is the whole source picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows: Option<String>,
}

/// The shape of the area of an effect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AreaShape {
    /// The ellipse that touches the four sides of the box.
    #[default]
    Ellipse,
    /// The box itself.
    Box,
    /// A shape with straight sides through the `points` of the area, in
    /// their order.
    Polygon,
}

impl AreaShape {
    pub fn is_ellipse(&self) -> bool {
        *self == Self::Ellipse
    }
}

impl EffectArea {
    pub const MIDDLE: [f64; 2] = [0.5, 0.5];
    pub const WHOLE: [f64; 2] = [1.0, 1.0];
    pub const CORNERS: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    pub const POINTS_KEPT: usize = 128;
    pub const PLACES: (f64, f64) = (-crate::motion::REACH, crate::motion::REACH);
    pub const SIZES: (f64, f64) = (0.0, crate::motion::REACH);
    pub const TURNS: (f64, f64) = (-crate::motion::TURN, crate::motion::TURN);
    pub const SHARES: (f64, f64) = (0.0, 1.0);

    pub fn corners(&self) -> &[[f64; 2]] {
        if self.points.is_empty() {
            &Self::CORNERS
        } else {
            &self.points
        }
    }

    pub fn middle(&self) -> [f64; 2] {
        self.center.unwrap_or(Self::MIDDLE)
    }

    pub fn extent(&self) -> [f64; 2] {
        self.size.unwrap_or(Self::WHOLE)
    }

    pub fn turn(&self) -> f64 {
        self.rotation.unwrap_or(0.0)
    }

    pub fn soft(&self) -> f64 {
        self.softness.unwrap_or(0.0)
    }
}

/// The keyframes of one value of an effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EffectKeys {
    pub value: EffectValue,
    pub keys: Vec<Keyframe>,
}

/// A value of an effect that can have keyframes. A color or a list of
/// channels has one value for each of its numbers, and a position or a center
/// has one for x and one for y. `softness`, `opacity` and `saturation` are
/// values of more than one kind of effect. The six `area_` values are the
/// center, the size, the rotation and the softness of the area of an effect.
/// An effect of any kind has them while it has an area.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EffectValue {
    Blend,
    Strength,
    VerticalStrength,
    HorizontalRed,
    HorizontalGreen,
    HorizontalBlue,
    HorizontalAlpha,
    VerticalRed,
    VerticalGreen,
    VerticalBlue,
    VerticalAlpha,
    PixelFrequency,
    SmoothStrength,
    Aliasing,
    XOffset,
    YOffset,
    Aspect,
    PositionX,
    PositionY,
    Threshold,
    AlphaThreshold,
    Spread,
    HvRatio,
    RelativeSpreadRed,
    RelativeSpreadGreen,
    RelativeSpreadBlue,
    Gain,
    Gamma,
    ColorFilterRed,
    ColorFilterGreen,
    ColorFilterBlue,
    SecondaryGain,
    SecondarySpread,
    SecondaryColorRed,
    SecondaryColorGreen,
    SecondaryColorBlue,
    AtmosphereStrength,
    AtmosphereDetail,
    AtmosphereScale,
    AtmosphereRatio,
    AtmosphereEvolution,
    AtmosphereSeethe,
    Recovery,
    Opacity,
    Texture,
    GrainSize,
    GrainAspect,
    GrainStrength,
    Offset,
    Symmetry,
    Softness,
    Saturation,
    DetailLoss,
    ChannelGainRed,
    ChannelGainGreen,
    ChannelGainBlue,
    Shadows,
    Midtones,
    Highlights,
    Size,
    Rotation,
    Anamorphism,
    CenterX,
    CenterY,
    Transparency,
    ColorRed,
    ColorGreen,
    ColorBlue,
    SharpenAmount,
    FineDetailSize,
    FineDetails,
    MediumDetails,
    LargeDetails,
    SharpenChroma,
    AreaCenterX,
    AreaCenterY,
    AreaWidth,
    AreaHeight,
    AreaRotation,
    AreaSoftness,
}

/// What an effect does, with its own values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectKind {
    /// A Gaussian blur across and down the frame.
    GaussianBlur(GaussianBlur),
    /// The picture as cells of one color each.
    MosaicBlur(MosaicBlur),
    /// A soft light around the bright parts of the picture.
    Glow(Box<Glow>),
    /// Noise with the look of film over the picture.
    FilmGrain(Box<FilmGrain>),
    /// A soft shape with the picture inside it and a color around it.
    Vignette(Vignette),
    /// Makes the detail of the picture stronger.
    Sharpen(Sharpen),
}

/// The effects there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectType {
    GaussianBlur,
    MosaicBlur,
    Glow,
    FilmGrain,
    Vignette,
    Sharpen,
}

/// A Gaussian blur. Its spread (the sigma of its curve) is 0.05625 × the
/// strength² × the frame's width, times the zoom of the picture and of the
/// compound clips around it. A channel value multiplies it for that channel
/// and that way. It works on the code values, across and down the frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GaussianBlur {
    /// How strong the blur is, from 0 (none) to 1; with `split`, across.
    /// Usual: 0.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<f64>,
    /// Across and down at strengths of their own: `strength` across and
    /// `vertical_strength` down. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub split: bool,
    /// With `split`, how strong the blur is down, from 0 to 1. Usual: 0.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_strength: Option<f64>,
    /// The blur across on red, green, blue and alpha, each a multiple of
    /// its spread from 0 (none) to 2. Usual: `[1, 1, 1, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizontal_channels: Option<[f64; 4]>,
    /// The blur down on red, green, blue and alpha, each a multiple of its
    /// spread from 0 (none) to 2. Usual: `[1, 1, 1, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_channels: Option<[f64; 4]>,
    /// What the blur takes from beyond the frame's edges. Usual: `reflect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
}

/// A mosaic: the frame in cells, each of one color. A cell is 2 × √(the
/// picture's width × its height) / `pixel_frequency` high, and `aspect` times
/// that wide. The picture's size is its size on the frame: its fit, times the
/// zoom of the picture and of the compound clips around it. A cell's color is
/// the mean of a box around its middle, `aliasing` cells across and down,
/// which repeats the frame's edge pixels beyond the frame. It works on the
/// code values, after the transform and the crop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MosaicBlur {
    /// How many cells there are: from 0.1 (few, large) to 4000. Usual: 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_frequency: Option<f64>,
    /// From 0 to 1. It does not change the picture. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth_strength: Option<f64>,
    /// The shape of the cells. Usual: `square`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_shape: Option<CellShape>,
    /// The size of the box that makes a cell's color, in cells, from 0 (the
    /// one pixel at its middle) to 2. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliasing: Option<f64>,
    /// Moves the cells to the right by this many half cells, from -1 to 1.
    /// Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_offset: Option<f64>,
    /// Moves the cells up by this many half cells, from -1 to 1. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_offset: Option<f64>,
    /// A cell's width over its height, from 0.001 to 1000. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f64>,
    /// The point the cells line up on, `[x, y]` as fractions of the picture
    /// on the frame from its bottom left, each from -1000000 to 1000000.
    /// Usual: `[0.5, 0.5]`, the picture's middle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    /// It does not change the picture: the box always repeats the edge
    /// pixels. Usual: `reflect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    /// Works at twice the size and then takes the mean of each 2 × 2, for
    /// clean lines between the cells. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub antialias: bool,
    /// On, a cell also takes the mean of the alpha. Off, the picture keeps
    /// its own alpha and the cells keep their colors, so a clear part adds
    /// its cell's color to the tracks below. Usual: on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_alpha: Option<bool>,
    /// With `blur_alpha` on, only a cell with no clear part in its box stays,
    /// with an alpha of 1. Elsewhere the alpha is 0, and the picture's own
    /// colors add to the tracks below. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole_cells: bool,
}

/// A vignette: a soft shape, with the picture inside it and `color` around
/// it. The shape's radius is 0.7 × (`size` + 0.01) × the picture's height. Its
/// softness is three box blurs, on cells 2 pixels apart. The picture's size is
/// its fit in the frame, before its zoom and its position. They do not move
/// the shape. It works on the code values, after the transform and the crop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Vignette {
    /// `basic` is an oval in the middle, with `color` in the `normal` mode.
    /// It uses only `size`, `softness`, `anamorphism` and `color`. `advanced`
    /// uses all the values. Usual: `basic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operating_mode: Option<VignetteMode>,
    /// The shape, in `advanced`. Usual: `oval`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_shape: Option<VignetteShape>,
    /// How large the shape is, from 0.01 to 2. Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Turns the shape counter-clockwise, in `advanced`, in degrees from
    /// -360 to 360. It comes before `anamorphism`. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// How much wider than high the shape is, from 0.2 to 5. Usual: 1.78.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anamorphism: Option<f64>,
    /// The middle of the shape, in `advanced`, `[x, y]` as fractions of the
    /// picture from its bottom left, each from -1000000 to 1000000. Usual:
    /// `[0.5, 0.5]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    /// How soft the edge of the shape is, from 0 to 1. Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub softness: Option<f64>,
    /// How much of the picture shows through the color, in `advanced`, from
    /// 0 (none) to 1 (all). Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparency: Option<f64>,
    /// The color around the shape, `[r, g, b]` in code values. Usual: black,
    /// `[0, 0, 0]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f64; 3]>,
    /// How the color mixes with the picture, in `advanced`. The formulas are
    /// those of a clip's composite mode, with these differences: nothing
    /// stops the result of `color_dodge`, `color_burn`, `divide` and
    /// `vivid_light` at 0 or 1 before the mix, `divide` adds 0.000001 to the
    /// color, `linear_burn` stops at 0, `lighter_color` and `darker_color`
    /// take the color on a tie, and `hue` with black or white takes the hue
    /// of red and with another gray one fixed hue. Usual: `multiply`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composite_type: Option<CompositeMode>,
}

/// A sharpen: it makes three bands of detail stronger, from fine to large. A
/// band is the difference between two smoothings that keep strong edges. It
/// works on the code values, after the transform and the crop, in pixels of
/// the frame. No value grows with the frame's size. The alpha does not change,
/// and nothing stops the colors at 0 or 1 before the output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Sharpen {
    /// How strong the detail of the brightness is, from 1 (no change) to 5.
    /// Usual: 1.8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharpen_amount: Option<f64>,
    /// How large each band's detail is, from 0.01 to 0.1. Usual: 0.05.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_detail_size: Option<f64>,
    /// Multiplies the strength of the fine band, from 1 to 5. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_details: Option<f64>,
    /// Multiplies the strength of the medium band, from 1 to 5. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium_details: Option<f64>,
    /// Multiplies the strength of the large band, from 1 to 5. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large_details: Option<f64>,
    /// How strong the detail of the colors is, from 1 to 5. Above 1 it
    /// multiplies the three Details values, and `sharpen_amount` does not
    /// change it. At 1 the colors keep their detail. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharpen_chroma: Option<f64>,
}

/// Which values of a vignette work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VignetteMode {
    /// Only its size, softness, anamorphism and color.
    #[default]
    Basic,
    /// All of them.
    Advanced,
}

impl VignetteMode {
    pub const ALL: [VignetteMode; 2] = [VignetteMode::Basic, VignetteMode::Advanced];
}

/// The shape of a vignette.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VignetteShape {
    /// An oval.
    #[default]
    Oval,
    /// A triangle with a flat top and its point down.
    Triangle,
    /// A square on its corner.
    Diamond,
    /// A rectangle.
    Rectangle,
    /// A hexagon, flat at the top and the bottom.
    Hexagon,
    /// An octagon.
    Octagon,
}

impl VignetteShape {
    pub const ALL: [VignetteShape; 6] = [
        VignetteShape::Oval,
        VignetteShape::Triangle,
        VignetteShape::Diamond,
        VignetteShape::Rectangle,
        VignetteShape::Hexagon,
        VignetteShape::Octagon,
    ];

    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as u32
    }
}

/// The shape of a mosaic's cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CellShape {
    /// Rows and columns of rectangles. An edge of a cell is on the point the
    /// cells line up on.
    #[default]
    Square,
    /// Hexagons, flat at the top and the bottom. One has its middle on the
    /// point the cells line up on.
    Hexagon,
    /// Triangles in rows, pointing up and down in turn.
    Triangle,
}

impl CellShape {
    pub const ALL: [CellShape; 3] = [CellShape::Square, CellShape::Hexagon, CellShape::Triangle];

    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as u32
    }
}

/// What a blur takes from beyond the frame's edges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Border {
    /// Nothing: black and clear.
    Black,
    /// The pixels of the edge, repeated.
    Replicate,
    /// The picture, mirrored about the pixels of the edge.
    #[default]
    Reflect,
    /// The picture from the other side.
    WrapAround,
}

impl Border {
    pub const ALL: [Border; 4] = [
        Border::Black,
        Border::Replicate,
        Border::Reflect,
        Border::WrapAround,
    ];

    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|b| *b == self).unwrap_or(2) as u32
    }
}

/// A glow: the bright parts of the picture, blurred and put over it. The
/// blur's sigma is 0.146667 × `spread` × √(the picture's width × its height,
/// on the frame). It works on the code values, after the transform and the
/// crop, in the frame's pixels.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Glow {
    /// What the effect gives. Usual: `glowing_image`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<GlowOutput>,
    /// How the picture's alpha acts. Usual: `masks_sources`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_alpha: Option<GlowAlpha>,
    /// The code value where a channel starts to shine, from 0 to 1. Usual:
    /// 0.75.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// With `input_alpha` `is_source`, the alpha where the picture starts to
    /// shine, from 0 to 1. Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha_threshold: Option<f64>,
    /// How wide the glow is, from 0 to 3. Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<f64>,
    /// Multiplies the spread across by 1 + this and down by 1 − this, from
    /// -1 to 1. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hv_ratio: Option<f64>,
    /// The spread on red, green and blue, each a multiple from 0 to 2. Usual:
    /// `[1, 1, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_spread: Option<[f64; 3]>,
    /// How bright the glow is, from 0 to 10. Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f64>,
    /// The gamma of the glow, from 0.1 to 10. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma: Option<f64>,
    /// The colorfulness of the glow, from 0 to 10. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    /// Multiplies the glow's red, green and blue, each from 0 to 1000000.
    /// Usual: `[1, 1, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_filter: Option<[f64; 3]>,
    /// Adds a second glow with its own gain, spread and color. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secondary: bool,
    /// How bright the second glow is, from 0 to 10. Usual: 0.75.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_gain: Option<f64>,
    /// How wide the second glow is, from 0 to 3. Its sigma is 3 times that of
    /// a `spread` of the same value, and `spread` does not change it. Usual:
    /// 0.25.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_spread: Option<f64>,
    /// Multiplies the second glow's red, green and blue, each from 0 to
    /// 1000000. Usual: `[1, 1, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_color: Option<[f64; 3]>,
    /// Breaks up the glow with soft noise. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub atmosphere: bool,
    /// How much the noise changes the glow, from -5 to 5. Usual: 0.25.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere_strength: Option<f64>,
    /// How much fine noise there is, from 0 to 1. Usual: 0.55.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere_detail: Option<f64>,
    /// The size of the noise, from 0.01 to 100. Usual: 0.6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere_scale: Option<f64>,
    /// Multiplies the noise's size across by 1 + this and down by 1 − this,
    /// from -0.99 to 0.99. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere_ratio: Option<f64>,
    /// Another pattern of the noise, from 0 to 1000. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere_evolution: Option<f64>,
    /// How fast the noise changes from frame to frame, from 0 to 10. Usual:
    /// 0.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere_seethe: Option<f64>,
    /// It does not change the picture. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub atmosphere_tracked: bool,
    /// What the blur takes from beyond the frame. Usual: `reflect_in_camera`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framing: Option<GlowFraming>,
    /// How the glow goes over the picture. Usual: `add`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composite_type: Option<CompositeMode>,
    /// How much of the composite shows, from 0 to 1. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Works on the picture's negative, and gives the negative of the result.
    /// Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invert: bool,
    /// Takes the glow away where the picture shines, from 0 to 5. Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<f64>,
}

/// What a glow gives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GlowOutput {
    /// The parts of the picture that shine, not blurred.
    ShinyRegions,
    /// The glow alone.
    GlowAlone,
    /// The glow over the picture.
    #[default]
    GlowingImage,
}

impl GlowOutput {
    pub const ALL: [GlowOutput; 3] = [
        GlowOutput::ShinyRegions,
        GlowOutput::GlowAlone,
        GlowOutput::GlowingImage,
    ];

    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|o| *o == self).unwrap_or(2) as u32
    }
}

/// How a glow uses the picture's alpha.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GlowAlpha {
    /// The alpha multiplies what shines, and the result is opaque.
    #[default]
    MasksSources,
    /// The same picture as `masks_sources`.
    Usual,
    /// The alpha itself shines, in gray, and the result is opaque.
    IsSource,
}

impl GlowAlpha {
    pub const ALL: [GlowAlpha; 3] = [
        GlowAlpha::MasksSources,
        GlowAlpha::Usual,
        GlowAlpha::IsSource,
    ];

    pub fn code(self) -> u32 {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0) as u32
    }
}

/// What a glow's blur takes from beyond the frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GlowFraming {
    /// Nothing. The glow also fades in the last pixels of the frame.
    OnlyAsSeen,
    /// The picture, mirrored just outside the frame.
    #[default]
    ReflectInCamera,
}

impl GlowFraming {
    pub const ALL: [GlowFraming; 2] = [GlowFraming::OnlyAsSeen, GlowFraming::ReflectInCamera];
}

/// Film grain: noise with the spectrum of a film stock, mixed with the
/// picture. It stays in the frame's pixels, so the transform does not move it.
/// It works on the code values, after the transform and the crop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FilmGrain {
    /// How the grain goes over the picture. Usual: `overlay`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain_composite: Option<GrainComposite>,
    /// How much grain shows, from 0 to 1. It scales the grain and mixes the
    /// composite, so the grain goes with its square. Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Shows the grain alone, with the picture's alpha. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub grain_only: bool,
    /// The grain of the clip's first frame on each frame. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub freeze: bool,
    /// The pattern of the grain, from 0 to 1: a higher value has a narrower
    /// band of sizes. Usual: 0.704.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<f64>,
    /// How large the grain is, from 0 to 1. Usual: 0.66.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain_size: Option<f64>,
    /// The width of the grain over its height, from 0.2 to 5. Usual: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain_aspect: Option<f64>,
    /// How strong the grain is, from 0 to 1. Usual: 0.14875.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain_strength: Option<f64>,
    /// The code value of the grain with `grain_only`, from 0 to 1. The grain
    /// on a picture changes only where it reaches 0 or 1. Usual: 0.482.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
    /// Moves the grain to the dark side, under 0.5, or to the bright side,
    /// from 0 to 1. Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symmetry: Option<f64>,
    /// How soft the grain is, from 0 to 1. Its sigma is 5 × this^1.5 × the
    /// frame's height / 1080 pixels. Usual: 0.2984.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub softness: Option<f64>,
    /// How much the grain differs on red, green and blue, from 0 (gray) to 1.
    /// Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>,
    /// Blurs the picture under the grain, from 0 to 2. Its sigma is 4.32 ×
    /// this² × √(width × height / (1920 × 1080)) pixels of the frame.
    /// Usual: 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_loss: Option<f64>,
    /// Multiplies the grain on red, green and blue, each from -10 to 10.
    /// Usual: `[1, 1, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_gains: Option<[f64; 3]>,
    /// How much grain the dark parts of the picture take, from 0 to 1. Usual:
    /// 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadows: Option<f64>,
    /// How much grain the middle tones of the picture take, from 0 to 1.
    /// Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midtones: Option<f64>,
    /// How much grain the bright parts of the picture take, from 0 to 1.
    /// Usual: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlights: Option<f64>,
    /// On, another pattern with the same look. Usual: off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub animate: bool,
}

/// How a Film Grain goes over the picture. Each value but `custom` is the
/// composite mode of its name, with the grain as `a` and the picture as `b`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrainComposite {
    #[default]
    Overlay,
    Multiply,
    Darken,
    Lighten,
    Screen,
    Subtract,
    Hardlight,
    Softlight,
    LinearLight,
    VividLight,
    PinLight,
    /// As `linear_light`.
    Custom,
}

impl GrainComposite {
    pub const ALL: [GrainComposite; 12] = [
        Self::Overlay,
        Self::Multiply,
        Self::Darken,
        Self::Lighten,
        Self::Screen,
        Self::Subtract,
        Self::Hardlight,
        Self::Softlight,
        Self::LinearLight,
        Self::VividLight,
        Self::PinLight,
        Self::Custom,
    ];

    pub fn mode(self) -> CompositeMode {
        match self {
            Self::Overlay => CompositeMode::Overlay,
            Self::Multiply => CompositeMode::Multiply,
            Self::Darken => CompositeMode::Darken,
            Self::Lighten => CompositeMode::Lighten,
            Self::Screen => CompositeMode::Screen,
            Self::Subtract => CompositeMode::Subtract,
            Self::Hardlight => CompositeMode::Hardlight,
            Self::Softlight => CompositeMode::Softlight,
            Self::LinearLight | Self::Custom => CompositeMode::LinearLight,
            Self::VividLight => CompositeMode::VividLight,
            Self::PinLight => CompositeMode::PinLight,
        }
    }
}

/// A film stock of a Film Grain: a set of its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum GrainPreset {
    #[serde(rename = "8mm_s1")]
    S1At8,
    #[serde(rename = "8mm_50d")]
    D50At8,
    #[serde(rename = "8mm_100d")]
    D100At8,
    #[serde(rename = "8mm_250d")]
    D250At8,
    #[serde(rename = "8mm_200t")]
    T200At8,
    #[serde(rename = "8mm_500t")]
    T500At8,
    #[serde(rename = "16mm_s1")]
    S1At16,
    #[serde(rename = "16mm_50d")]
    D50At16,
    #[serde(rename = "16mm_250d")]
    D250At16,
    #[serde(rename = "16mm_500t")]
    T500At16,
    #[serde(rename = "16mm_archival")]
    ArchivalAt16,
    #[serde(rename = "16mm_archival_print")]
    ArchivalPrintAt16,
    #[serde(rename = "16mm_reversal")]
    ReversalAt16,
    #[serde(rename = "35mm_s1")]
    S1At35,
    #[serde(rename = "35mm_200t")]
    T200At35,
    #[serde(rename = "35mm_400t")]
    T400At35,
    #[serde(rename = "35mm_reversal_bw")]
    ReversalBwAt35,
    /// Values of no film stock.
    #[serde(rename = "custom")]
    Custom,
}

impl GrainPreset {
    pub const ALL: [GrainPreset; 18] = [
        Self::S1At8,
        Self::D50At8,
        Self::D100At8,
        Self::D250At8,
        Self::T200At8,
        Self::T500At8,
        Self::S1At16,
        Self::D50At16,
        Self::D250At16,
        Self::T500At16,
        Self::ArchivalAt16,
        Self::ArchivalPrintAt16,
        Self::ReversalAt16,
        Self::S1At35,
        Self::T200At35,
        Self::T400At35,
        Self::ReversalBwAt35,
        Self::Custom,
    ];

    fn values(self) -> Option<[f64; 6]> {
        let usual = [
            FilmGrain::TEXTURE,
            FilmGrain::SIZE,
            FilmGrain::STRENGTH,
            FilmGrain::OFFSET,
            FilmGrain::SYMMETRY,
            FilmGrain::SOFTNESS,
        ];
        match self {
            Self::S1At8 => Some([
                0.613_684_237_003_326_4,
                0.682_000_041_007_995_6,
                0.307_500_004_768_371_6,
                0.483_999_997_377_395_63,
                0.5,
                0.659_025_967_121_124_3,
            ]),
            Self::D50At8 => Some([
                0.682_297_348_976_135_3,
                0.693_000_018_596_649_2,
                0.170_000_001_788_139_34,
                0.515_999_972_820_282,
                0.5,
                0.215_443_462_133_407_6,
            ]),
            Self::D100At8 => Some([
                0.794_900_536_537_170_4,
                0.215_599_954_128_265_38,
                1.0,
                0.545_000_016_689_300_5,
                0.5,
                0.321_154_654_026_031_5,
            ]),
            Self::D250At8 => Some([
                0.727_819_561_958_313,
                0.807_399_988_174_438_5,
                0.272_499_978_542_327_9,
                0.5,
                0.5,
                0.265_754_342_079_162_6,
            ]),
            Self::T200At8 => Some([
                0.771_309_494_972_229,
                0.638_000_011_444_091_8,
                0.317_499_995_231_628_4,
                0.5,
                0.5,
                0.269_619_941_711_425_8,
            ]),
            Self::T500At8 => Some([
                0.761_538_505_554_199_2,
                0.728_200_018_405_914_3,
                1.0,
                0.398_000_001_907_348_63,
                0.800_000_011_920_929,
                0.341_995_179_653_167_7,
            ]),
            Self::S1At16 => Some(usual),
            Self::D50At16 => Some([
                0.703_740_179_538_726_8,
                0.261_799_991_130_828_86,
                0.170_000_001_788_139_34,
                0.5,
                0.5,
                0.218_306_541_442_871_1,
            ]),
            Self::D250At16 => Some([
                0.726_035_058_498_382_6,
                0.409_200_012_683_868_4,
                0.216_249_987_483_024_6,
                0.476_999_998_092_651_37,
                0.512_000_024_318_695_1,
                0.166_599_079_966_545_1,
            ]),
            Self::T500At16 => Some([
                0.704_328_417_778_015_1,
                0.362_999_975_681_304_93,
                0.237_499_997_019_767_76,
                0.495_999_991_893_768_3,
                0.5,
                0.202_314_764_261_245_73,
            ]),
            Self::ArchivalAt16 => Some([
                0.646_250_009_536_743_2,
                0.307_999_968_528_747_56,
                0.166_250_005_364_418_03,
                0.504_999_995_231_628_4,
                0.5,
                0.202_314_764_261_245_73,
            ]),
            Self::ArchivalPrintAt16 => Some([
                0.682_380_318_641_662_6,
                0.272_800_028_324_127_2,
                0.341_250_002_384_185_8,
                0.492_000_013_589_859,
                0.5,
                0.233_732_417_225_837_7,
            ]),
            Self::ReversalAt16 => Some([
                0.726_079_106_330_871_6,
                0.488_400_042_057_037_35,
                0.397_499_978_542_327_9,
                0.414_000_004_529_953,
                0.942_999_958_992_004_4,
                0.218_306_541_442_871_1,
            ]),
            Self::S1At35 => Some([
                0.783_991_873_264_312_7,
                0.262_019_991_874_694_8,
                0.158_749_997_615_814_2,
                0.485_000_014_305_114_75,
                0.5,
                0.219_731_062_650_680_54,
            ]),
            Self::T200At35 => Some([
                0.726_426_959_037_780_8,
                0.136_400_043_964_386,
                0.173_749_998_211_860_66,
                0.501_999_974_250_793_5,
                0.5,
                0.233_732_417_225_837_7,
            ]),
            Self::T400At35 => Some([
                0.750_200_033_187_866_2,
                0.0,
                0.283_749_997_615_814_2,
                0.5,
                0.5,
                0.233_732_417_225_837_7,
            ]),
            Self::ReversalBwAt35 => Some([
                0.501_734_733_581_543,
                0.021_999_955_177_307_13,
                0.263_749_986_886_978_15,
                0.514_999_985_694_885_3,
                0.5,
                0.218_306_541_442_871_1,
            ]),
            Self::Custom => None,
        }
    }
}

impl EffectType {
    pub const ALL: [EffectType; 6] = [
        EffectType::GaussianBlur,
        EffectType::MosaicBlur,
        EffectType::Glow,
        EffectType::FilmGrain,
        EffectType::Vignette,
        EffectType::Sharpen,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::GaussianBlur => "Gaussian Blur",
            Self::MosaicBlur => "Mosaic Blur",
            Self::Glow => "Glow",
            Self::FilmGrain => "Film Grain",
            Self::Vignette => "Vignette",
            Self::Sharpen => "Sharpen",
        }
    }

    pub fn made(self) -> Effect {
        let kind = match self {
            Self::GaussianBlur => EffectKind::GaussianBlur(GaussianBlur::default()),
            Self::MosaicBlur => EffectKind::MosaicBlur(MosaicBlur::default()),
            Self::Glow => EffectKind::Glow(Box::default()),
            Self::FilmGrain => EffectKind::FilmGrain(Box::default()),
            Self::Vignette => EffectKind::Vignette(Vignette::default()),
            Self::Sharpen => EffectKind::Sharpen(Sharpen::default()),
        };
        Effect {
            on: true,
            kind,
            blend: None,
            use_alpha: true,
            render_cache: false,
            keys: Vec::new(),
            area: None,
        }
    }
}

impl Effect {
    pub fn kind(&self) -> EffectType {
        match self.kind {
            EffectKind::GaussianBlur(_) => EffectType::GaussianBlur,
            EffectKind::MosaicBlur(_) => EffectType::MosaicBlur,
            EffectKind::Glow(_) => EffectType::Glow,
            EffectKind::FilmGrain(_) => EffectType::FilmGrain,
            EffectKind::Vignette(_) => EffectType::Vignette,
            EffectKind::Sharpen(_) => EffectType::Sharpen,
        }
    }

    pub fn moves_by_itself(&self) -> bool {
        match &self.kind {
            EffectKind::Glow(g) => g.atmosphere,
            EffectKind::FilmGrain(g) => !g.freeze,
            _ => false,
        }
    }

    pub fn mixed(&self) -> f64 {
        self.blend.unwrap_or(1.0)
    }
}

impl GaussianBlur {
    pub const STRENGTH: f64 = 0.4;
    pub const STRENGTHS: (f64, f64) = (0.0, 1.0);
    pub const CHANNELS: (f64, f64) = (0.0, 2.0);
    pub const SPREAD: f64 = 0.05625;
    pub const SAME: [f64; 4] = [1.0; 4];

    pub fn across(&self) -> f64 {
        self.strength.unwrap_or(Self::STRENGTH)
    }

    pub fn down(&self) -> f64 {
        if self.split {
            self.vertical_strength.unwrap_or(Self::STRENGTH)
        } else {
            self.across()
        }
    }

    pub fn across_channels(&self) -> [f64; 4] {
        self.horizontal_channels.unwrap_or(Self::SAME)
    }

    pub fn down_channels(&self) -> [f64; 4] {
        self.vertical_channels.unwrap_or(Self::SAME)
    }

    pub fn edge(&self) -> Border {
        self.border.unwrap_or_default()
    }

    pub fn spreads(&self, width: f64) -> ([f64; 4], [f64; 4]) {
        let of = |s: f64, k: [f64; 4]| k.map(|k| Self::SPREAD * s * s * width * k);
        (
            of(self.across(), self.across_channels()),
            of(self.down(), self.down_channels()),
        )
    }
}

impl MosaicBlur {
    pub const CELL: f64 = 2.0;
    pub const FREQUENCY: f64 = 100.0;
    pub const FREQUENCIES: (f64, f64) = (0.1, 4000.0);
    pub const ALIASING: f64 = 1.0;
    pub const ALIASINGS: (f64, f64) = (0.0, 2.0);
    pub const STRETCH: f64 = 1.0;
    pub const STRETCHES: (f64, f64) = (0.001, 1000.0);
    pub const OFFSETS: (f64, f64) = (-1.0, 1.0);
    pub const SMOOTHS: (f64, f64) = (0.0, 1.0);
    pub const MIDDLE: [f64; 2] = [0.5, 0.5];
    pub const PLACES: (f64, f64) = (-1e6, 1e6);

    pub fn frequency(&self) -> f64 {
        self.pixel_frequency.unwrap_or(Self::FREQUENCY)
    }

    pub fn blurs_alpha(&self) -> bool {
        self.blur_alpha.unwrap_or(true)
    }

    pub fn smooth(&self) -> f64 {
        self.smooth_strength.unwrap_or(0.0)
    }

    pub fn shape(&self) -> CellShape {
        self.cell_shape.unwrap_or_default()
    }

    pub fn box_size(&self) -> f64 {
        self.aliasing.unwrap_or(Self::ALIASING)
    }

    pub fn offsets(&self) -> [f64; 2] {
        [self.x_offset.unwrap_or(0.0), self.y_offset.unwrap_or(0.0)]
    }

    pub fn stretch(&self) -> f64 {
        self.aspect.unwrap_or(Self::STRETCH)
    }

    pub fn place(&self) -> [f64; 2] {
        self.position.unwrap_or(Self::MIDDLE)
    }

    pub fn edge(&self) -> Border {
        self.border.unwrap_or_default()
    }
}

impl Glow {
    pub const SIGMA: f64 = 0.146_666_666_666_666_66;
    pub const SECOND: f64 = 3.0;
    pub const THRESHOLD: f64 = 0.75;
    pub const ALPHA_THRESHOLD: f64 = 0.5;
    pub const UNIT: (f64, f64) = (0.0, 1.0);
    pub const SPREAD: f64 = 0.5;
    pub const SPREADS: (f64, f64) = (0.0, 3.0);
    pub const RATIOS: (f64, f64) = (-1.0, 1.0);
    pub const WHITE: [f64; 3] = [1.0; 3];
    pub const RELATIVES: (f64, f64) = (0.0, 2.0);
    pub const GAIN: f64 = 0.5;
    pub const GAINS: (f64, f64) = (0.0, 10.0);
    pub const GAMMAS: (f64, f64) = (0.1, 10.0);
    pub const SATURATIONS: (f64, f64) = (0.0, 10.0);
    pub const COLORS: (f64, f64) = (0.0, 1e6);
    pub const SECONDARY_GAIN: f64 = 0.75;
    pub const SECONDARY_SPREAD: f64 = 0.25;
    pub const STRENGTH: f64 = 0.25;
    pub const STRENGTHS: (f64, f64) = (-5.0, 5.0);
    pub const DETAIL: f64 = 0.55;
    pub const SCALE: f64 = 0.6;
    pub const SCALES: (f64, f64) = (0.01, 100.0);
    pub const NOISE_RATIOS: (f64, f64) = (-0.99, 0.99);
    pub const EVOLUTIONS: (f64, f64) = (0.0, 1000.0);
    pub const SEETHE: f64 = 0.3;
    pub const SEETHES: (f64, f64) = (0.0, 10.0);
    pub const RECOVERIES: (f64, f64) = (0.0, 5.0);

    pub fn shows(&self) -> GlowOutput {
        self.output.unwrap_or_default()
    }

    pub fn alpha(&self) -> GlowAlpha {
        self.input_alpha.unwrap_or_default()
    }

    pub fn shine(&self) -> f64 {
        self.threshold.unwrap_or(Self::THRESHOLD)
    }

    pub fn alpha_shine(&self) -> f64 {
        self.alpha_threshold.unwrap_or(Self::ALPHA_THRESHOLD)
    }

    pub fn wide(&self) -> f64 {
        self.spread.unwrap_or(Self::SPREAD)
    }

    pub fn ratio(&self) -> f64 {
        self.hv_ratio.unwrap_or(0.0)
    }

    pub fn relative(&self) -> [f64; 3] {
        self.relative_spread.unwrap_or(Self::WHITE)
    }

    pub fn bright(&self) -> f64 {
        self.gain.unwrap_or(Self::GAIN)
    }

    pub fn curve(&self) -> f64 {
        self.gamma.unwrap_or(1.0)
    }

    pub fn colorful(&self) -> f64 {
        self.saturation.unwrap_or(1.0)
    }

    pub fn filter(&self) -> [f64; 3] {
        self.color_filter.unwrap_or(Self::WHITE)
    }

    pub fn second_bright(&self) -> f64 {
        self.secondary_gain.unwrap_or(Self::SECONDARY_GAIN)
    }

    pub fn second_wide(&self) -> f64 {
        self.secondary_spread.unwrap_or(Self::SECONDARY_SPREAD)
    }

    pub fn second_color(&self) -> [f64; 3] {
        self.secondary_color.unwrap_or(Self::WHITE)
    }

    pub fn noise_strength(&self) -> f64 {
        self.atmosphere_strength.unwrap_or(Self::STRENGTH)
    }

    pub fn noise_detail(&self) -> f64 {
        self.atmosphere_detail.unwrap_or(Self::DETAIL)
    }

    pub fn noise_scale(&self) -> f64 {
        self.atmosphere_scale.unwrap_or(Self::SCALE)
    }

    pub fn noise_ratio(&self) -> f64 {
        self.atmosphere_ratio.unwrap_or(0.0)
    }

    pub fn noise_evolution(&self) -> f64 {
        self.atmosphere_evolution.unwrap_or(0.0)
    }

    pub fn noise_seethe(&self) -> f64 {
        self.atmosphere_seethe.unwrap_or(Self::SEETHE)
    }

    pub fn frames(&self) -> GlowFraming {
        self.framing.unwrap_or_default()
    }

    pub fn over(&self) -> CompositeMode {
        self.composite_type.unwrap_or(CompositeMode::Add)
    }

    pub fn shown(&self) -> f64 {
        self.opacity.unwrap_or(1.0)
    }

    pub fn recovers(&self) -> f64 {
        self.recovery.unwrap_or(0.0)
    }
}

impl FilmGrain {
    pub const UNIT: (f64, f64) = (0.0, 1.0);
    pub const OPACITY: f64 = 0.5;
    pub const TEXTURE: f64 = 0.704;
    pub const SIZE: f64 = 0.66;
    pub const ASPECTS: (f64, f64) = (0.2, 5.0);
    pub const STRENGTH: f64 = 0.14875;
    pub const OFFSET: f64 = 0.482;
    pub const SYMMETRY: f64 = 0.5;
    pub const SOFTNESS: f64 = 0.298_395_127_058_029_2;
    pub const DETAILS: (f64, f64) = (0.0, 2.0);
    pub const WHITE: [f64; 3] = [1.0; 3];
    pub const GAINS: (f64, f64) = (-10.0, 10.0);
    pub const TONE: f64 = 0.5;
    const SAME: f64 = 1e-6;

    pub fn over(&self) -> GrainComposite {
        self.grain_composite.unwrap_or_default()
    }

    pub fn shown(&self) -> f64 {
        self.opacity.unwrap_or(Self::OPACITY)
    }

    pub fn rough(&self) -> f64 {
        self.texture.unwrap_or(Self::TEXTURE)
    }

    pub fn large(&self) -> f64 {
        self.grain_size.unwrap_or(Self::SIZE)
    }

    pub fn stretch(&self) -> f64 {
        self.grain_aspect.unwrap_or(1.0)
    }

    pub fn strong(&self) -> f64 {
        self.grain_strength.unwrap_or(Self::STRENGTH)
    }

    pub fn middle(&self) -> f64 {
        self.offset.unwrap_or(Self::OFFSET)
    }

    pub fn lean(&self) -> f64 {
        self.symmetry.unwrap_or(Self::SYMMETRY)
    }

    pub fn soft(&self) -> f64 {
        self.softness.unwrap_or(Self::SOFTNESS)
    }

    pub fn colorful(&self) -> f64 {
        self.saturation.unwrap_or(0.0)
    }

    pub fn lost(&self) -> f64 {
        self.detail_loss.unwrap_or(0.0)
    }

    pub fn gains(&self) -> [f64; 3] {
        self.channel_gains.unwrap_or(Self::WHITE)
    }

    pub fn tones(&self) -> [f64; 3] {
        [self.shadows, self.midtones, self.highlights].map(|t| t.unwrap_or(Self::TONE))
    }

    pub fn preset(&self) -> GrainPreset {
        let plain = self.shown() == Self::OPACITY
            && self.stretch() == 1.0
            && self.colorful() == 0.0
            && self.lost() == 0.0
            && self.gains() == Self::WHITE
            && self.tones() == [Self::TONE; 3];
        let now = [
            self.rough(),
            self.large(),
            self.strong(),
            self.middle(),
            self.lean(),
            self.soft(),
        ];
        let same = |p: &GrainPreset| {
            p.values().is_some_and(|values| {
                values
                    .iter()
                    .zip(&now)
                    .all(|(a, b)| (a - b).abs() < Self::SAME)
            })
        };
        match plain {
            true => GrainPreset::ALL
                .into_iter()
                .find(|p| same(p))
                .unwrap_or(GrainPreset::Custom),
            false => GrainPreset::Custom,
        }
    }

    pub fn take_preset(&mut self, preset: GrainPreset) {
        let Some([texture, size, strength, offset, symmetry, softness]) = preset.values() else {
            return;
        };
        *self = Self {
            grain_composite: self.grain_composite,
            grain_only: self.grain_only,
            freeze: self.freeze,
            animate: self.animate,
            texture: Some(texture),
            grain_size: Some(size),
            grain_strength: Some(strength),
            offset: Some(offset),
            symmetry: Some(symmetry),
            softness: Some(softness),
            ..Self::default()
        };
    }
}

impl Vignette {
    pub const SIZE: f64 = 0.5;
    pub const SIZES: (f64, f64) = (0.01, 2.0);
    pub const TURNS: (f64, f64) = (-360.0, 360.0);
    pub const STRETCH: f64 = 1.78;
    pub const STRETCHES: (f64, f64) = (0.2, 5.0);
    pub const MIDDLE: [f64; 2] = [0.5, 0.5];
    pub const PLACES: (f64, f64) = (-1e6, 1e6);
    pub const SOFTNESS: f64 = 0.5;
    pub const SHARES: (f64, f64) = (0.0, 1.0);
    pub const BLACK: [f64; 3] = [0.0; 3];
    pub const MIXING: CompositeMode = CompositeMode::Multiply;

    pub fn mode(&self) -> VignetteMode {
        self.operating_mode.unwrap_or_default()
    }

    pub fn shape(&self) -> VignetteShape {
        self.border_shape.unwrap_or_default()
    }

    pub fn large(&self) -> f64 {
        self.size.unwrap_or(Self::SIZE)
    }

    pub fn turn(&self) -> f64 {
        self.rotation.unwrap_or(0.0)
    }

    pub fn stretch(&self) -> f64 {
        self.anamorphism.unwrap_or(Self::STRETCH)
    }

    pub fn middle(&self) -> [f64; 2] {
        self.center.unwrap_or(Self::MIDDLE)
    }

    pub fn soft(&self) -> f64 {
        self.softness.unwrap_or(Self::SOFTNESS)
    }

    pub fn through(&self) -> f64 {
        self.transparency.unwrap_or(0.0)
    }

    pub fn tint(&self) -> [f64; 3] {
        self.color.unwrap_or(Self::BLACK)
    }

    pub fn mixing(&self) -> CompositeMode {
        self.composite_type.unwrap_or(Self::MIXING)
    }
}

impl Sharpen {
    pub const AMOUNT: f64 = 1.8;
    pub const GAIN: f64 = 1.0;
    pub const GAINS: (f64, f64) = (1.0, 5.0);
    pub const SIZE: f64 = 0.05;
    pub const SIZES: (f64, f64) = (0.01, 0.1);

    pub fn amount(&self) -> f64 {
        self.sharpen_amount.unwrap_or(Self::AMOUNT)
    }

    pub fn size(&self) -> f64 {
        self.fine_detail_size.unwrap_or(Self::SIZE)
    }

    pub fn fine(&self) -> f64 {
        self.fine_details.unwrap_or(Self::GAIN)
    }

    pub fn medium(&self) -> f64 {
        self.medium_details.unwrap_or(Self::GAIN)
    }

    pub fn large(&self) -> f64 {
        self.large_details.unwrap_or(Self::GAIN)
    }

    pub fn chroma(&self) -> f64 {
        self.sharpen_chroma.unwrap_or(Self::GAIN)
    }
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
    pub const REACH: f64 = 100.0;
    pub const SIZE: (f64, f64) = (0.01, 100.0);

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
/// (`left`, `right`) and height (`top`, `bottom`), each from 0 to 1. When
/// two opposite sides meet, it does not show. Usual: 0.
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
    pub const REACH: f64 = 4.0;
    pub const ZOOM: (f64, f64) = (0.0, 100.0);
    pub const TURNS: f64 = 100_000.0;
    pub const LEAN: f64 = 1.5;
    pub const SOFT: f64 = 100.0;

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
    /// The clips it was made from, whole, on the tracks they came from,
    /// frames from its first: what `edit.decompose` puts back. Left out on
    /// a compound clip from another editor's file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub made_from: Vec<Track>,
}

/// A compound clip in the Media Pool: a copy that can go on the timeline
/// again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PooledCompound {
    /// `k<N>`, never reused.
    pub id: String,
    pub name: String,
    /// The bin it is in (a path from [`Project::bins`]); empty: the top level.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bin: String,
    /// The picture block. Left out when it holds only sound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<Compound>,
    /// The sound block. Left out when it holds only pictures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<Compound>,
}

impl Compound {
    pub fn holds(&self, media: &str) -> bool {
        self.tracks
            .iter()
            .chain(&self.made_from)
            .flat_map(|t| &t.clips)
            .any(|c| c.uses(media))
    }
}

impl PooledCompound {
    pub fn frames(&self) -> i64 {
        self.picture
            .as_ref()
            .or(self.sound.as_ref())
            .map_or(0, |k| k.frames)
    }

    pub fn blocks(&self) -> impl Iterator<Item = &Compound> {
        self.picture.iter().chain(&self.sound)
    }

    pub fn blocks_mut(&mut self) -> impl Iterator<Item = &mut Compound> {
        self.picture.iter_mut().chain(&mut self.sound)
    }

    pub fn holds(&self, media: &str) -> bool {
        self.blocks().any(|k| k.holds(media))
    }
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
    /// The value there: the volume in dB, from -100 to 30.
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
    /// The volume in dB, from -100 to 30, while the clip has no volume
    /// keyframes.
    pub gain_db: f64,
    /// The volume keyframes (decision D81), in the order of their frames,
    /// one at most on a frame. With them, the volume is their curve.
    /// Left out when the clip has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gain_keys: Vec<Keyframe>,
    pub muted: bool,
    /// The pan, from -100 (all the way left) to 100 (all the way right).
    /// Left out at 0, the middle.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pan: f64,
    /// The switch of the pan. Off keeps the pan, and the sound plays as at
    /// 0. Usual: on.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub pan_on: bool,
    /// The equalizer. Left out when the clip has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equalizer: Option<Equalizer>,
    /// The Dialogue Leveler. Left out when the clip has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialogue_leveler: Option<DialogueLeveler>,
    /// The compressor. Left out when the clip has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compressor: Option<Compressor>,
    /// Noise Reduction. Left out when the clip has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_reduction: Option<NoiseReduction>,
    /// Effects on its sound, carried from another editor. The clip volume
    /// is `gain_db` and `gain_keys`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Effects>,
    /// Voice Isolation of the clip. A clip that has it off stays off, also
    /// when its track has it on. Left out, the clip follows its track's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_isolation: Option<VoiceIsolation>,
}

/// An equalizer: six bands, one after the other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Equalizer {
    /// The switch. Off keeps the bands.
    pub on: bool,
    /// Bands 1 to 6, from the low end to the high end.
    pub bands: [EqBand; 6],
}

/// One band of an equalizer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EqBand {
    /// The switch of this band. Off keeps its values.
    pub on: bool,
    /// Its filter.
    pub kind: EqKind,
    /// Its frequency in Hz, from 20 to 22000.
    pub freq: f64,
    /// Its gain in dB, from -70 to 24. Only `bell`, `lo_shelf` and
    /// `hi_shelf` use it.
    pub gain: f64,
    /// Its width, from 0.3 to 100. Only `bell` uses it.
    pub q: f64,
}

/// The filter of an equalizer band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EqKind {
    Bell,
    LoShelf,
    HiShelf,
    Notch,
    HiPass,
    LoPass,
}

/// The Dialogue Leveler: it rides the level of dialogue.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DialogueLeveler {
    /// The switch. Off keeps the settings.
    pub on: bool,
    /// How far each of the three processes goes.
    pub preset: LevelerPreset,
    /// Rides loud dialogue down.
    pub reduce_loud: bool,
    /// Lifts soft dialogue.
    pub lift_soft: bool,
    /// Lowers the sound between the lines of dialogue.
    pub background_reduction: bool,
    /// The output gain in dB, from 0 to 6. Only with `reduce_loud` on.
    pub output_gain_db: f64,
}

/// The preset of a Dialogue Leveler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LevelerPreset {
    AllowWiderDynamics,
    OptimizeModerateLevels,
    MoreLiftForLowLevels,
    LiftSoftWhisperySources,
}

/// A compressor: it lowers the level above its threshold.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Compressor {
    /// The switch. Off keeps the settings.
    pub on: bool,
    /// The level above which it compresses, in dB, from -50 to 0.
    pub threshold_db: f64,
    /// The ratio of the input to the output, from 1 to 10.
    pub ratio: f64,
    /// The width of the curve around the threshold, in dB, from 0 to 30.
    /// At 0, compression starts at the threshold.
    pub knee_db: f64,
    /// How much of the sound stays as it was, in percent, from 0 to 100.
    pub mix: f64,
    /// How fast it starts to compress, in ms, from 0 to 100.
    pub attack_ms: f64,
    /// How long it keeps the reduction after the attack, in ms, from 0 to
    /// 4000.
    pub hold_ms: f64,
    /// How fast it stops to compress, in ms, from 50 to 4000.
    pub release_ms: f64,
    /// The gain after it, in dB, from 0 to 24.
    pub makeup_db: f64,
}

/// Noise Reduction: it lowers the steady noise of the room and the hiss.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoiseReduction {
    /// The switch. Off keeps the settings.
    pub on: bool,
    /// How far above the noise a sound must be to stay, in dB, from 0 to
    /// 40. A higher value removes more.
    pub threshold_db: f64,
    /// How fast the noise profile follows a change, in ms, from 10 to 1000.
    pub attack_ms: f64,
    /// How much the noise profile is raised, in dB, from 0 to 12. A higher
    /// value removes more, also of the dialogue.
    pub sensitivity_db: f64,
    /// How much faster the signal profile is than the noise profile, from
    /// 1 to 10.
    pub ratio: f64,
    /// How much the result is smoothed across frequencies, in percent,
    /// from 0 to 100.
    pub frequency_smoothing: f64,
    /// Smooths the result over time.
    pub time_smoothing: bool,
    /// The share of the processed sound, in percent, from 0 to 100.
    pub dry_wet: f64,
    /// The gain before the mix with the dry sound, in dB, from -6 to 18.
    pub level_db: f64,
}

/// Voice Isolation: a switch and an Amount.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceIsolation {
    /// The switch. Off keeps the Amount for the next time it goes on.
    pub on: bool,
    /// From 0 to 100: that much of the voice the model hears in the sound,
    /// mixed with the rest of the sound. At 0 the sound equals the source,
    /// and the model does not run.
    pub amount: f64,
}

impl VoiceIsolation {
    pub fn heard(self) -> Option<f64> {
        (self.on && self.amount > 0.0).then_some(self.amount)
    }
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

    pub fn is_sound_block(&self) -> bool {
        self.is_block() && !self.video
    }

    pub fn uses(&self, media: &str) -> bool {
        self.media == media || self.compound.as_ref().is_some_and(|k| k.holds(media))
    }

    pub fn draws(&self) -> bool {
        self.generator.as_ref().is_some_and(Generator::draws)
    }

    pub fn is_offline(&self) -> bool {
        self.generator.as_ref().is_some_and(Generator::is_offline)
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
    /// Text height as a fraction of the picture height, from 1/1080 to
    /// 400/1080. Usual: 58/1080 (about 0.054).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// `#rrggbb`, `#rgb` or a name such as `yellow`. Usual: white.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Outline color and width. Usual: white, 1 pixel of the timeline wide,
    /// centered on the edge of the letters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<SubtitleOutline>,
    /// A box with round corners behind the whole block of text. Usual:
    /// black, half opaque.
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
    /// A soft copy of the text behind it. An offset of 0 draws nothing.
    /// Usual: none.
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
    /// How the lines line up with the longest line: `left`, `center` or
    /// `right`. The block stays where it is. Usual: `center`. On a cue, left
    /// out keeps the track's alignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<SubtitleAlign>,
    /// Where the block sits, `[x, y]` as fractions of the picture from its
    /// top left, from -4 to 4 across and from -3 to 5 down. It is the point
    /// of the block that `anchor` names. Without an anchor, `x` is the middle
    /// of the longest line, and `y` the middle of the block, or with a
    /// `margin` the edge of a top or bottom cue. Usual: the cue's place. On a
    /// cue, left out keeps the track's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
    /// The point of the block that goes to `position`, or to the cue's
    /// place: a corner, the middle of a side, or `middle`. Across, the block
    /// is as wide as its longest line. Usual: the middle across; down, the
    /// middle, or with a `margin` the top edge of a top cue and the bottom
    /// edge of a bottom cue. On a cue, left out keeps the track's anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<TitleAnchor>,
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

fn one_upper(c: char) -> char {
    let mut mapped = c.to_uppercase();
    match (mapped.next(), mapped.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

fn one_lower(c: char) -> char {
    let mut mapped = c.to_lowercase();
    match (mapped.next(), mapped.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

/// How a title's letters draw.
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TitleCase {
    #[default]
    Mixed,
    Upper,
    Lower,
    Small,
    Title,
}

impl TitleCase {
    pub const SMALL_CAPS: f64 = 0.7;

    pub fn word(self) -> &'static str {
        match self {
            Self::Mixed => "mixed",
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::Small => "small",
            Self::Title => "title",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        [
            Self::Mixed,
            Self::Upper,
            Self::Lower,
            Self::Small,
            Self::Title,
        ]
        .into_iter()
        .find(|c| c.word() == word)
    }

    pub fn pieces(self, text: &str, before: &str) -> Vec<(String, bool)> {
        match self {
            Self::Mixed => vec![(text.to_string(), false)],
            Self::Upper => vec![(text.chars().map(one_upper).collect(), false)],
            Self::Lower => vec![(text.chars().map(one_lower).collect(), false)],
            Self::Title => {
                let whole = format!("{before}{text}");
                let mut out = String::with_capacity(text.len());
                for (at, word) in whole.split_word_bound_indices() {
                    let mut start = true;
                    for (i, c) in word.char_indices() {
                        if at + i >= before.len() {
                            out.push(if start { one_upper(c) } else { c });
                        }
                        start = matches!(c, '.' | ':');
                    }
                }
                vec![(out, false)]
            }
            Self::Small => {
                let mut out: Vec<(String, bool)> = Vec::new();
                for c in text.chars() {
                    let small = get_general_category(c) == GeneralCategory::LowercaseLetter;
                    if out.last().is_none_or(|(_, s)| *s != small) {
                        out.push((String::new(), small));
                    }
                    let last = out.last_mut().expect("just pushed");
                    last.0.push(if small { one_upper(c) } else { c });
                }
                out
            }
        }
    }
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
    /// `#rrggbb`, `#rgb` or a name. Usual: white on a subtitle track, black
    /// on a title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Width in pixels of the timeline, from 0 to 16, whatever the text size
    /// and the timeline size. `0` draws none. Usual: 1 on a subtitle track,
    /// 0 on a title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Half of the width always reaches past the letters. `true` draws it
    /// behind them and leaves them whole. Left out, it goes over them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outside: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStroke {
    pub ink: [u8; 3],
    pub width: f64,
    pub outside: bool,
}

impl TextStroke {
    pub const MOST: f64 = 16.0;

    pub fn past_letter(&self) -> f64 {
        self.width / 2.0
    }
}

/// A box behind a subtitle track's or a title's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubtitleBackground {
    /// `#rrggbb`, `#rgb` or a name.
    pub color: String,
    /// From 0 to 1. `0` draws no box on a subtitle track. Usual: 1/2 on a
    /// subtitle track, 1 on a title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// `true` draws no box and keeps these values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<bool>,
}

impl SubtitleBackground {
    pub fn is_off(&self) -> bool {
        self.off == Some(true)
    }
}

impl SubtitleStyle {
    pub const MOST_SIZE: u32 = 400;
    pub const SIZE: f64 = 58.0 / 1080.0;
    pub const LINE: f64 = 1.362;
    pub const STROKE: f64 = 1.0;
    pub const BOX_ACROSS: f64 = 0.3 * 1080.0;
    pub const BOX_DOWN: f64 = 5.0;
    pub const BOX_CORNER: f64 = 0.010;
    pub const BOX_OPACITY: f64 = 0.5;

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
            && self.anchor.is_none()
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
            anchor: cue.anchor.or(self.anchor),
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
        self.size.unwrap_or(Self::SIZE)
    }

    pub fn ink(&self) -> [u8; 3] {
        paint(self.color.as_deref(), [255, 255, 255])
    }

    pub fn stroke(&self) -> Option<TextStroke> {
        stroke_of(self.outline.as_ref(), Self::STROKE, [255, 255, 255])
    }

    pub fn backing(&self) -> Option<([u8; 3], f64)> {
        if self
            .background
            .as_ref()
            .is_some_and(SubtitleBackground::is_off)
        {
            return None;
        }
        let (ink, opacity) = self.background.as_ref().map_or(([0, 0, 0], None), |b| {
            (paint(Some(&b.color), [0, 0, 0]), b.opacity)
        });
        let opacity = opacity.unwrap_or(Self::BOX_OPACITY);
        (opacity > 0.0).then_some((ink, opacity))
    }

    pub fn inset(&self) -> f64 {
        self.margin.unwrap_or(0.05)
    }

    pub fn wrap_width(&self, picture_width: f64, picture_height: f64) -> f64 {
        (picture_width * (1.0 - 2.0 * self.inset())).max(self.text_fraction() * picture_height)
    }

    pub fn anchor(&self, place: Place) -> (f64, f64) {
        let (at, above) = match (self.margin, place) {
            (Some(m), Place::Top) => (m, 0.0),
            (Some(m), Place::Bottom) => (1.0 - m, 1.0),
            (_, Place::Middle) => (0.5, 0.5),
            (None, Place::Top) => (0.2, 0.5),
            (None, Place::Bottom) => (0.8, 0.5),
        };
        (at, self.anchor.map_or(above, |a| a.fractions()[1]))
    }

    pub fn anchor_across(&self) -> f64 {
        self.anchor.map_or(0.5, |a| a.fractions()[0])
    }

    pub fn anchor_in(&self, place: Place) -> TitleAnchor {
        TitleAnchor::at([self.anchor_across(), self.anchor(place).1])
    }

    pub fn usual_position(&self, place: Place) -> [f64; 2] {
        [0.5, self.anchor(place).0]
    }

    pub fn block_top(&self, place: Place, picture_height: f64, lines: f64) -> f64 {
        let (at, above) = self.anchor(place);
        picture_height * at - lines * above
    }

    pub fn block_box(
        &self,
        place: Place,
        (picture_w, picture_h): (f64, f64),
        (wrap, widest): (f64, f64),
        lines: f64,
    ) -> (f64, f64) {
        let (at, above) = self.anchor(place);
        let [x, y] = self.position.unwrap_or([0.5, at]);
        let lined = match self.line_align() {
            SubtitleAlign::Left => 0.0,
            SubtitleAlign::Center => 0.5,
            SubtitleAlign::Right => 1.0,
        };
        let point = lined * (wrap - widest) + self.anchor_across() * widest;
        (picture_w * x - point, picture_h * y - lines * above)
    }
}

fn paint(hex: Option<&str>, usual: [u8; 3]) -> [u8; 3] {
    hex.and_then(crate::subtitles::rgb).unwrap_or(usual)
}

fn stroke_of(o: Option<&SubtitleOutline>, usual: f64, ink: [u8; 3]) -> Option<TextStroke> {
    let width = o.and_then(|o| o.size).unwrap_or(usual);
    (width > 0.0).then(|| TextStroke {
        ink: paint(o.and_then(|o| o.color.as_deref()), ink),
        width,
        outside: o.and_then(|o| o.outside).unwrap_or(false),
    })
}

fn backing_of(b: Option<&SubtitleBackground>) -> Option<([u8; 3], f64)> {
    let b = b.filter(|b| !b.is_off())?;
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
    /// How its picture goes over the frame: position, zoom, rotation, pitch,
    /// yaw, crop and opacity, with their keyframes. The picture is as large
    /// as the frame. Keyframes count frames from the cue's start, and can be
    /// before or after the cue. The whole value is left out when nothing is
    /// changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<Picture>,
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

    pub fn as_clip(&self) -> Clip {
        Clip {
            id: self.id.clone(),
            media: String::new(),
            start: self.start,
            duration: self.duration,
            source_in: 0,
            video: true,
            audio: None,
            enabled: true,
            name: Some(crate::subtitles::plain(&self.text).replace('\n', " ")),
            effects: None,
            generator: None,
            title: None,
            picture: self.picture.clone(),
            compound: None,
            speed: None,
            retime: ClipRetime::Project,
            color: None,
            render_cache: ClipCache::Auto,
            motion: Vec::new(),
        }
    }

    pub fn move_keys(&mut self, by: i64) {
        if let Some(picture) = self.picture.as_mut() {
            picture.retarget(|f| f + by);
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
    pub group: u64,
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
            compounds: Vec::new(),
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
                group: 1,
            },
            bins: Vec::new(),
            groups: Vec::new(),
            speakers: Default::default(),
        }
    }

    pub fn group(&self, id: &str) -> Option<&SyncGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_of(&self, media: &str) -> Option<&SyncGroup> {
        self.groups
            .iter()
            .rev()
            .find(|g| g.clips().any(|c| c.media == media))
    }

    pub fn shown_clip(&self, playhead: i64) -> Option<&Clip> {
        for ti in self.tracks_of(TrackKind::Video).into_iter().rev() {
            let track = &self.tracks[ti];
            if !track.video_enabled {
                continue;
            }
            let Some(ci) = track.clip_at(playhead) else {
                continue;
            };
            let clip = &track.clips[ci];
            if clip.enabled && clip.video && !clip.is_block() {
                return Some(clip);
            }
        }
        None
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

    pub fn clips_using<'a>(&'a self, media: &'a str) -> impl Iterator<Item = &'a Clip> {
        self.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(move |c| c.uses(media))
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
        match media.subclip {
            Some(s) => self.frames_in(s.end - s.start),
            None => self.file_frames(media),
        }
    }

    pub fn subclip_span(&self, media: &Media) -> Option<(i64, i64)> {
        let first = self.frames_in(media.subclip?.start);
        Some((first, first + self.media_frames(media)))
    }

    pub fn file_frames(&self, media: &Media) -> i64 {
        let secs = match (&media.info.video, media.info.audio.first()) {
            (Some(v), _) if v.still => self.settings.still_secs,
            (Some(v), _) => v.duration,
            (None, Some(a)) => a.duration,
            (None, None) => 0.0,
        };
        self.frames_in(secs)
    }

    pub fn marker(&self, id: &str) -> Option<&Marker> {
        self.markers
            .iter()
            .chain(self.media.iter().flat_map(|m| &m.markers))
            .find(|k| k.id == id)
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

    pub fn alloc_pooled_compound_id(&mut self) -> String {
        let id = format!("k{}", self.next_id.clip);
        self.next_id.clip += 1;
        id
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

    pub fn new_group_id(&mut self) -> String {
        let id = format!("g{}", self.next_id.group);
        self.next_id.group += 1;
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

    pub fn key_holder(&self, id: &str) -> Option<Clip> {
        self.clip(id)
            .cloned()
            .or_else(|| self.cue(id).map(Cue::as_clip))
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
            render_cache: true,
        }
    }

    pub fn has_sound(&self) -> bool {
        self.kind == TrackKind::Audio || self.clips.iter().any(|c| c.audio.is_some())
    }

    pub fn in_mix(&self, any_solo: bool) -> bool {
        !self.muted && (self.solo || !any_solo)
    }

    pub fn audible(&self, any_solo: bool) -> bool {
        self.in_mix(any_solo) && !crate::validate::fader_silent(self.volume_db)
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
            anchor,
            text,
            style,
            position,
            zoom,
            rotation,
            opacity,
            upper,
            follows,
            ..
        } = self;
        (kind, align, anchor, text, style, follows).hash(h);
        pair_bits(*position).hash(h);
        (
            floats_bits(*zoom),
            floats_bits(*rotation),
            floats_bits(*opacity),
        )
            .hash(h);
        upper
            .as_ref()
            .map(|u| {
                (
                    u.anchor,
                    pair_bits(u.position),
                    floats_bits(u.zoom),
                    floats_bits(u.rotation),
                )
            })
            .hash(h);
    }
}

impl Eq for Title {}

impl Hash for TitleStyle {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let TitleStyle {
            font,
            size,
            line_spacing,
            tracking,
            case,
            color,
            outline,
            background,
            shape,
            shadow,
        } = self;
        (
            font,
            floats_bits(*size),
            floats_bits(*line_spacing),
            floats_bits(*tracking),
            case,
            color,
            outline,
            background,
            shape,
            shadow,
        )
            .hash(h);
    }
}

impl Eq for TitleStyle {}

impl Hash for TitleShape {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let TitleShape {
            size,
            center,
            corner,
            outline_color,
            outline_width,
        } = self;
        (
            pair_bits(Some(*size)),
            pair_bits(*center),
            floats_bits(*corner),
            outline_color,
            floats_bits(*outline_width),
        )
            .hash(h);
    }
}

impl Eq for TitleShape {}

impl Hash for TitleShadow {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let TitleShadow {
            color,
            opacity,
            offset,
            blur,
            off,
        } = self;
        (
            color,
            floats_bits(*opacity),
            pair_bits(*offset),
            floats_bits(*blur),
            off,
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
            anchor,
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
            anchor,
        )
            .hash(h);
    }
}

impl Eq for SubtitleStyle {}

impl Hash for SubtitleOutline {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let SubtitleOutline {
            color,
            size,
            outside,
        } = self;
        (color, floats_bits(*size), outside).hash(h);
    }
}

impl Eq for SubtitleOutline {}

impl Hash for SubtitleBackground {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let SubtitleBackground {
            color,
            opacity,
            off,
        } = self;
        (color, floats_bits(*opacity), off).hash(h);
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
