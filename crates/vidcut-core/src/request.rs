//! The command protocol: every capability as one serializable request.
//!
//! The JSON form is `{"method": "...", "params": {...}}`. It is the same for
//! JSON-RPC, `vidcut apply` batch lines and `vidcut rpc`, and every GUI action
//! maps onto one of these. The common options `if_rev` and `dry_run` travel
//! next to the params (see [`Envelope`]).

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::export::{EncoderChoice, ExportSettings};
use crate::model::Clip;
use crate::ops::{Edge, MoveMode};
use crate::time::{RangeSpec, Rational, TimeSpec};

/// A clip reference: an id like `c17`, or `V1@<TimeSpec>` for the clip under
/// that frame on a track.
pub type ClipRef = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    // ---------------------------------------------------------------- project
    /// The whole project as JSON.
    #[serde(rename = "project.get")]
    ProjectGet {},
    /// Settings, counts, duration and marks.
    #[serde(rename = "project.summary")]
    ProjectSummary {},
    /// Change settings. `fps` can only change while the timeline is empty.
    #[serde(rename = "project.set")]
    ProjectSet(ProjectSet),

    // ---------------------------------------------------------------- media pool
    /// Add files to the media pool (probing them); returns their media ids.
    #[serde(rename = "media.add")]
    MediaAdd { paths: Vec<PathBuf> },
    #[serde(rename = "media.list")]
    MediaList {},
    #[serde(rename = "media.get")]
    MediaGet { media: String },
    /// Remove from the pool; `force` also deletes the clips that use it.
    #[serde(rename = "media.remove")]
    MediaRemove {
        media: String,
        #[serde(default)]
        force: bool,
    },
    /// Point a pool entry at another file (re-probed).
    #[serde(rename = "media.relink")]
    MediaRelink { media: String, path: PathBuf },
    #[serde(rename = "media.rename")]
    MediaRename { media: String, name: String },
    /// Set a media item's source In and/or Out (I/O in a source viewer or over
    /// a pool thumbnail). Source time, `out` exclusive. Edits without a
    /// `range` use these marks (decisions D5, D6).
    #[serde(rename = "media.marks.set")]
    MediaMarksSet {
        media: String,
        #[serde(rename = "in", default)]
        mark_in: Option<TimeSpec>,
        #[serde(default)]
        out: Option<TimeSpec>,
    },
    /// Clear a media item's source In and Out.
    #[serde(rename = "media.marks.clear")]
    MediaMarksClear { media: String },
    /// Probe a file without adding it.
    #[serde(rename = "probe")]
    Probe { path: PathBuf },

    // ---------------------------------------------------------------- timeline queries
    /// Tracks with their clips and gaps (frames plus timecodes).
    #[serde(rename = "timeline.get")]
    TimelineGet {
        #[serde(default)]
        range: Option<RangeSpec>,
    },
    /// What is at a frame on each track.
    #[serde(rename = "timeline.at")]
    TimelineAt { at: TimeSpec },
    /// Edit points of V1 (clip boundaries), as the Smart Indicator sees them.
    #[serde(rename = "timeline.edit_points")]
    TimelineEditPoints {},

    // ---------------------------------------------------------------- edit functions (spec §7)
    /// Append after the last clip (Shift+F12).
    #[serde(rename = "edit.append")]
    EditAppend(SourceParams),
    /// Insert at the edit point nearest `playhead` (Smart Insert, F9).
    #[serde(rename = "edit.smart_insert")]
    EditSmartInsert {
        #[serde(flatten)]
        source: SourceParams,
        playhead: TimeSpec,
    },
    /// Insert at an exact frame, splitting a clip there (API-only).
    #[serde(rename = "edit.insert")]
    EditInsert {
        #[serde(flatten)]
        source: SourceParams,
        at: TimeSpec,
    },
    /// Overwrite (F10): at the timeline In, backtimed to a lone Out, or at `at`.
    #[serde(rename = "edit.overwrite")]
    EditOverwrite {
        #[serde(flatten)]
        source: SourceParams,
        #[serde(default)]
        at: Option<TimeSpec>,
    },
    /// Ripple Overwrite (Shift+F10): replace the In–Out range or the clip at `at`.
    #[serde(rename = "edit.ripple_overwrite")]
    EditRippleOverwrite {
        #[serde(flatten)]
        source: SourceParams,
        #[serde(default)]
        at: Option<TimeSpec>,
    },

    // ---------------------------------------------------------------- timeline editing (spec §8)
    /// Split at one or more frames (Ctrl+\).
    #[serde(rename = "edit.split")]
    EditSplit {
        at: OneOrMany<TimeSpec>,
        /// Only split these clips (decision D2).
        #[serde(default)]
        clips: Vec<ClipRef>,
    },
    /// Delete clips; ripples on V1 unless `lift`.
    #[serde(rename = "edit.delete")]
    EditDelete {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        lift: bool,
    },
    /// Delete a range on V1; ripples unless `lift`.
    #[serde(rename = "edit.delete_range")]
    EditDeleteRange {
        range: RangeSpec,
        #[serde(default)]
        lift: bool,
    },
    /// Close the gap containing `at`.
    #[serde(rename = "edit.close_gap")]
    EditCloseGap { at: TimeSpec },
    /// Move clips to an edit point (`insert`) or over whatever is there (`overwrite`).
    #[serde(rename = "edit.move")]
    EditMove {
        clips: OneOrMany<ClipRef>,
        to: TimeSpec,
        #[serde(default = "insert_mode")]
        mode: MoveMode,
    },
    /// Move a clip onto another, replacing it (rippling the difference).
    #[serde(rename = "edit.replace")]
    EditReplace { clip: ClipRef, onto: ClipRef },
    /// Resize a clip edge `by` frames (positive = later) or `to` a frame.
    #[serde(rename = "edit.trim")]
    EditTrim {
        clip: ClipRef,
        edge: Edge,
        #[serde(default)]
        by: Option<TimeSpec>,
        #[serde(default)]
        to: Option<TimeSpec>,
        /// Override Ripple On for this trim.
        #[serde(default)]
        ripple: Option<bool>,
    },
    /// Roll the edit at a clip's edge.
    #[serde(rename = "edit.roll")]
    EditRoll {
        clip: ClipRef,
        edge: Edge,
        by: TimeSpec,
    },
    /// Slip a clip's content.
    #[serde(rename = "edit.slip")]
    EditSlip { clip: ClipRef, by: TimeSpec },
    /// Audio-only roll at a clip's edge (J/L cut).
    #[serde(rename = "edit.audio_roll")]
    EditAudioRoll {
        clip: ClipRef,
        edge: Edge,
        by: TimeSpec,
    },
    /// Move clips' head or tail to `at` (Trim Start/End to Playhead).
    #[serde(rename = "edit.trim_to_playhead")]
    EditTrimToPlayhead {
        edge: Edge,
        at: TimeSpec,
        #[serde(default)]
        clips: Vec<ClipRef>,
        #[serde(default)]
        ripple: Option<bool>,
    },
    /// Change Clip Duration (Ctrl+D).
    #[serde(rename = "edit.change_duration")]
    EditChangeDuration {
        clips: OneOrMany<ClipRef>,
        duration: TimeSpec,
        #[serde(default)]
        ripple: Option<bool>,
    },
    /// Copy clips (Ctrl+C): returns them as `clips`, ready for `edit.paste`.
    /// The GUI also keeps them on its clipboard.
    #[serde(rename = "edit.copy")]
    EditCopy { clips: OneOrMany<ClipRef> },
    /// Cut clips (Ctrl+X): copy, then delete with ripple on Track 1.
    #[serde(rename = "edit.cut")]
    EditCut { clips: OneOrMany<ClipRef> },
    /// Paste clips (Ctrl+V) over Track 1 at `at`, keeping their spacing.
    /// Defaults: the GUI's clipboard and playhead.
    #[serde(rename = "edit.paste")]
    EditPaste {
        #[serde(default)]
        clips: Option<Vec<Clip>>,
        #[serde(default)]
        at: Option<TimeSpec>,
    },

    // ---------------------------------------------------------------- clips, marks, markers
    /// Enable/disable video (D), mute, gain (dB, -60..24) and name.
    #[serde(rename = "clip.set")]
    ClipSet {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        muted: Option<bool>,
        #[serde(default)]
        gain_db: Option<f64>,
        /// Empty string clears the name.
        #[serde(default)]
        name: Option<String>,
    },
    /// Set timeline marks; `out` is exclusive (the first frame after the range).
    #[serde(rename = "marks.set")]
    MarksSet {
        #[serde(rename = "in", default)]
        mark_in: Option<TimeSpec>,
        #[serde(default)]
        out: Option<TimeSpec>,
    },
    /// Clear timeline In and Out (Alt+X).
    #[serde(rename = "marks.clear")]
    MarksClear {},
    #[serde(rename = "markers.add")]
    MarkersAdd {
        at: TimeSpec,
        #[serde(default)]
        name: String,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        note: String,
    },
    #[serde(rename = "markers.move")]
    MarkersMove { marker: String, to: TimeSpec },
    #[serde(rename = "markers.remove")]
    MarkersRemove { marker: String },

    // ---------------------------------------------------------------- export (spec §10)
    /// Plan an export without encoding: settings, encoder, outputs, segments.
    #[serde(rename = "export.plan")]
    ExportPlan(ExportParams),
    /// Export. Quick Export semantics: the timeline In–Out range when both
    /// marks are set, otherwise the whole timeline (p. 789).
    #[serde(rename = "export.start")]
    ExportStart(ExportParams),
    /// Render one frame as PNG: the timeline at `at`, or with `media` that
    /// media's frame at `at` (source time).
    #[serde(rename = "frame.render")]
    FrameRender {
        at: TimeSpec,
        out: PathBuf,
        #[serde(default)]
        media: Option<String>,
        /// Output width; height follows the picture's aspect. Default: full size.
        #[serde(default)]
        width: Option<u32>,
    },
    /// Built-in and user export presets.
    #[serde(rename = "presets.list")]
    PresetsList {},
    #[serde(rename = "presets.get")]
    PresetsGet { name: String },
    /// Which GPU encoders work here (cached; `refresh` re-tests).
    #[serde(rename = "encoders.list")]
    EncodersList {
        #[serde(default)]
        refresh: bool,
    },

    // ---------------------------------------------------------------- history and batches
    #[serde(rename = "history.undo")]
    HistoryUndo {
        #[serde(default = "one")]
        steps: u32,
    },
    #[serde(rename = "history.redo")]
    HistoryRedo {
        #[serde(default = "one")]
        steps: u32,
    },
    /// Undo and redo labels, newest first.
    #[serde(rename = "history.get")]
    HistoryGet {},
    /// Apply requests atomically as one undo step.
    #[serde(rename = "apply")]
    Apply {
        commands: Vec<Request>,
        #[serde(default)]
        label: Option<String>,
    },

    // ---------------------------------------------------------------- live (a running GUI)
    /// Open (or with `create`, create) a project in the running GUI.
    #[serde(rename = "project.open")]
    ProjectOpen {
        path: PathBuf,
        #[serde(default)]
        create: bool,
    },
    /// Show the viewer in a mode: `source_clip`, `source_tape` or `timeline`.
    #[serde(rename = "viewer.set")]
    ViewerSet { mode: ViewerMode },
    /// Open a media item in the viewer as Source Clip, optionally at a source time.
    #[serde(rename = "viewer.load")]
    ViewerLoad {
        media: String,
        #[serde(default)]
        at: Option<TimeSpec>,
    },
    /// Play the viewer (`speed` < 0 plays backwards). With `viewer`, switch to it first.
    #[serde(rename = "player.play")]
    PlayerPlay {
        #[serde(default)]
        speed: Option<f64>,
        #[serde(default)]
        viewer: Option<ViewerMode>,
    },
    #[serde(rename = "player.pause")]
    PlayerPause {
        #[serde(default)]
        viewer: Option<ViewerMode>,
    },
    /// Play/stop (Space).
    #[serde(rename = "player.toggle")]
    PlayerToggle {
        #[serde(default)]
        viewer: Option<ViewerMode>,
    },
    /// Move the playhead and wait until the frame is shown. Times are relative
    /// to the viewer's content (`+10f` and `-2s` are relative to the playhead).
    /// `fast` seeks to a keyframe (scrubbing).
    #[serde(rename = "player.seek")]
    PlayerSeek {
        at: TimeSpec,
        #[serde(default)]
        viewer: Option<ViewerMode>,
        #[serde(default)]
        fast: bool,
    },
    /// Step frames (negative: backwards).
    #[serde(rename = "player.step")]
    PlayerStep {
        #[serde(default = "one_frame")]
        frames: i64,
        #[serde(default)]
        viewer: Option<ViewerMode>,
    },
    /// Viewer mode, both playheads, the source clip and selection.
    #[serde(rename = "player.state")]
    PlayerState {},
    #[serde(rename = "selection.get")]
    SelectionGet {},
    /// Select timeline clips and/or pool media (omitted parts stay as they are).
    #[serde(rename = "selection.set")]
    SelectionSet {
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
        #[serde(default)]
        media: Option<OneOrMany<String>>,
    },
    /// Save a PNG of the viewer (the frame as shown) or of the whole window.
    #[serde(rename = "gui.screenshot")]
    GuiScreenshot {
        out: PathBuf,
        #[serde(default)]
        target: ScreenshotTarget,
    },
    /// The GUI's command log: recent requests from every client, oldest first.
    #[serde(rename = "gui.log")]
    GuiLog {
        /// Only entries after this sequence number.
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Change the window: inner `size` in logical pixels `[w, h]`, minimize
    /// (Wayland can't un-minimize), maximize, fullscreen.
    #[serde(rename = "gui.window")]
    GuiWindow {
        #[serde(default)]
        size: Option<[f32; 2]>,
        #[serde(default)]
        minimized: Option<bool>,
        #[serde(default)]
        maximized: Option<bool>,
        #[serde(default)]
        fullscreen: Option<bool>,
    },
    /// Close the GUI.
    #[serde(rename = "gui.quit")]
    GuiQuit {},

    // ---------------------------------------------------------------- meta
    /// JSON Schema of the protocol, or of one method's params.
    #[serde(rename = "schema")]
    Schema {
        #[serde(default)]
        method: Option<String>,
    },
    /// Report dependencies (FFmpeg, libmpv, GPUs).
    #[serde(rename = "doctor")]
    Doctor {},
}

