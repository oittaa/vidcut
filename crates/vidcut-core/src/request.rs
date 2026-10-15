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
use crate::model::{TrackClip, TrackKind};
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
    /// Create a project file. Without `fps` and a size, the first video
    /// added sets the project's format.
    #[serde(rename = "project.create")]
    ProjectCreate {
        /// The new `.vidcut` file.
        path: PathBuf,
        /// An OpenTimelineIO file to make the project from: `.otio`, an
        /// `.otioz` bundle (its media are unpacked next to it) or an `.otiod`
        /// bundle folder. A file vidcut wrote of its whole timeline gives the
        /// project it holds, and takes no format. Another editor's timeline
        /// is imported. Without a format, the project then takes the file's
        /// frame rate and start timecode, and its first video's size.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<PathBuf>,
        /// Frame rate, like `25`, `29.97` or `30000/1001`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<String>,
        /// Frame width; give the height too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        /// Frame height; give the width too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_rate: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channels: Option<u16>,
        /// Count record timecode drop-frame (29.97 and 59.94; decision D47).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drop_frame: Option<bool>,
        /// Record timecode of the first frame: `00:00:00:00` (the default),
        /// `01:00:00:00`, `10:00:00:00` or any other (deviation X7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_tc: Option<TimeSpec>,
    },

    // ---------------------------------------------------------------- media pool
    /// Add files to the media pool (probing them); returns their media ids.
    #[serde(rename = "media.add")]
    MediaAdd {
        paths: Vec<PathBuf>,
        /// Into this bin (default: the top level).
        #[serde(default)]
        bin: Option<String>,
        /// Each path is a file of a numbered image sequence, added as one clip:
        /// the files numbered in a row around it.
        #[serde(default)]
        sequence: bool,
        /// Frame rate of the image sequences (default: the project's).
        #[serde(default)]
        fps: Option<Rational>,
    },
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
    /// Point pool items at their files again (re-probed): one at `path`, or
    /// each found by name in the `search` folders and their subfolders.
    /// A file found is relinked only if it is the item's file (the same
    /// size, streams and fingerprint); the result lists the others with
    /// why. A file given as `path` is relinked either way.
    #[serde(rename = "media.relink")]
    MediaRelink {
        /// Media ids. Absent: every item whose file is missing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<OneOrMany<String>>,
        /// The file, for one item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
        /// Folders to look in by file name, subfolders too.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        search: Vec<PathBuf>,
    },
    #[serde(rename = "media.rename")]
    MediaRename { media: String, name: String },
    /// Set the Clip Color of media items, which their clips show unless
    /// they have their own.
    #[serde(rename = "media.set")]
    MediaSet {
        media: OneOrMany<String>,
        /// A color as `clip.set` takes it. Empty string clears it.
        color: String,
    },
    // Decisions D5, D6.
    /// Set a media item's source In and/or Out (I/O in a source viewer or over
    /// a pool thumbnail). Source time, `out` exclusive. Edits without a `range`
    /// use these marks.
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
    // Decisions D16, D17.
    /// Set Source Tape In and/or Out. `in` and `out` are tape frames of `order`
    /// (the media ids in tape order); `out` is exclusive. A span on one clip is
    /// stored as that clip's source marks. A span across clips is stored as
    /// tape marks and is not written onto one clip.
    #[serde(rename = "tape.marks.set")]
    TapeMarksSet {
        #[serde(rename = "in", default)]
        mark_in: Option<TimeSpec>,
        #[serde(default)]
        out: Option<TimeSpec>,
        /// Media ids in the current tape order.
        order: Vec<String>,
    },
    /// Clear Source Tape marks (Alt+X in Source Tape). With `media`, also
    /// clear that clip's source marks when the span does not cross clips.
    #[serde(rename = "tape.marks.clear")]
    TapeMarksClear {
        #[serde(default)]
        media: Option<String>,
    },
    /// Put media into a bin (`""`: the top level).
    #[serde(rename = "media.move")]
    MediaMove {
        media: OneOrMany<String>,
        bin: String,
    },
    // Decision D41.
    /// Write playback proxies for video and image sequences. With no `media`,
    /// every such item in the pool. Stills and audio-only files are skipped
    /// then; naming one is an error.
    #[serde(rename = "proxy.generate")]
    ProxyGenerate {
        /// Media ids. Absent: every video and image sequence.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<OneOrMany<String>>,
        /// Longest side in pixels, even, from 16 to 7680. Default: the
        /// preferences' proxy size (960 unless changed). Pictures already
        /// smaller stay their size.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        /// Only the media the automatic proxy rules pick (preferences
        /// `proxies`) that have no fresh proxy, of `media` or of the pool;
        /// none is fine.
        #[serde(default)]
        auto: bool,
    },
    /// Delete proxy files and clear them from the pool. With no `media`,
    /// every item that has one.
    #[serde(rename = "proxy.remove")]
    ProxyRemove {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<OneOrMany<String>>,
    },
    /// The Media Pool's bins.
    #[serde(rename = "bins.list")]
    BinsList {},
    /// Add a Media Pool bin, like `Interviews/Day 1`; missing parents are added
    /// too.
    #[serde(rename = "bins.add")]
    BinsAdd { path: String },
    /// Remove a bin and the bins inside it; their media move to its parent.
    #[serde(rename = "bins.remove")]
    BinsRemove { path: String },
    /// Rename or move a bin, with everything inside it.
    #[serde(rename = "bins.rename")]
    BinsRename { path: String, to: String },
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
    /// The Boring Detector: clips on video tracks longer than `boring`, and
    /// shorter than `jump` (jump cuts, flash frames).
    #[serde(rename = "timeline.boring")]
    TimelineBoring {
        #[serde(default = "boring_length")]
        boring: TimeSpec,
        #[serde(default = "jump_length")]
        jump: TimeSpec,
    },

    // ---------------------------------------------------------------- analysis
    /// Silent stretches (as Ripple Delete Silence finds them) in `media`
    /// (source frames), in `clips` (timeline frames), or in every clip with
    /// sound.
    #[serde(rename = "analyze.silence")]
    AnalyzeSilence {
        #[serde(default)]
        media: Option<String>,
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
        /// Sound below this level (dBFS) is silence.
        #[serde(default = "silence_threshold")]
        threshold_db: f64,
        /// The shortest silence reported.
        #[serde(default = "silence_min")]
        min: TimeSpec,
    },
    /// Shot changes (as Detect Scene Cuts finds them) in `media`, in `clips`,
    /// or in every clip with video: the first frame of each new shot with its
    /// scene score.
    #[serde(rename = "analyze.scenes")]
    AnalyzeScenes {
        #[serde(default)]
        media: Option<String>,
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
        /// Scene score (0..100) from which a frame starts a new shot.
        #[serde(default = "scene_threshold")]
        threshold: f64,
    },
    /// Black stretches in `media`, in `clips`, or in every clip with video.
    #[serde(rename = "analyze.black")]
    AnalyzeBlack {
        #[serde(default)]
        media: Option<String>,
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
        /// Share of a frame's pixels that must be black (0..1).
        #[serde(default = "black_share")]
        share: f64,
        /// The shortest stretch reported.
        #[serde(default = "black_min")]
        min: TimeSpec,
    },
    /// Loudness of the timeline's mix (EBU R128): integrated LUFS, range and
    /// true peak, over `range` (default: In–Out if set, else everything).
    #[serde(rename = "analyze.loudness")]
    AnalyzeLoudness {
        #[serde(default)]
        range: Option<RangeSpec>,
    },
    // Deviation X9.
    /// Import an OpenTimelineIO timeline: an `.otio` file, an `.otioz` bundle
    /// (its media are unpacked first) or an `.otiod` bundle folder, as other
    /// editors write them. Its media join the pool, and its tracks go on V1,
    /// V2, … and A1, A2, … as `mode` says. An empty timeline takes the file's
    /// frame rate and start timecode. The media of a project vidcut wrote
    /// come as it has them, in a bin named after the file. What can't be
    /// imported is listed in `warnings`.
    #[serde(rename = "timeline.import")]
    TimelineImport {
        /// The `.otio`, `.otioz` or `.otiod` file, read by the process that
        /// runs the request.
        #[serde(default)]
        path: Option<PathBuf>,
        /// The OTIO document itself, instead of `path`.
        #[serde(default)]
        otio: Option<serde_json::Value>,
        #[serde(default)]
        mode: ImportMode,
        /// Directories to look in, by file name, for media missing at its path.
        #[serde(default)]
        search: Vec<PathBuf>,
        /// Where an `.otioz` bundle's media are unpacked (default: a folder
        /// named after the bundle, next to it). A dry run doesn't unpack.
        #[serde(default)]
        unpack: Option<PathBuf>,
        /// Decompose the compound clips it holds, and those they hold, into
        /// their clips (`edit.decompose`).
        #[serde(default)]
        flatten: bool,
    },

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
        // Spec §7.3.
        /// `false` ignores the timeline In/Out and writes at `at` (drag and
        /// drop places by position).
        #[serde(default = "yes")]
        use_marks: bool,
    },
    /// Ripple Overwrite (Shift+F10): replace the In–Out range or the clip at `at`.
    #[serde(rename = "edit.ripple_overwrite")]
    EditRippleOverwrite {
        #[serde(flatten)]
        source: SourceParams,
        #[serde(default)]
        at: Option<TimeSpec>,
        /// `false` ignores the timeline In/Out and replaces the clip at `at`.
        #[serde(default = "yes")]
        use_marks: bool,
    },

    /// Place On Top (F12): on the first track above every clip in its span
    /// (made if needed), never rippling. Lines up with the timeline In, a lone
    /// Out, the edit nearest `at` (within 5 frames), or `at`. Returns the clip
    /// and its track.
    #[serde(rename = "edit.place_on_top")]
    EditPlaceOnTop {
        #[serde(flatten)]
        source: SourceParams,
        #[serde(default)]
        at: Option<TimeSpec>,
        /// `false` ignores the timeline In/Out.
        #[serde(default = "yes")]
        use_marks: bool,
    },

    // ---------------------------------------------------------------- timeline editing (spec §8)
    /// Split at one or more frames (Ctrl+\).
    #[serde(rename = "edit.split")]
    EditSplit {
        at: OneOrMany<TimeSpec>,
        // Decision D2.
        /// Only split these clips.
        #[serde(default)]
        clips: Vec<ClipRef>,
    },
    /// Join Match Edit Clips: clips that a split left join back into one.
    /// Two or more `clips` join where they meet. Otherwise the clips that
    /// meet at `at` (default: the playhead) join, only the given ones if
    /// any are there (decision D49).
    #[serde(rename = "edit.join")]
    EditJoin {
        /// Where the clips meet. Default: the playhead.
        #[serde(default)]
        at: Option<TimeSpec>,
        /// The clips to join.
        #[serde(default)]
        clips: Vec<ClipRef>,
    },
    /// The Dissolve button and Ctrl+T (decision D50): a Cross Dissolve on
    /// the edit of `track` (default V1) nearest `at` (default: the
    /// playhead), and a Cross Fade +3 dB where its clips' sounds meet
    /// there. `length` (default 1 s) is shortened to the frames its clips'
    /// media and the track leave. `align`: centered on the edit, starting
    /// on it or ending on it. `picture` or `sound` false leaves that one
    /// out. `all`: on every edit between two clips of the track (Add
    /// Dissolve to All Edits). Transitions on those edits go.
    #[serde(rename = "edit.dissolve")]
    EditDissolve {
        #[serde(default)]
        at: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
        #[serde(default)]
        length: Option<TimeSpec>,
        #[serde(default)]
        align: crate::ops::Align,
        #[serde(default = "yes")]
        picture: bool,
        #[serde(default = "yes")]
        sound: bool,
        #[serde(default)]
        all: bool,
    },
    /// The Cut button (decision D50): the edit of `track` (default V1)
    /// nearest `at` (default: the playhead) becomes a cut again, its
    /// transitions gone, those of its clips' sounds too. `picture` or
    /// `sound` false keeps those. `all`: every transition of the track
    /// (Clear Transition from All Edits).
    #[serde(rename = "edit.remove_transition")]
    EditRemoveTransition {
        #[serde(default)]
        at: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
        #[serde(default = "yes")]
        picture: bool,
        #[serde(default = "yes")]
        sound: bool,
        #[serde(default)]
        all: bool,
    },
    /// Delete clips. On V1 it ripples unless `lift`, and the other tracks
    /// follow; elsewhere gaps close only where V1 has nothing.
    #[serde(rename = "edit.delete")]
    EditDelete {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        lift: bool,
    },
    /// Delete a range on every unlocked track; ripples unless `lift`.
    #[serde(rename = "edit.delete_range")]
    EditDeleteRange {
        range: RangeSpec,
        #[serde(default)]
        lift: bool,
    },
    /// Close the gap containing `at`.
    #[serde(rename = "edit.close_gap")]
    EditCloseGap { at: TimeSpec },
    /// Move clips to an edit point (`insert`) or over whatever is there
    /// (`overwrite`). `to` is where the first clip starts, in the timeline as
    /// it is before the move; `track` moves them to another track (the next
    /// new track is made). Only V1 inserts; elsewhere clips overwrite.
    #[serde(rename = "edit.move")]
    EditMove {
        clips: OneOrMany<ClipRef>,
        to: TimeSpec,
        #[serde(default = "insert_mode")]
        mode: MoveMode,
        #[serde(default)]
        track: Option<String>,
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
    /// Decompose in Place: a compound clip from another editor (deviation
    /// X9) becomes the clips it holds, at their times, trimmed to the part
    /// it shows. A picture block brings its pictures: its first track in
    /// the block's place, each other on the next video track above that is
    /// free there. A sound block brings its sound the same way on audio
    /// tracks.
    #[serde(rename = "edit.decompose")]
    EditDecompose { clips: OneOrMany<ClipRef> },
    /// Slide a clip (Dynamic Trim's Slide mode): it keeps its media and moves,
    /// the clip before it getting longer and the one after it shorter (or the
    /// reverse).
    #[serde(rename = "edit.slide")]
    EditSlide { clip: ClipRef, by: TimeSpec },
    /// Ripple Delete Silence: cut the silent parts out of `clips` and close the
    /// gaps, as one undo step. `dry_run` reports the parts without cutting (the
    /// red stripes).
    #[serde(rename = "edit.ripple_delete_silence")]
    EditRippleDeleteSilence {
        clips: OneOrMany<ClipRef>,
        /// Sound below this level (dBFS) is silence.
        #[serde(default = "silence_threshold")]
        threshold_db: f64,
        /// Silence kept before each audible part ("pre head").
        #[serde(default = "silence_margin")]
        pre_head: TimeSpec,
        /// Silence kept after each audible part ("post tail").
        #[serde(default = "silence_margin")]
        post_tail: TimeSpec,
        /// The shortest silence removed ("minimum to trip").
        #[serde(default = "silence_min")]
        min: TimeSpec,
    },
    /// Detect Scene Cuts: split `clips` where their shots change.
    #[serde(rename = "edit.detect_scene_cuts")]
    EditDetectSceneCuts {
        clips: OneOrMany<ClipRef>,
        /// Scene score (0..100) from which a frame starts a new shot.
        #[serde(default = "scene_threshold")]
        threshold: f64,
    },
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
    /// Change Clip Duration (Ctrl+D). `end` runs each clip to the end of its
    /// media.
    #[serde(rename = "edit.change_duration")]
    EditChangeDuration {
        clips: OneOrMany<ClipRef>,
        duration: TimeSpec,
        #[serde(default)]
        ripple: Option<bool>,
    },
    /// Change Clip Speed (R; decision D48): each clip plays at `speed`
    /// (2.0 twice as fast, 0.5 half, -1.0 backward, 0 a freeze frame). With
    /// ripple (Ripple On unless overridden) it keeps its source and its
    /// length changes, later clips following; without, it keeps its length.
    /// A freeze holds the frame at `at` (the playhead) when it is over the
    /// clip, else its first.
    #[serde(rename = "edit.change_speed")]
    EditChangeSpeed {
        clips: OneOrMany<ClipRef>,
        speed: f64,
        #[serde(default)]
        ripple: Option<bool>,
        #[serde(default)]
        at: Option<TimeSpec>,
    },
    /// Copy clips (Ctrl+C): returns them with their tracks as `clips`,
    /// ready for `edit.paste`. The GUI also keeps them on its clipboard.
    #[serde(rename = "edit.copy")]
    EditCopy { clips: OneOrMany<ClipRef> },
    /// Cut clips (Ctrl+X): copy, then delete with ripple on Track 1.
    #[serde(rename = "edit.cut")]
    EditCut { clips: OneOrMany<ClipRef> },
    /// Paste clips (Ctrl+V) at `at`, each over its own track, keeping their
    /// spacing. Defaults: the GUI's clipboard and playhead.
    #[serde(rename = "edit.paste")]
    EditPaste {
        #[serde(default)]
        clips: Option<Vec<TrackClip>>,
        #[serde(default)]
        at: Option<TimeSpec>,
    },

    // ---------------------------------------------------------------- tracks, clips, marks, markers
    /// The tracks, with their header and mixer settings and how many clips
    /// each holds.
    #[serde(rename = "track.list")]
    TrackList {},
    /// Add tracks (Timeline Actions > Add Track): video tracks go on top, audio
    /// tracks at the bottom. Returns their ids.
    #[serde(rename = "track.add")]
    TrackAdd {
        kind: TrackKind,
        #[serde(default = "one")]
        count: u32,
    },
    /// Remove a track; one with clips needs `force`. V1 stays. Tracks above
    /// (or below) it are renumbered.
    #[serde(rename = "track.remove")]
    TrackRemove {
        track: String,
        #[serde(default)]
        force: bool,
    },
    /// Track header toggles (lock, mute, video on or off) and the mixer (solo,
    /// volume in dB).
    #[serde(rename = "track.set")]
    TrackSet {
        track: String,
        #[serde(default)]
        locked: Option<bool>,
        #[serde(default)]
        muted: Option<bool>,
        #[serde(default)]
        solo: Option<bool>,
        #[serde(default)]
        volume_db: Option<f64>,
        #[serde(default)]
        video_enabled: Option<bool>,
        /// Empty string clears the name.
        #[serde(default)]
        name: Option<String>,
    },
    /// Enable/disable video (D), mute, gain (dB, -60..24), name and Clip
    /// Color.
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
        /// Clip Color: orange, apricot, yellow, lime, olive, green, teal,
        /// navy, blue, purple, violet, pink, tan, beige, brown or
        /// chocolate. Empty string clears it, and the clip shows its
        /// media's color.
        #[serde(default)]
        color: Option<String>,
    },
    /// The timeline's In and Out marks.
    #[serde(rename = "marks.get")]
    MarksGet {},
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
    /// The timeline's markers.
    #[serde(rename = "markers.list")]
    MarkersList {},
    #[serde(rename = "markers.add")]
    MarkersAdd {
        at: TimeSpec,
        #[serde(default)]
        name: String,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        note: String,
        /// Its length (`25f`, `1s`); one frame by default. The timeline
        /// shows it as a point; OTIO files keep it (spec D40).
        #[serde(default)]
        duration: Option<TimeSpec>,
    },
    #[serde(rename = "markers.move")]
    MarkersMove { marker: String, to: TimeSpec },
    /// Change a marker's name, color, note or length (M on a marker).
    #[serde(rename = "markers.set")]
    MarkersSet {
        marker: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        note: Option<String>,
        /// Its length (`25f`, `1s`).
        #[serde(default)]
        duration: Option<TimeSpec>,
    },
    #[serde(rename = "markers.remove")]
    MarkersRemove { marker: String },

    // ---------------------------------------------------------------- export (spec §10)
    /// Plan an export without encoding: settings, encoder, outputs, segments.
    #[serde(rename = "export.plan")]
    ExportPlan(ExportParams),
    /// Export. Quick Export semantics: the timeline In–Out range when both
    /// marks are set, otherwise the whole timeline.
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
    /// A storyboard: one PNG with a picture of every shot (or every
    /// `every`), labeled with timecode and clip, to look at an edit at a
    /// glance. Over `range` (default: everything).
    #[serde(rename = "storyboard.render")]
    StoryboardRender {
        out: PathBuf,
        /// Pictures per row.
        #[serde(default = "storyboard_columns")]
        columns: u32,
        /// Width of each picture.
        #[serde(default = "storyboard_width")]
        width: u32,
        /// A picture every this often instead of one per shot.
        #[serde(default)]
        every: Option<TimeSpec>,
        #[serde(default)]
        range: Option<RangeSpec>,
    },
    /// Built-in and user export presets.
    #[serde(rename = "presets.list")]
    PresetsList {},
    #[serde(rename = "presets.get")]
    PresetsGet { name: String },
    /// Save a user preset (`~/.config/vidcut/presets/NAME.toml`, on Windows
    /// in `%APPDATA%\vidcut`, on macOS in `~/Library/Application
    /// Support/vidcut`): `settings`, or preset `from` with `set` overrides.
    /// `quick_export` puts it in Quick Export.
    #[serde(rename = "presets.save")]
    PresetsSave {
        /// File-name safe: letters, digits, `-`, `_` and `.`.
        name: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        quick_export: bool,
        #[serde(default)]
        settings: Option<ExportSettings>,
        /// Start from this preset's settings.
        #[serde(default)]
        from: Option<String>,
        /// Overrides like `video.quality=20`.
        #[serde(default)]
        set: Vec<String>,
        /// Replace a user preset of that name (or shadow a built-in one).
        #[serde(default)]
        overwrite: bool,
    },
    /// Delete a user preset.
    #[serde(rename = "presets.delete")]
    PresetsDelete { name: String },
    /// Which GPU encoders work here (cached; `refresh` re-tests).
    #[serde(rename = "encoders.list")]
    EncodersList {
        #[serde(default)]
        refresh: bool,
    },
    /// Which codecs each GPU decoder here decodes: a short clip of each,
    /// made once and kept, decoded through on each.
    #[serde(rename = "decoders.list")]
    DecodersList {},
    /// Application preferences (`~/.config/vidcut/preferences.toml`, on
    /// Windows in `%APPDATA%\vidcut`, on macOS in `~/Library/Application
    /// Support/vidcut`) and what the thumbnail cache holds.
    #[serde(rename = "preferences.get")]
    PreferencesGet {},
    /// Change application preferences; what is left out stays. Turning the
    /// thumbnail cache off empties it, and a lower limit evicts at once.
    #[serde(rename = "preferences.set")]
    PreferencesSet(crate::preferences::PreferencesPatch),
    /// The keyboard shortcuts, one action each, with a few plain words
    /// where the action's name doesn't say what it does (spec §11). Ctrl
    /// and Alt are Cmd and Option on macOS.
    #[serde(rename = "keys.list")]
    KeysList {},

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
    /// How many steps there are to undo and redo (`undo_steps`,
    /// `redo_steps`), and the newest 100 of each by label, newest first.
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
    /// Open (or with `create`, create) a project in the running GUI. An
    /// OpenTimelineIO file opens as a new project file made from it, as
    /// `project.create` with `from` makes it.
    #[serde(rename = "project.open")]
    ProjectOpen {
        path: PathBuf,
        #[serde(default)]
        create: bool,
        /// With `create`, replace a file already there with the new project
        /// (and fresh history) instead of opening it.
        #[serde(default)]
        force: bool,
        /// For an OpenTimelineIO file: the project file to make (default:
        /// next to it, named alike, like `cut.vidcut` for `cut.otio`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        save_as: Option<PathBuf>,
        /// With `create` or an OpenTimelineIO file, the new project's frame
        /// rate, like `25`, `29.97` or `30000/1001`. Without a rate and a
        /// size, the first video added sets the format.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<String>,
        /// With `create` or an OpenTimelineIO file, the new project's frame
        /// width; give the height too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        /// With `create` or an OpenTimelineIO file, the new project's frame
        /// height; give the width too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        /// With `create` or an OpenTimelineIO file, count record timecode
        /// drop-frame (29.97 and 59.94; decision D47).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drop_frame: Option<bool>,
        /// With `create` or an OpenTimelineIO file, the record timecode of
        /// the first frame (deviation X7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_tc: Option<TimeSpec>,
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
    /// `fast` marks a scrub (a drag of the playhead): it is left out of the
    /// command log. Every seek shows its exact frame.
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
    /// Mute what the viewer plays, without changing the project (the monitoring
    /// mute next to the meter). Without `mute`, toggles.
    #[serde(rename = "player.mute")]
    PlayerMute {
        #[serde(default)]
        mute: Option<bool>,
    },
    /// Stats for nerds: a panel over the viewer of what its player does
    /// (the picture, its decoder, pictures kept, the sound, the renderer).
    /// Without `show`, toggles. `player.state` has the same as `stats`.
    #[serde(rename = "player.stats")]
    PlayerStats {
        #[serde(default)]
        show: Option<bool>,
    },
    /// Audio scrubbing (decision D42): frame steps and moves of a paused
    /// playhead play the sound of the frame arrived at. On by default; not
    /// saved with the project. Without `on`, toggles (Shift+S).
    #[serde(rename = "player.audio_scrubbing")]
    PlayerAudioScrubbing {
        #[serde(default)]
        on: Option<bool>,
    },
    /// Hear `track`'s fader at `volume_db` in the timeline preview at once,
    /// without changing the project: what the Mixer's fader does while it is
    /// dragged; `track.set` keeps a level. `live: false` in the result:
    /// nothing plays the timeline now. A track with no sound in the
    /// timeline's mix (muted, left out by solo, or empty) is an error.
    #[serde(rename = "mixer.preview")]
    MixerPreview { track: String, volume_db: f64 },
    /// The Media Pool view: the bin shown, the sort order and how it shows
    /// its items. Source Tape strings together that bin's media, and its
    /// bins', in that order.
    #[serde(rename = "pool.set")]
    PoolSet {
        #[serde(default)]
        bin: Option<String>,
        #[serde(default)]
        sort: Option<PoolSort>,
        #[serde(default)]
        descending: Option<bool>,
        #[serde(default)]
        view: Option<PoolMode>,
    },
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
    /// Open a window for the person at the GUI: `quick_export`, `export`,
    /// `jobs`, `log`, `mixer`, `preferences` or `help` (the keyboard
    /// shortcuts).
    #[serde(rename = "gui.show")]
    GuiShow { panel: GuiPanel },
    /// The GUI's export jobs, oldest first: state, progress, ETA, result.
    #[serde(rename = "jobs.list")]
    JobsList {},
    #[serde(rename = "jobs.get")]
    JobsGet { job: String },
    /// Cancel a running job; its partial output is removed.
    #[serde(rename = "jobs.cancel")]
    JobsCancel { job: String },
    /// Wait until a job ends (or `timeout` seconds pass) and return it.
    #[serde(rename = "jobs.wait")]
    JobsWait {
        job: String,
        #[serde(default)]
        timeout: Option<f64>,
    },

    // ---------------------------------------------------------------- meta
    /// JSON Schema of the protocol, or of one method's params.
    #[serde(rename = "schema")]
    Schema {
        #[serde(default)]
        method: Option<String>,
    },
    /// Report dependencies (FFmpeg and the parts of it this build lacks,
    /// the audio device, GPUs).
    #[serde(rename = "doctor")]
    Doctor {},
}