/// What to export and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExportParams {
    /// Output file; for `per_clip`, a directory.
    pub output: PathBuf,
    /// Preset name (see `presets.list`). Default: from the output extension
    /// (`h264-master` settings for video, the matching `audio-*` preset for audio).
    #[serde(default)]
    pub preset: Option<String>,
    /// Full settings instead of a preset.
    #[serde(default)]
    pub settings: Option<ExportSettings>,
    /// Overrides like `video.quality=20` or `audio.bitrate=192000`.
    #[serde(default)]
    pub set: Vec<String>,
    /// Timeline range. Default: In–Out if both marks are set, else everything.
    #[serde(default)]
    pub range: Option<RangeSpec>,
    /// One file per clip (in the range) instead of one file.
    #[serde(default)]
    pub per_clip: bool,
    /// Leave the video out.
    #[serde(default)]
    pub audio_only: bool,
    /// Shortcut for `video.encoder`.
    #[serde(default)]
    pub encoder: Option<EncoderChoice>,
    /// Per-clip file name template: `{index}`, `{index:03}`, `{name}`, `{clip}`, `{media}`.
    #[serde(default)]
    pub name: Option<String>,
    /// Replace existing output files.
    #[serde(default)]
    pub overwrite: bool,
}

/// What the single viewer shows (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewerMode {
    #[serde(alias = "source", alias = "clip")]
    SourceClip,
    #[serde(alias = "tape")]
    SourceTape,
    Timeline,
}