/// What to export and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExportParams {
    // Deviation X9.
    /// Output file; for `per_clip`, a directory. A timeline for other editors
    /// instead of media: `.otio` (the edit), `.otioz` (a zip bundle of the edit
    /// and its media) or `.otiod` (the same as a folder).
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
    /// Return at once with a job id instead of waiting (running GUI only;
    /// see `jobs.*`).
    #[serde(default)]
    pub background: bool,
}

/// Where `timeline.import` puts the timeline (deviation X9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Its tracks after the last clip.
    #[default]
    Append,
    /// Its tracks instead of the timeline's clips, markers and marks.
    Replace,
    /// One compound clip after the last clip, holding its tracks.
    Compound,
    /// Only its media, into the pool.
    Media,
}

// Spec §4.1.
/// What the single viewer shows.
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

/// A window `gui.show` opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuiPanel {
    /// Quick Export.
    #[serde(alias = "quick-export")]
    QuickExport,
    /// The Export dialog.
    Export,
    /// Export jobs with their progress.
    Jobs,
    /// The command log.
    Log,
    /// The Mixer: track faders, mute, solo and meters.
    Mixer,
    /// Application preferences.
    Preferences,
    /// The keyboard shortcuts.
    Help,
}

/// Media Pool sort orders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PoolSort {
    /// Import order.
    #[default]
    Added,
    Name,
    Duration,
    /// The file's modification time.
    Date,
}