impl ViewerMode {
    pub fn is_source(self) -> bool {
        self != Self::Timeline
    }
}

/// What `gui.screenshot` captures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotTarget {
    /// The viewer picture as displayed (works while the window is hidden).
    #[default]
    Viewer,
    /// The whole window (needs it to be visible).
    Window,
}

fn one_frame() -> i64 {
    1
}

fn insert_mode() -> MoveMode {
    MoveMode::Insert
}

fn one() -> u32 {
    1
}

/// What to put on the timeline: a media range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceParams {
    /// Media id (`m1`).
    pub media: String,
    /// Source range (TimeSpecs relative to the media's start). Default: all of it.
    #[serde(default)]
    pub range: Option<RangeSpec>,
    /// Leave the audio out (Video Only).
    #[serde(default)]
    pub video_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSet {
    #[serde(default)]
    pub fps: Option<Rational>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub channels: Option<u16>,
    #[serde(default)]
    pub ripple_on: Option<bool>,
    /// Record timecode of the first frame, e.g. `01:00:00:00`.
    #[serde(default)]
    pub start_tc: Option<TimeSpec>,
}

/// One value or a list (`"c1"` or `["c1", "c2"]`).
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

// Hand-written so the inner error ("number without a unit...") reaches the
// user instead of serde's generic "did not match any variant".
impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for OneOrMany<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Array(items) => items
                .into_iter()
                .map(|v| T::deserialize(v).map_err(D::Error::custom))
                .collect::<Result<_, _>>()
                .map(Self::Many),
            v => T::deserialize(v).map(Self::One).map_err(D::Error::custom),
        }
    }
}