/// How the Media Pool shows its items (spec §3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PoolMode {
    /// Pictures in a grid (the Grid button).
    #[default]
    Thumbnail,
    /// A strip of each one's pictures, as wide as the pool.
    Filmstrip,
    /// A row for each, its details in columns.
    List,
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

fn yes() -> bool {
    true
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

// Defaults for the analysis tools (spec decisions D29 and D30).

fn silence_threshold() -> f64 {
    -40.0
}

fn silence_margin() -> TimeSpec {
    TimeSpec::Seconds(0.15)
}

fn silence_min() -> TimeSpec {
    TimeSpec::Seconds(0.5)
}

fn scene_threshold() -> f64 {
    10.0
}

fn black_share() -> f64 {
    0.98
}

fn black_min() -> TimeSpec {
    TimeSpec::Seconds(0.5)
}

fn boring_length() -> TimeSpec {
    TimeSpec::Seconds(30.0)
}

fn jump_length() -> TimeSpec {
    TimeSpec::Frames(10)
}

fn storyboard_columns() -> u32 {
    4
}

fn storyboard_width() -> u32 {
    320
}

/// What to put on the timeline: a media range, and where.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceParams {
    /// Media id (`m1`). Omitted when `tape` is set.
    #[serde(default)]
    pub media: String,
    // Decision D17.
    /// Edit the Source Tape In–Out span instead of one media item. Both marks
    /// must be set.
    #[serde(default)]
    pub tape: bool,
    /// Source range (TimeSpecs relative to the media's start). Default: the
    /// media's source marks, or all of it.
    #[serde(default)]
    pub range: Option<RangeSpec>,
    /// Leave the audio out (Video Only).
    #[serde(default)]
    pub video_only: bool,
    /// Leave the video out (Audio Only).
    #[serde(default)]
    pub audio_only: bool,
    // Spec §7.
    /// The target track. Default: V1, or A1 for audio-only clips. The next new
    /// track (`V3` when there are two) is made. On an audio track only the
    /// audio is used.
    #[serde(default)]
    pub track: Option<String>,
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
    /// Count record timecode drop-frame: at 29.97 and 59.94 (decision D47).
    #[serde(default)]
    pub drop_frame: Option<bool>,
    /// Record timecode of the first frame, e.g. `01:00:00:00` (`01:00:00;00`
    /// drop-frame).
    #[serde(default)]
    pub start_tc: Option<TimeSpec>,
    // Decision D38.
    /// How long a still lasts when edited without a range, e.g. `5s`.
    #[serde(default)]
    pub still_duration: Option<TimeSpec>,
    // Decision D41.
    /// Play fresh proxies in the preview. Export still reads the originals.
    #[serde(default)]
    pub use_proxies: Option<bool>,
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
        // a mistyped parameter must fail rather than be silently ignored. A
        // null is no value: an optional parameter left out on the way back.
        if let (Some(given), Ok(serde_json::Value::Object(parsed))) =
            (params.as_object(), serde_json::to_value(&request))
        {
            let known: Vec<&str> = parsed
                .get("params")
                .and_then(|p| p.as_object())
                .map(|p| p.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let unknown: Vec<&str> = given
                .iter()
                .filter(|(k, v)| !v.is_null() && !known.contains(&k.as_str()))
                .map(|(k, _)| k.as_str())
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

    /// Whether the request only makes sense in a running GUI (viewer, player,
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
                | Self::PlayerMute { .. }
                | Self::PlayerStats { .. }
                | Self::PlayerAudioScrubbing { .. }
                | Self::MixerPreview { .. }
                | Self::PoolSet { .. }
                | Self::SelectionGet {}
                | Self::SelectionSet { .. }
                | Self::GuiScreenshot { .. }
                | Self::GuiLog { .. }
                | Self::GuiWindow { .. }
                | Self::GuiQuit {}
                | Self::GuiShow { .. }
                | Self::JobsList {}
                | Self::JobsGet { .. }
                | Self::JobsCancel { .. }
                | Self::JobsWait { .. }
        )
    }

    /// Whether the request works without a project (it never needs one open).
    pub fn is_projectless(&self) -> bool {
        matches!(
            self,
            Self::Schema { .. }
                | Self::Doctor {}
                | Self::Probe { .. }
                | Self::ProjectCreate { .. }
                | Self::PresetsList {}
                | Self::PresetsGet { .. }
                | Self::PresetsSave { .. }
                | Self::PresetsDelete { .. }
                | Self::EncodersList { .. }
                | Self::DecodersList {}
                | Self::PreferencesGet {}
                | Self::PreferencesSet(_)
                | Self::KeysList {}
        )
    }

    /// Whether the request changes the project (and so takes part in undo).
    pub fn is_mutation(&self) -> bool {
        !self.is_live()
            && !matches!(
                self,
                Self::ProjectGet {}
                    | Self::ProjectSummary {}
                    | Self::ProjectCreate { .. }
                    | Self::TrackList {}
                    | Self::BinsList {}
                    | Self::MarksGet {}
                    | Self::MarkersList {}
                    | Self::MediaList {}
                    | Self::MediaGet { .. }
                    | Self::Probe { .. }
                    | Self::TimelineGet { .. }
                    | Self::TimelineAt { .. }
                    | Self::TimelineEditPoints {}
                    | Self::TimelineBoring { .. }
                    | Self::AnalyzeSilence { .. }
                    | Self::AnalyzeScenes { .. }
                    | Self::AnalyzeBlack { .. }
                    | Self::AnalyzeLoudness { .. }
                    | Self::StoryboardRender { .. }
                    | Self::HistoryGet {}
                    | Self::Schema { .. }
                    | Self::Doctor {}
                    | Self::ExportPlan(_)
                    | Self::ExportStart(_)
                    | Self::FrameRender { .. }
                    | Self::PresetsList {}
                    | Self::PresetsGet { .. }
                    | Self::PresetsSave { .. }
                    | Self::PresetsDelete { .. }
                    | Self::EncodersList { .. }
                    | Self::DecodersList {}
                    | Self::PreferencesGet {}
                    | Self::PreferencesSet(_)
                    | Self::KeysList {}
                    | Self::EditCopy { .. }
            )
    }

    /// The file paths the request names, those of `apply`'s commands too.
    /// Clients make them absolute before sending: a GUI that answers runs
    /// in a directory of its own.
    pub fn paths_mut(&mut self) -> Vec<&mut PathBuf> {
        match self {
            Self::MediaAdd { paths, .. } => paths.iter_mut().collect(),
            Self::MediaRelink { path, search, .. } => {
                path.iter_mut().chain(search.iter_mut()).collect()
            }
            Self::Probe { path } => vec![path],
            Self::ProjectOpen { path, save_as, .. } => {
                std::iter::once(path).chain(save_as.iter_mut()).collect()
            }
            Self::ProjectCreate { path, from, .. } => {
                std::iter::once(path).chain(from.iter_mut()).collect()
            }
            Self::TimelineImport {
                path,
                search,
                unpack,
                ..
            } => path
                .iter_mut()
                .chain(search.iter_mut())
                .chain(unpack.iter_mut())
                .collect(),
            Self::ExportPlan(p) | Self::ExportStart(p) => vec![&mut p.output],
            Self::FrameRender { out, .. }
            | Self::StoryboardRender { out, .. }
            | Self::GuiScreenshot { out, .. } => vec![out],
            Self::Apply { commands, .. } => commands.iter_mut().flat_map(Self::paths_mut).collect(),
            _ => Vec::new(),
        }
    }
}