impl<T: Clone> OneOrMany<T> {
    pub fn to_vec(&self) -> Vec<T> {
        match self {
            Self::One(v) => vec![v.clone()],
            Self::Many(v) => v.clone(),
        }
    }
}

impl Request {
    /// The JSON-RPC method name.
    pub fn method(&self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(m)) => m
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }

    /// Builds a request from a method name and optional params (JSON-RPC lets
    /// clients omit params for methods without required ones).
    pub fn from_parts(method: &str, params: Option<serde_json::Value>) -> Result<Self, String> {
        let params = params.unwrap_or_else(|| serde_json::json!({}));
        let request: Self = serde_json::from_value(
            serde_json::json!({ "method": method, "params": params.clone() }),
        )
        .map_err(|e| {
            if e.to_string().contains("unknown variant") {
                format!("unknown method {method:?}; see `vidcut schema`")
            } else {
                format!("invalid params for {method}: {e}")
            }
        })?;
        // serde can't deny unknown fields next to `flatten`, so compare keys:
        // a mistyped parameter must fail rather than be silently ignored.
        if let (Some(given), Ok(serde_json::Value::Object(parsed))) =
            (params.as_object(), serde_json::to_value(&request))
        {
            let known: Vec<&str> = parsed
                .get("params")
                .and_then(|p| p.as_object())
                .map(|p| p.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let unknown: Vec<&str> = given
                .keys()
                .map(String::as_str)
                .filter(|k| !known.contains(k))
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "unknown parameter(s) {} for {method}; see `vidcut schema {method}`",
                    unknown.join(", ")
                ));
            }
        }
        Ok(request)
    }

    /// Whether the request only makes sense in a running GUI (viewer, players,
    /// selection, window). Headless runs reject these with `NO_INSTANCE`.
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            Self::ProjectOpen { .. }
                | Self::ViewerSet { .. }
                | Self::ViewerLoad { .. }
                | Self::PlayerPlay { .. }
                | Self::PlayerPause { .. }
                | Self::PlayerToggle { .. }
                | Self::PlayerSeek { .. }
                | Self::PlayerStep { .. }
                | Self::PlayerState {}
                | Self::SelectionGet {}
                | Self::SelectionSet { .. }
                | Self::GuiScreenshot { .. }
                | Self::GuiLog { .. }
                | Self::GuiWindow { .. }
                | Self::GuiQuit {}
        )
    }

    /// Whether the request works without a project (it never needs one open).
    pub fn is_projectless(&self) -> bool {
        matches!(
            self,
            Self::Schema { .. }
                | Self::Doctor {}
                | Self::Probe { .. }
                | Self::PresetsList {}
                | Self::PresetsGet { .. }
                | Self::EncodersList { .. }
        )
    }

    /// Whether the request changes the project (and so takes part in undo).
    pub fn is_mutation(&self) -> bool {
        !self.is_live()
            && !matches!(
                self,
                Self::ProjectGet {}
                    | Self::ProjectSummary {}
                    | Self::MediaList {}
                    | Self::MediaGet { .. }
                    | Self::Probe { .. }
                    | Self::TimelineGet { .. }
                    | Self::TimelineAt { .. }
                    | Self::TimelineEditPoints {}
                    | Self::HistoryGet {}
                    | Self::Schema { .. }
                    | Self::Doctor {}
                    | Self::ExportPlan(_)
                    | Self::ExportStart(_)
                    | Self::FrameRender { .. }
                    | Self::PresetsList {}
                    | Self::PresetsGet { .. }
                    | Self::EncodersList { .. }
                    | Self::EditCopy { .. }
            )
    }
}

/// A request plus the options every mutation accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub request: Request,
    /// Fail with `CONFLICT` unless the project is at this revision.
    pub if_rev: Option<u64>,
    /// Compute the result and changes without committing them.
    pub dry_run: bool,
}

impl Envelope {
    pub fn new(request: Request) -> Self {
        Self {
            request,
            if_rev: None,
            dry_run: false,
        }
    }

    /// Parses `{"method": ..., "params": {..., "if_rev": N, "dry_run": true}}`
    /// (the options may also sit next to `method`).
    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        let serde_json::Value::Object(mut obj) = value else {
            return Err("a request must be a JSON object with \"method\"".into());
        };
        let method = obj
            .remove("method")
            .and_then(|m| m.as_str().map(str::to_string))
            .ok_or("missing \"method\"")?;
        let mut params = obj.remove("params");
        let mut take = |key: &str| -> Option<serde_json::Value> {
            obj.remove(key).or_else(|| {
                params
                    .as_mut()
                    .and_then(|p| p.as_object_mut())
                    .and_then(|p| p.remove(key))
            })
        };
        let if_rev = match take("if_rev") {
            Some(v) => Some(v.as_u64().ok_or("if_rev must be a non-negative integer")?),
            None => None,
        };
        let dry_run = match take("dry_run") {
            Some(v) => v.as_bool().ok_or("dry_run must be a boolean")?,
            None => false,
        };
        Ok(Self {
            request: Request::from_parts(&method, params)?,
            if_rev,
            dry_run,
        })
    }
}