/// A request plus the options every mutation accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub request: Request,
    /// Fail with `CONFLICT` unless the project is at this revision: for
    /// edits, and for what reads the project (queries, exports, frames).
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

    /// The request as it runs: a dry-run `export.start` is `export.plan`,
    /// the plan without the export, whoever sends it.
    pub fn normalized(mut self) -> Self {
        if self.dry_run
            && let Request::ExportStart(params) = self.request
        {
            self.request = Request::ExportPlan(params);
        }
        self
    }

    /// `CONFLICT` if `if_rev` names another revision than `rev`, the
    /// project's. Requests without a project pass.
    pub fn check_rev(&self, rev: u64) -> crate::Result<()> {
        match self.if_rev {
            Some(want) if want != rev && !self.request.is_projectless() => {
                Err(crate::Error::conflict(format!(
                    "project is at revision {rev}, not {want}; re-read it and retry"
                )))
            }
            _ => Ok(()),
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
    /// Problems that did not fail the request. An undo history that could
    /// not be written is one: the edit is saved, and the text says so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_given_is_known() {
        use serde_json::json;
        // What a client may send for leaving things at their defaults: a
        // flag that is false, an optional parameter that is null.
        for params in [
            json!({ "auto": false }),
            json!({ "media": null, "width": null, "auto": false }),
            json!({ "media": ["m1"], "auto": true }),
        ] {
            Request::from_parts("proxy.generate", Some(params.clone()))
                .unwrap_or_else(|e| panic!("{params}: {e}"));
        }
        // A mistyped parameter fails, whatever its value.
        for params in [json!({ "autoo": true }), json!({ "autoo": false })] {
            let e = Request::from_parts("proxy.generate", Some(params)).unwrap_err();
            assert!(e.contains("unknown parameter(s) autoo"), "{e}");
        }
    }

    #[test]
    fn paths_include_those_of_batched_commands() {
        let mut r = Request::from_parts(
            "apply",
            Some(serde_json::json!({"commands": [
                {"method": "media.add", "params": {"paths": ["a.mp4", "b.mp4"]}},
                {"method": "edit.split", "params": {"at": "1s"}},
                {"method": "timeline.import", "params": {"path": "c.otio", "search": ["d"]}},
            ]})),
        )
        .unwrap();
        let paths: Vec<_> = r.paths_mut().into_iter().map(|p| p.clone()).collect();
        assert_eq!(paths, ["a.mp4", "b.mp4", "c.otio", "d"].map(PathBuf::from));
    }
}