/// Ids touched by a mutation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Changes {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

/// The reply to a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    /// Project revision after the request (unchanged for queries and dry runs).
    pub rev: u64,
    #[serde(default, skip_serializing_if = "Changes::is_empty")]
    pub changes: Changes,
    /// Method-specific result.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub result: serde_json::Value,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// Where the request ran: `headless` or `gui:<pid>`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub via: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_methods_with_and_without_params() {
        let r = Request::from_parts("project.get", None).unwrap();
        assert_eq!(r, Request::ProjectGet {});
        let r = Request::from_parts(
            "edit.append",
            Some(serde_json::json!({"media": "m1", "range": "5s..+20s"})),
        )
        .unwrap();
        assert!(matches!(r, Request::EditAppend(SourceParams { ref media, .. }) if media == "m1"));
        assert_eq!(r.method(), "edit.append");
        let r = Request::from_parts("edit.split", Some(serde_json::json!({"at": ["1m", 900]})))
            .unwrap();
        assert!(matches!(r, Request::EditSplit { at: OneOrMany::Many(ref v), .. } if v.len() == 2));
        let err = Request::from_parts("edit.frobnicate", None).unwrap_err();
        assert!(err.contains("unknown method"), "{err}");
        let err = Request::from_parts("edit.append", Some(serde_json::json!({}))).unwrap_err();
        assert!(err.contains("media"), "{err}");
    }

    #[test]
    fn envelope_takes_common_options_from_params() {
        let e = Envelope::from_json(serde_json::json!({
            "method": "edit.delete",
            "params": {"clips": "c3", "if_rev": 7, "dry_run": true}
        }))
        .unwrap();
        assert_eq!((e.if_rev, e.dry_run), (Some(7), true));
        assert!(e.request.is_mutation());
        assert!(!Request::TimelineGet { range: None }.is_mutation());
    }
}
