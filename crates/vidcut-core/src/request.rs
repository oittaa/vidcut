use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::export::{EncoderChoice, ExportSettings};
use crate::looks::{LookKind, SubtitleLook, TitleLook};
use crate::model::{
    DuckDetect, DynamicZoom, Ease, Handle, PictureTool, PictureTransition, Place, SubtitleStyle,
    TitleAlign, TitleKind, TitleStyle, TrackClip, ZoomBox,
};
use crate::ops::{Edge, MoveMode};
use crate::time::{DurationSpec, RangeSpec, Rational, TimeSpec};

pub type ClipRef = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Request {
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

    /// Add files to the media pool (probing them); returns their media ids.
    /// A subtitle file (SRT, WebVTT, TTML) becomes a subtitle item, with no
    /// pictures or sound, for `subtitles.import` with `media`.
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
    /// Bring a folder in as a bin, with a bin inside it for each folder in it.
    /// Subtitle files come in too, as with `media.add`. Files that aren't
    /// media are skipped and listed; returns the bin, the media ids and the
    /// skipped files.
    #[serde(rename = "media.add_folder")]
    MediaAddFolder {
        path: PathBuf,
        /// Put the folder's bin into this bin (default: the top level).
        #[serde(default)]
        bin: Option<String>,
        /// Numbered images, two or more in a row, come in as one clip each,
        /// as with media.add's sequence; without it, each image is a still.
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
    /// What a pool item's file says about itself, read from it again: the
    /// container's format, bit rate and tags (creation time, camera make and
    /// model, encoder), and each stream's codec and profile, bit rate, pixel
    /// format, bits, chroma subsampling, color tags, sample rate, channel
    /// layout, language and tags. An image sequence answers for its first
    /// picture.
    #[serde(rename = "media.details")]
    MediaDetails { media: String },
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
    /// Transcribe Audio: hear each pool item's own sound (`stream`, counted
    /// from 0) and keep its words, with their times, with the media, for
    /// `analyze.words` with `media` or `clips` and the Transcription window.
    /// A transcript kept from before is used unless `again`, or another
    /// model or another `language` made it, when the new one replaces it.
    /// Only a model that times words is kept: Parakeet Ultra; the others are
    /// refused. It runs as a job. A transcript is kept with vidcut's other
    /// measurements of the file, so a changed file has none.
    #[serde(rename = "media.transcribe")]
    MediaTranscribe {
        media: OneOrMany<String>,
        #[serde(default)]
        stream: u32,
        /// Default: `parakeet-ultra`.
        #[serde(default)]
        model: Option<SpeechModel>,
        /// A transcribe.cpp GGUF model file to use instead of `model`'s.
        #[serde(default)]
        model_file: Option<PathBuf>,
        /// A Silero VAD model file (GGUF, or whisper.cpp's `.bin`) to find
        /// the speech with, instead of the one built into vidcut.
        #[serde(default)]
        vad_file: Option<PathBuf>,
        /// The language spoken, as a code such as `en` or `fi`. Default: the
        /// model tells it from the longest stretch of speech.
        #[serde(default)]
        language: Option<String>,
        /// Download the model files this computer doesn't have yet from
        /// Hugging Face, each checked against its SHA-256. Without it, a
        /// missing file is an error that says what would be downloaded.
        #[serde(default)]
        download: bool,
        /// How the speech to hear is found: `sound` (the default), everything
        /// louder than `sound_db`; `voice`, the built-in voice-activity
        /// model, which misses singing, speech over music and shouts such as
        /// a countdown; or `none`, all of the audio. Words kept for a file are used again only when they
        /// were found the same way, at the same `sound_db`; without `find`,
        /// either way does.
        #[serde(default)]
        find: Option<FindSpeech>,
        /// For `find: sound`: sound quieter than this, in dBFS, for half a
        /// second or more is silence and isn't heard. Default: -40.
        #[serde(default)]
        sound_db: Option<f64>,
        /// Give each word its speaker. On this computer, in English and
        /// Chinese MOSS-Transcribe-Diarize tells them apart, else up to 4 in
        /// each file (NVIDIA's Sortformer); with `xai`, one numbering across
        /// every file heard; with `google`, the same up to 30 minutes of
        /// speech, past that this computer's. The project names them
        /// (`speakers.set`).
        #[serde(default)]
        speakers: bool,
        /// With `speakers` on this computer, the model that tells them
        /// apart: `moss` (English and Chinese), `sortformer` (any
        /// language, up to 4), or `auto`, by the language (the default).
        #[serde(default)]
        speaker_model: SpeakerModel,
        /// With the `xai` or `google` model: send the audio to that company.
        /// Without it nothing is uploaded, and the error gives the
        /// duration, the size and the estimated cost.
        #[serde(default)]
        send: bool,
        /// xAI's options, for the `xai` model.
        #[serde(default)]
        xai: Option<XaiOptions>,
        /// Gemini's options, for the `google` model.
        #[serde(default)]
        google: Option<GoogleOptions>,
        /// Hear media that have a transcript again, replacing it.
        #[serde(default)]
        again: bool,
    },
    /// The people speaking in the transcripts of the project's media and
    /// on its cues: each with its name, its color, the files' speakers it
    /// is, and how many words and cues.
    #[serde(rename = "speakers.list")]
    SpeakersList {},
    /// Name a person (`speakers.list`), or give them the color their cues
    /// are painted with (the subtitle track's `speaker_color`).
    #[serde(rename = "speakers.set")]
    SpeakersSet {
        person: String,
        /// Their name. `""` forgets it, back to `Speaker N`.
        #[serde(default)]
        name: Option<String>,
        /// `#rrggbb`, `#rgb` or a name such as `yellow`. `""` takes it away.
        #[serde(default)]
        color: Option<String>,
    },
    /// Same Person As: `person` becomes `into` everywhere, both as
    /// `speakers.list` gives them or by name: their cues on every track,
    /// the files' speakers and words that were theirs. `into` keeps its
    /// name, and its color, else takes `person`'s.
    #[serde(rename = "speakers.join")]
    SpeakersJoin { person: String, into: String },
    /// Make a file's speaker a person: one from `speakers.list`, which joins
    /// them, or a new one. Without `person`, it is again the person its
    /// transcript gives.
    #[serde(rename = "speakers.link")]
    SpeakersLink {
        media: String,
        #[serde(default)]
        stream: u32,
        /// The speaker's number in the file's transcript.
        speaker: u32,
        #[serde(default)]
        person: Option<String>,
    },
    /// Assign To: give a file's words whose middle is from `start` to `end`,
    /// in seconds of its sound (`analyze.words` gives each word's
    /// `file_time`), to a person. Without `person`, they are again their
    /// speaker's.
    #[serde(rename = "speakers.assign")]
    SpeakersAssign {
        media: String,
        #[serde(default)]
        stream: u32,
        start: f64,
        end: f64,
        #[serde(default)]
        person: Option<String>,
    },
    /// Leave words of a pool item out of edits, as Delete does in the
    /// Transcription window on a source clip: their spans (source frames,
    /// as `analyze.words` gives them with `media`) are struck through there,
    /// and its Place on Top, Insert and Append edit only the rest of the
    /// selection. `back` brings them back (Undelete).
    #[serde(rename = "media.leave_out")]
    MediaLeaveOut {
        media: String,
        words: Vec<SpokenWord>,
        #[serde(default)]
        back: bool,
    },
    /// Set the Clip Color of media items, which their clips show unless
    /// they have their own.
    #[serde(rename = "media.set")]
    MediaSet {
        media: OneOrMany<String>,
        /// A color as `clip.set` takes it. Empty string clears it.
        #[serde(default)]
        color: Option<String>,
        /// How its pictures' colors are read: `tagged`, as the file says, or
        /// `bt709`, `bt601`, `pq` or `hlg` in place of the file's tags, for a
        /// file whose tags are wrong. The viewer, thumbnails and exports
        /// follow it.
        #[serde(default)]
        color_space: Option<crate::model::ColorSpace>,
    },
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
        /// The marked duration, as the viewer's Duration field takes it:
        /// the Out moves to the In plus it; with no In, the first frame is
        /// marked In (decision D60). Not with `out`.
        #[serde(default)]
        duration: Option<DurationSpec>,
    },
    /// Clear a media item's source In and Out.
    #[serde(rename = "media.marks.clear")]
    MediaMarksClear { media: String },
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
        /// `proxies`) that have no proxy to play (none, an out-of-date one,
        /// or one whose file is gone), of `media` or of the pool; none is
        /// fine.
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

    /// Silent stretches (as Ripple Delete Silence finds them) in `media`
    /// (source frames), in `clips` (timeline frames), or in every clip with
    /// sound.
    #[serde(rename = "analyze.silence")]
    AnalyzeSilence {
        #[serde(default)]
        media: Option<String>,
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
        /// What counts as silence.
        #[serde(default)]
        detect: Detect,
        /// Sound below this level (dBFS) is silence (`detect: level`).
        #[serde(default = "silence_threshold")]
        threshold_db: f64,
        /// Speech probability (0..1) from which a moment is speech
        /// (`detect: speech`).
        #[serde(default = "speech_threshold")]
        speech_threshold: f64,
        /// Shorter speech doesn't count (`detect: speech`).
        #[serde(default = "min_speech")]
        min_speech: TimeSpec,
        /// The shortest silence reported.
        #[serde(default = "silence_min")]
        min: TimeSpec,
        /// `detect: speech` hears with transcribe.cpp's library and Silero VAD:
        /// download them if this computer doesn't have them yet, each checked
        /// against its SHA-256. Without it, a missing one is an error that
        /// says what would be downloaded.
        #[serde(default)]
        download: bool,
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
    /// Loudness of the timeline's mix (EBU R128): integrated LUFS, range,
    /// true peak (dBTP) and sample peak (dBFS), over `range` (default: In–Out
    /// if set, else everything).
    #[serde(rename = "analyze.loudness")]
    AnalyzeLoudness {
        #[serde(default)]
        range: Option<RangeSpec>,
    },
    /// The words said, each with its frames, for `edit.ripple_delete_words`.
    /// With `media` or `clips`, from their media's transcripts
    /// (`media.transcribe`), heard nothing new: the media's source frames,
    /// or each clip's timeline frames. Otherwise the mix of the tracks heard
    /// (`tracks`) over `range` is heard now, as `subtitles.transcribe` hears
    /// it, without adding cues. Only a model that times words: Parakeet
    /// Ultra does. Whisper large-v3 times phrases and Qwen3-ASR gives no
    /// times, so their words are refused rather than have estimates cut as
    /// words.
    #[serde(rename = "analyze.words")]
    AnalyzeWords {
        /// A pool item: its words in source frames, from its transcript.
        #[serde(default)]
        media: Option<String>,
        /// With `media`: the sound stream whose transcript is read, counted
        /// from 0. Default: the first one transcribed.
        #[serde(default)]
        stream: Option<u32>,
        /// Clips: the words each one shows, in timeline frames, from its
        /// media's transcript.
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
        #[serde(default)]
        range: Option<RangeSpec>,
        /// The tracks heard, such as `["A1"]`, muted or not; the others
        /// aren't. Default: the tracks playback hears, those not muted, or
        /// the soloed ones.
        #[serde(default)]
        tracks: Option<Vec<String>>,
        /// Default: `parakeet-ultra`.
        #[serde(default)]
        model: Option<SpeechModel>,
        /// A transcribe.cpp GGUF model file to use instead of `model`'s.
        #[serde(default)]
        model_file: Option<PathBuf>,
        /// A Silero VAD model file (GGUF, or whisper.cpp's `.bin`) to find
        /// the speech with, instead of the one built into vidcut.
        #[serde(default)]
        vad_file: Option<PathBuf>,
        /// The language spoken, as a code such as `en` or `fi`. Default: the
        /// model tells it from the longest stretch of speech.
        #[serde(default)]
        language: Option<String>,
        /// Download the model files this computer doesn't have yet from
        /// Hugging Face, each checked against its SHA-256. Without it, a
        /// missing file is an error that says what would be downloaded.
        #[serde(default)]
        download: bool,
        /// How the speech to hear is found: `sound` (the default), everything
        /// louder than `sound_db`; `voice`, the built-in voice-activity
        /// model, which misses singing, speech over music and shouts such as
        /// a countdown; or `none`, all of the audio. Words kept for a file are used again only when they
        /// were found the same way, at the same `sound_db`; without `find`,
        /// either way does.
        #[serde(default)]
        find: Option<FindSpeech>,
        /// For `find: sound`: sound quieter than this, in dBFS, for half a
        /// second or more is silence and isn't heard. Default: -40.
        #[serde(default)]
        sound_db: Option<f64>,
        /// Give each word its speaker. On this computer, in English and
        /// Chinese MOSS-Transcribe-Diarize tells them apart, else up to 4 in
        /// each file (NVIDIA's Sortformer); with `xai`, one numbering across
        /// every file heard; with `google`, the same up to 30 minutes of
        /// speech, past that this computer's. The project names them
        /// (`speakers.set`).
        #[serde(default)]
        speakers: bool,
        /// With `speakers` on this computer, the model that tells them
        /// apart: `moss` (English and Chinese), `sortformer` (any
        /// language, up to 4), or `auto`, by the language (the default).
        #[serde(default)]
        speaker_model: SpeakerModel,
        /// With the `xai` or `google` model: send the audio to that company.
        /// Without it nothing is uploaded, and the error gives the
        /// duration, the size and the estimated cost.
        #[serde(default)]
        send: bool,
        /// xAI's options, for the `xai` model.
        #[serde(default)]
        xai: Option<XaiOptions>,
        /// Gemini's options, for the `google` model.
        #[serde(default)]
        google: Option<GoogleOptions>,
    },
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

    /// Split at one or more frames (Ctrl+\).
    #[serde(rename = "edit.split")]
    EditSplit {
        at: OneOrMany<TimeSpec>,
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
    /// on it or ending on it. `before` and `after` instead give each side
    /// of the edit, each shortened to its own media, as dragging a
    /// dissolve's edge does. `picture` or `sound` false leaves that one
    /// out. `all`: on every edit between two clips of the track (Add
    /// Dissolve to All Edits). Transitions on those edits go.
    #[serde(rename = "edit.dissolve")]
    EditDissolve {
        #[serde(default)]
        at: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
        /// The selected clips (Ctrl+T): a dissolve on each of their edges
        /// instead, on their own tracks.
        #[serde(default)]
        clips: Vec<ClipRef>,
        #[serde(default)]
        length: Option<TimeSpec>,
        #[serde(default)]
        align: crate::ops::Align,
        /// Frames before the edit, with `after`, instead of `length` and
        /// `align`.
        #[serde(default)]
        before: Option<TimeSpec>,
        /// Frames after the edit, with `before`.
        #[serde(default)]
        after: Option<TimeSpec>,
        #[serde(default = "yes")]
        picture: bool,
        #[serde(default = "yes")]
        sound: bool,
        #[serde(default)]
        all: bool,
        /// A cross dissolve, or one wipe. Left out, a cross dissolve.
        #[serde(default, skip_serializing_if = "PictureTransition::is_dissolve")]
        transition: PictureTransition,
    },
    /// Sets the type of the picture transition on the edit of `track`
    /// (default V1) nearest `at` (default: the playhead). The length stays.
    /// A dissolve can become a wipe, and a wipe a dissolve.
    #[serde(rename = "edit.transition")]
    EditTransition {
        #[serde(default)]
        at: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
        transition: PictureTransition,
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
    /// tracks. The picture of a placed compound clip goes to each clip
    /// that has no picture of its own and fills the frame; with a Dynamic
    /// Zoom, only to a clip that spans all of the compound clip. A warning
    /// names the clips that don't take it.
    #[serde(rename = "edit.decompose")]
    EditDecompose { clips: OneOrMany<ClipRef> },
    /// Slide a clip (Dynamic Trim's Slide mode): it keeps its media and moves,
    /// the clip before it getting longer and the one after it shorter (or the
    /// reverse).
    #[serde(rename = "edit.slide")]
    EditSlide { clip: ClipRef, by: TimeSpec },
    /// Ripple Delete Silence: cut the silent parts out of `clips` and close the
    /// gaps, as one undo step. `dry_run` reports the parts without cutting (the
    /// red stripes, `removed`). `parts` cuts a list of them instead, edited
    /// as the stripes are (deviation X48): the dry run's `removed` with
    /// parts left out to keep them and `start` or `end` moved to trim them.
    #[serde(rename = "edit.ripple_delete_silence")]
    EditRippleDeleteSilence {
        clips: OneOrMany<ClipRef>,
        /// Cut exactly these parts and find none: each `start..end`
        /// (timeline frames, `end` excluded) inside its `clip`, one of
        /// `clips`, and parts of a clip apart. `detect` and its settings are
        /// then unused.
        #[serde(default)]
        parts: Option<Vec<ClipPart>>,
        /// What counts as silence.
        #[serde(default)]
        detect: Detect,
        /// Sound below this level (dBFS) is silence (`detect: level`).
        #[serde(default = "silence_threshold")]
        threshold_db: f64,
        /// Speech probability (0..1) from which a moment is speech
        /// (`detect: speech`).
        #[serde(default = "speech_threshold")]
        speech_threshold: f64,
        /// Shorter speech doesn't count (`detect: speech`).
        #[serde(default = "min_speech")]
        min_speech: TimeSpec,
        /// Silence kept before each audible part ("pre head").
        #[serde(default = "silence_margin")]
        pre_head: TimeSpec,
        /// Silence kept after each audible part ("post tail").
        #[serde(default = "silence_margin")]
        post_tail: TimeSpec,
        /// The shortest silence removed ("minimum to trip").
        #[serde(default = "silence_min")]
        min: TimeSpec,
        /// `detect: speech` hears with transcribe.cpp's library and Silero VAD:
        /// download them if this computer doesn't have them yet, each checked
        /// against its SHA-256. Without it, a missing one is an error that
        /// says what would be downloaded.
        #[serde(default)]
        download: bool,
    },
    /// Ripple Delete Words: take these words out of the timeline. Each is a
    /// `start` and `end` from `analyze.words` (its other fields are
    /// ignored). Touching or overlapping spans are one cut. The spans come
    /// out of every unlocked track and the rest closes up, whether or not
    /// ripple is on. A dry run says what would go and changes nothing.
    #[serde(rename = "edit.ripple_delete_words")]
    EditRippleDeleteWords {
        words: Vec<SpokenWord>,
        /// Move each cut's edges to the quietest frame boundary nearby, in the
        /// sound of the tracks heard there, as far as the clips' transcripts
        /// show the words: inward no further than a third into any word
        /// wholly inside the cut, so the words cut always lose their middles,
        /// and outward only into the pause beside it, up to the next word,
        /// within about a tenth of a second. Without a transcript the edges
        /// stay. Levels not measured yet are measured first.
        #[serde(default)]
        quiet: bool,
        /// Clear the timeline In and Out in the same step, as the
        /// Transcription window does when they mark the words it deletes.
        #[serde(default)]
        clear_marks: bool,
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

    /// The tracks, with their header and mixer settings and how many clips
    /// each holds.
    #[serde(rename = "track.list")]
    TrackList {},
    /// Add tracks (Timeline Actions > Add Track): video tracks go on top, audio
    /// tracks at the bottom, subtitle tracks above the video. Returns their
    /// ids. A new subtitle track is the one shown.
    #[serde(rename = "track.add")]
    TrackAdd {
        kind: NewTrack,
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
    /// volume in dB, Voice Isolation, ducking). A subtitle track (`S1`) takes `locked`, `name`, `shown`,
    /// `language`, and `style` or a `look`.
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
        /// Voice Isolation's Amount, from 1 to 100: that much of the voice
        /// the model hears in the sound of the track's clips
        /// that have none of their own, mixed with the rest of it; 0 turns
        /// it off. The model runs on this computer and is downloaded once,
        /// when `download` allows it.
        #[serde(default)]
        voice_isolation: Option<f64>,
        /// Ducking: the track dips by its Duck Amount where any of its
        /// sources is active, by speech or by sound level. Each field given
        /// replaces its own; null takes it off with its settings. `{}`
        /// turns it on with the usual values, under every other track with
        /// sound. Sources not measured yet are measured first.
        #[serde(
            default,
            deserialize_with = "given",
            skip_serializing_if = "Option::is_none"
        )]
        duck: Option<Option<DuckSet>>,
        /// Download what this computer doesn't have yet: Voice Isolation's
        /// model, or the speech detector of ducking by speech, each checked
        /// against its SHA-256. Without it, a missing file is an error that
        /// says what would be downloaded.
        #[serde(default)]
        download: bool,
        #[serde(default)]
        video_enabled: Option<bool>,
        /// Empty string clears the name.
        #[serde(default)]
        name: Option<String>,
        /// A subtitle track's cues show in the viewer. Showing one hides the
        /// others.
        #[serde(default)]
        shown: Option<bool>,
        /// A subtitle track's language, as a code such as `en` or `pt-BR`.
        /// Empty string clears it.
        #[serde(default)]
        language: Option<String>,
        /// A subtitle track's look, burned in and written to an IMSC1 file.
        /// Replaces the track's style; a field left out goes back to the
        /// usual look. `font` is a family name. `size` is the text height
        /// as a fraction of the picture height (above 0, at most 1/2).
        /// Colors are `#rrggbb`, `#rgb` or a name such as `yellow`.
        /// `outline` is a color and a `size` as a fraction of the text
        /// height, from 0 to 1/10 (`0` draws none). `background` is a color
        /// and an `opacity` from 0 to 1. `margin` is the inset from the
        /// left and right, and from the top or bottom, as a fraction of the
        /// picture (from 0 up to but not including 1/2). `bold` takes the
        /// font's bold face; `case` draws every letter `upper` or `lower`.
        /// `shadow` is a title's drop shadow. `words` shows the words as
        /// they are spoken, by the cues' word times: `show` (`highlight`,
        /// `reveal` or `word`), the spoken word's `color`, and its `pop`,
        /// how large it starts, from 1 to 2.
        #[serde(default)]
        style: Option<SubtitleStyle>,
        /// A subtitle look's name, built in or yours (`looks.list`), whose
        /// style replaces the track's. Not with `style`.
        #[serde(default)]
        look: Option<String>,
    },
    /// Enable/disable video (D), mute, gain (dB, -60..24), name, Clip Color
    /// and Voice Isolation.
    #[serde(rename = "clip.set")]
    ClipSet {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        muted: Option<bool>,
        /// The volume. A clip with volume keyframes gets it as a keyframe
        /// at `at` (decision D82). Such a clip without sound there keeps
        /// its volume.
        #[serde(default)]
        gain_db: Option<f64>,
        /// Where `gain_db` goes as a keyframe on clips with volume
        /// keyframes: the playhead by default.
        #[serde(default)]
        at: Option<TimeSpec>,
        /// Empty string clears the name.
        #[serde(default)]
        name: Option<String>,
        /// Clip Color: orange, apricot, yellow, lime, olive, green, teal,
        /// navy, blue, purple, violet, pink, tan, beige, brown or
        /// chocolate. Empty string clears it, and the clip shows its
        /// media's color.
        #[serde(default)]
        color: Option<String>,
        /// Voice Isolation's Amount, from 1 to 100: that much of the voice
        /// the model hears in the sound, mixed with the rest of it; 0 turns
        /// it off for the clip, also when its track has it on. A clip never
        /// given one follows its track's. The model runs on this computer
        /// and is downloaded once, when `download` allows it.
        #[serde(default)]
        voice_isolation: Option<f64>,
        /// Download Voice Isolation's model if this computer doesn't have it
        /// yet, checked against its SHA-256. Without it, a missing model is
        /// an error that says what would be downloaded.
        #[serde(default)]
        download: bool,
    },
    /// Normalize Audio Levels: set the volume (`gain_db`) of `clips` so
    /// that their level reaches `target`. Each clip is measured over the
    /// sound it plays, as the export mixes it, without its volume and its
    /// volume keyframes, mute, Voice Isolation and transitions, and without
    /// its track's mixer. The volume of a clip with volume keyframes is its
    /// highest keyframe, and every keyframe moves by the same dB (decision
    /// D83). The volume stops at -60 and +24 dB. One undo step; a dry run
    /// gives the volumes and changes nothing. Returns each clip's `level`
    /// (null when silent), its volume before (`from_db`) and after
    /// (`gain_db`), and `held` when the volume stopped short of the target.
    #[serde(rename = "clip.normalize")]
    ClipNormalize {
        clips: OneOrMany<ClipRef>,
        /// What is measured and brought to `target`.
        #[serde(default)]
        mode: NormalizeMode,
        /// The level to reach: LUFS with `loudness` (-60 to -5), dBFS with
        /// `sample_peak` and dBTP with `true_peak` (-60 to 0). Default: -14
        /// LUFS, as the export's loudness, or -1 for a peak.
        #[serde(default)]
        target: Option<f64>,
        /// Set Level Relative: the clips count as one, so the loudest, with
        /// its volume, reaches `target`, and every volume changes by the
        /// same dB. Otherwise (Independent) each clip reaches `target`, and
        /// a silent clip keeps its volume.
        #[serde(default)]
        relative: bool,
    },
    /// The keyframes of `clips`, or of every clip that has some (decision
    /// D81). Each clip gives its `track`, the value (`param`) and its
    /// keyframes in order: the source `frame`, the timeline frame `at` and
    /// its timecode, the `value`, the `ease` (null when a handle was
    /// dragged), and the `in` and `out` handles. Previous and Next Keyframe
    /// are this and `player.seek`.
    #[serde(rename = "keyframes.list")]
    KeyframesList {
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
    },
    /// Add a keyframe at `at` (the playhead by default) to each of `clips`
    /// whose sound is there, on the source frame under `at`. Without a
    /// `value`, it takes the value the clip has there. A keyframe already
    /// on that frame takes the value and the ease. With its first
    /// keyframe, a clip's volume follows its keyframes. Returns the
    /// `clips` with the source `frame` of each new keyframe.
    #[serde(rename = "keyframes.add")]
    KeyframesAdd {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        param: KeyParam,
        #[serde(default)]
        at: Option<TimeSpec>,
        /// The value at the keyframe. A number for volume (dB, from -60
        /// to 24), rotation (degrees) and a crop side (a fraction). For
        /// position and zoom, two numbers, x then y, or one number for
        /// both axes of zoom.
        #[serde(default)]
        value: Option<KeyValue>,
        #[serde(default)]
        ease: Option<Ease>,
    },
    /// Change the keyframes of `clips`: those on the source `frames`, those
    /// at the timeline frames `at`, or, with neither, all of them. Give a
    /// `value` or `by`, and an `ease` or handles. A handle given makes the
    /// keyframe's ease none of the four.
    #[serde(rename = "keyframes.set")]
    KeyframesSet {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        param: KeyParam,
        #[serde(default)]
        frames: Option<OneOrMany<i64>>,
        #[serde(default)]
        at: Option<OneOrMany<TimeSpec>>,
        /// The new value. A number, or two numbers for position and zoom.
        #[serde(default)]
        value: Option<KeyValue>,
        /// Added to each value. A number, or two numbers for position and
        /// zoom. Volume stops at -60 and +24 dB. A picture value stops at
        /// its own limits.
        #[serde(default)]
        by: Option<KeyValue>,
        /// Sets the handles: flat, a third of the way to the keyframe
        /// before (In) and after (Out). `linear` takes them off.
        #[serde(default)]
        ease: Option<Ease>,
        /// The In handle: `frames` (at most 0) and `value` (dB) from the
        /// keyframe. Null takes it off.
        #[serde(
            rename = "in",
            default,
            deserialize_with = "given",
            skip_serializing_if = "Option::is_none"
        )]
        in_handle: Option<Option<Handle>>,
        /// The Out handle: `frames` (at least 0) and `value` (dB) from the
        /// keyframe. Null takes it off.
        #[serde(
            rename = "out",
            default,
            deserialize_with = "given",
            skip_serializing_if = "Option::is_none"
        )]
        out_handle: Option<Option<Handle>>,
    },
    /// Move the keyframes of `clips` (those on the source `frames`, those
    /// at the timeline frames `at`, or all of them) `by` timeline frames,
    /// or move one keyframe `to` a timeline frame. A keyframe stays on a
    /// whole source frame, and two cannot share one.
    #[serde(rename = "keyframes.move")]
    KeyframesMove {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        param: KeyParam,
        #[serde(default)]
        frames: Option<OneOrMany<i64>>,
        #[serde(default)]
        at: Option<OneOrMany<TimeSpec>>,
        /// Timeline frames. Negative moves them earlier.
        #[serde(default)]
        by: Option<i64>,
        #[serde(default)]
        to: Option<TimeSpec>,
    },
    /// Remove the keyframes of `clips`: those on the source `frames`, those
    /// at the timeline frames `at`, or, with neither, all of them. When a
    /// clip's last keyframes go, the value of the first of them stays as
    /// its volume.
    #[serde(rename = "keyframes.remove")]
    KeyframesRemove {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        param: KeyParam,
        #[serde(default)]
        frames: Option<OneOrMany<i64>>,
        #[serde(default)]
        at: Option<OneOrMany<TimeSpec>>,
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
        /// The marked duration, as the viewer's Duration field takes it:
        /// the Out moves to the In plus it; with no In, the timeline's
        /// start is marked In (decision D60). Not with `out`.
        #[serde(default)]
        duration: Option<DurationSpec>,
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
    /// Subtitle cues with their times in frames: every track's, or `track`'s;
    /// those in `range` only, if given.
    #[serde(rename = "subtitles.list")]
    SubtitlesList {
        #[serde(default)]
        track: Option<String>,
        #[serde(default)]
        range: Option<RangeSpec>,
    },
    /// Add a cue: `text` from `at` (default: the playhead) to `end`, or for
    /// `duration`. Without either it lasts 3 s, or until the track's next cue
    /// starts. On `track` (default: the shown subtitle track; S1 is made when
    /// there is none). Returns its id.
    #[serde(rename = "subtitles.add")]
    SubtitlesAdd {
        /// Its lines, separated by a line break. `<i>`, `<b>`, `<u>` and
        /// `<font color="#rrggbb">` style words.
        text: String,
        #[serde(default)]
        at: Option<TimeSpec>,
        #[serde(default)]
        end: Option<TimeSpec>,
        #[serde(default)]
        duration: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
        #[serde(default)]
        place: Option<Place>,
        /// The frame each word of `text` starts on, counted from the cue's
        /// start, in order: one for each word, words being parted by spaces
        /// and line breaks.
        #[serde(default)]
        words: Option<Vec<i64>>,
        /// The person saying it, as `speakers.list` gives them, or a name.
        #[serde(default)]
        speaker: Option<String>,
        /// Its own color instead of its speaker's: `#rrggbb`, `#rgb` or a
        /// name such as `yellow`.
        #[serde(default)]
        color: Option<String>,
    },
    /// Change a cue (`q3`, or `S1@15s` for the cue on S1 at 15 s): its text,
    /// its start or end (each moves that edge alone), where it shows, its
    /// words' times, its speaker, its own color, or its own style. A style
    /// field left out keeps the track's style. `{}` drops the cue's style.
    /// Moving an edge keeps each word on its frame. New text
    /// with as many words keeps their times; with another number of words,
    /// they are read again from the words kept for the clips' files under
    /// it, as `subtitles.time_words` reads them, else spread over the cue by
    /// their characters.
    #[serde(rename = "subtitles.set")]
    SubtitlesSet {
        cue: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        start: Option<TimeSpec>,
        #[serde(default)]
        end: Option<TimeSpec>,
        #[serde(default)]
        place: Option<Place>,
        /// The frame each word starts on, counted from the cue's start, in
        /// order: one for each word of its text. `[]` clears them.
        #[serde(default)]
        words: Option<Vec<i64>>,
        /// The person saying it, as `speakers.list` gives them, or a name.
        /// `""` takes it away.
        #[serde(default)]
        speaker: Option<String>,
        /// Its own color instead of its speaker's: `#rrggbb`, `#rgb` or a
        /// name such as `yellow`. `""` goes back to its speaker's.
        #[serde(default)]
        color: Option<String>,
        /// Its own style. A field left out keeps the track's. `{}` drops it.
        /// `align` is `left`, `center` or `right`. `position` is `[x, y]`,
        /// fractions of the picture from the top left, each from 0 to 1.
        #[serde(default)]
        style: Option<SubtitleStyle>,
    },
    /// Set the style of one word of a cue (`q3`, or `S1@15s`). `word` counts
    /// from 0, in the order of the words of the cue's text. A style field
    /// left out keeps the cue's style, which keeps the track's where the cue
    /// leaves a field out. `{}` drops the word's style. The other words stay
    /// as they are.
    #[serde(rename = "subtitles.set_word")]
    SubtitlesSetWord {
        cue: String,
        /// The word, counted from 0.
        word: usize,
        /// Its own style. A field left out keeps the cue's. `{}` drops it.
        style: SubtitleStyle,
    },
    /// Apply to All: `cue`'s own color becomes its speaker's, and every cue
    /// of that speaker on its track takes its place and drops a color of
    /// its own. Returns the cues changed.
    #[serde(rename = "subtitles.match_speaker")]
    SubtitlesMatchSpeaker { cue: String },
    /// Move cues by `by` (`+12f`, `-1s`) or so the first starts at `to`; with
    /// `track`, onto that subtitle track.
    #[serde(rename = "subtitles.move")]
    SubtitlesMove {
        cues: OneOrMany<String>,
        #[serde(default)]
        by: Option<TimeSpec>,
        #[serde(default)]
        to: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
    },
    /// Remove cues (`q3`, or `S1@15s`).
    #[serde(rename = "subtitles.remove")]
    SubtitlesRemove { cues: OneOrMany<String> },
    /// Split a cue at `at` (default: the playhead). Both parts keep its text.
    #[serde(rename = "subtitles.split")]
    SubtitlesSplit {
        cue: String,
        #[serde(default)]
        at: Option<TimeSpec>,
    },
    /// Join cues of one track into the first: from its start to the last
    /// one's end, their texts on separate lines.
    #[serde(rename = "subtitles.join")]
    SubtitlesJoin { cues: OneOrMany<String> },
    /// Put a subtitle file's cues on a subtitle track: SRT, WebVTT, or TTML
    /// (`.ttml`, `.dfxp`, `.xml`, IMSC1 too), in UTF-8, UTF-16 or
    /// Windows-1252, a file (`path`) or a subtitle item in the pool (`media`),
    /// whose file is read again. Each time goes to the nearest frame. The
    /// answer's `placed` says how: `at`, `timecode` (the file's times read as
    /// the timeline's timecode) or `start` (counted from the timeline's
    /// start).
    #[serde(rename = "subtitles.import")]
    SubtitlesImport {
        #[serde(default)]
        path: Option<PathBuf>,
        /// A subtitle item in the pool, instead of `path`.
        #[serde(default)]
        media: Option<String>,
        /// The subtitle track to add them to. Default: a new one, named after
        /// the file, in the file's language when it names one, and shown.
        #[serde(default)]
        track: Option<String>,
        /// Place the first cue here, the others after it as in the file.
        #[serde(default)]
        at: Option<TimeSpec>,
        /// The file is timed in the timeline's timecode (true) or from the
        /// timeline's start (false). Default: true when every cue is at or
        /// after the timeline's start timecode, or, on a timeline starting at
        /// zero and shorter than an hour, at or after 01:00:00:00.
        #[serde(default)]
        timecode: Option<bool>,
        /// How the file's times count frames. Default: `auto`.
        #[serde(default)]
        time_base: Option<SubtitleTimeBase>,
    },
    /// Write a subtitle track's cues to a file: SRT, WebVTT or TTML (IMSC1).
    #[serde(rename = "subtitles.export")]
    SubtitlesExport {
        out: PathBuf,
        /// Default: the shown subtitle track.
        #[serde(default)]
        track: Option<String>,
        /// Default: from the extension of `out`.
        #[serde(default)]
        format: Option<SubtitleFormat>,
        /// The cues in this range only, cut to it.
        #[serde(default)]
        range: Option<RangeSpec>,
        /// Times in the timeline's timecode (the default), or from the start
        /// of `range` (false).
        #[serde(default)]
        timecode: Option<bool>,
        #[serde(default)]
        overwrite: bool,
    },
    /// Create Subtitles from Audio: transcribe the speech of the tracks heard
    /// (`tracks`) over `range` (default: In–Out when both are set, else
    /// everything) on this computer, on the GPU unless the preferences turn
    /// it off, and add it as cues to a subtitle track. The model files are
    /// downloaded once, into the cache folder, when `download` allows it.
    #[serde(rename = "subtitles.transcribe")]
    SubtitlesTranscribe {
        #[serde(default)]
        range: Option<RangeSpec>,
        /// The tracks heard, such as `["A1"]`, muted or not; the others
        /// aren't. Default: the tracks playback hears, those not muted, or
        /// the soloed ones.
        #[serde(default)]
        tracks: Option<Vec<String>>,
        /// Default: `parakeet-ultra`.
        #[serde(default)]
        model: Option<SpeechModel>,
        /// A transcribe.cpp GGUF model file to use instead of `model`'s.
        #[serde(default)]
        model_file: Option<PathBuf>,
        /// A Silero VAD model file (GGUF, or whisper.cpp's `.bin`) to find
        /// the speech with, instead of the one built into vidcut.
        #[serde(default)]
        vad_file: Option<PathBuf>,
        /// The language spoken, as a code such as `en` or `fi`. Default: the
        /// model tells it from the longest stretch of speech.
        #[serde(default)]
        language: Option<String>,
        /// The subtitle track for the cues. Default: a new one with `look`,
        /// else the highest one, or a new one when there is none.
        #[serde(default)]
        track: Option<String>,
        /// A subtitle look's name, built in or yours (`looks.list`), which
        /// sets the track's style and the usual `chars` and `lines`. Built
        /// in: `Pop`, the spoken word yellow and popping, in bold capitals,
        /// 20 characters on one line; `Highlight`, the spoken word yellow;
        /// `Reveal`, each word shown as it is spoken; `Word`, only the
        /// spoken word, large; `Plain`, the usual style. Names match
        /// whatever their case.
        #[serde(default)]
        look: Option<String>,
        /// Characters per line, 1 to 60. Default: `look`'s, else 42.
        #[serde(default)]
        chars: Option<u32>,
        /// Lines per cue: 1 or 2. Default: `look`'s, else 1.
        #[serde(default)]
        lines: Option<u32>,
        /// Frames left between a cue and the next, 0 to 10.
        #[serde(default)]
        gap: u32,
        /// `media`: the words of each heard clip's file, kept from before when
        /// the same model made them in the language asked, else transcribed
        /// and kept (Transcribe Audio), placed through the clips. Where two
        /// tracks have the same word at once, the first track's is kept (V1,
        /// V2, …, then A1, A2, …). `mix`: the mix of the tracks heard over
        /// `range`, which keeps nothing. `transcript`: the words each file
        /// has already, whichever model made them, in their language; only
        /// a file without any is transcribed with `model` and `language`,
        /// and kept. Default: `media` over the whole timeline with a model
        /// that times words and no `model_file`; else `mix`, so a part of a
        /// long recording is all that is heard.
        #[serde(default)]
        hear: Option<HearFrom>,
        /// Download what this computer doesn't have yet: transcribe.cpp's
        /// library, Silero VAD and the model, each checked against its
        /// SHA-256. Without it, a missing one is an error that says what
        /// would be downloaded.
        #[serde(default)]
        download: bool,
        /// How the speech to hear is found: `sound` (the default), everything
        /// louder than `sound_db`; `voice`, the built-in voice-activity
        /// model, which misses singing, speech over music and shouts such as
        /// a countdown; or `none`, all of the audio. Words kept for a file are used again only when they
        /// were found the same way, at the same `sound_db`; without `find`,
        /// either way does.
        #[serde(default)]
        find: Option<FindSpeech>,
        /// For `find: sound`: sound quieter than this, in dBFS, for half a
        /// second or more is silence and isn't heard. Default: -40.
        #[serde(default)]
        sound_db: Option<f64>,
        /// Give each word its speaker. On this computer, in English and
        /// Chinese MOSS-Transcribe-Diarize tells them apart, else up to 4 in
        /// each file (NVIDIA's Sortformer); with `xai`, one numbering across
        /// every file heard; with `google`, the same up to 30 minutes of
        /// speech, past that this computer's. The project names them
        /// (`speakers.set`).
        #[serde(default)]
        speakers: bool,
        /// With `speakers` on this computer, the model that tells them
        /// apart: `moss` (English and Chinese), `sortformer` (any
        /// language, up to 4), or `auto`, by the language (the default).
        #[serde(default)]
        speaker_model: SpeakerModel,
        /// With the `xai` or `google` model: send the audio to that company.
        /// Without it nothing is uploaded, and the error gives the
        /// duration, the size and the estimated cost.
        #[serde(default)]
        send: bool,
        /// xAI's options, for the `xai` model.
        #[serde(default)]
        xai: Option<XaiOptions>,
        /// Gemini's options, for the `google` model.
        #[serde(default)]
        google: Option<GoogleOptions>,
    },
    /// Re-time a subtitle track to the timeline's speech. Where the built-in
    /// voice-activity model hears speech is lined up with where the cues
    /// show, so it needs no words and works for any language, translations
    /// too: one offset, and a speed change where one fits better, snapped
    /// to a frame-rate ratio such as 25/23.976 when it is near one. The
    /// answer says how clearly the best offset stands out from the next
    /// best more than 2 s away; when it isn't 1.25 times as high, nothing
    /// moves and the error gives the numbers. With `words`, the speech's
    /// words are read too, as `subtitles.transcribe` reads them (`hear`) with
    /// Parakeet Ultra, and the cues whose first word is in a run of 3 or more
    /// words matching them check the result. Then the simplest
    /// correction that puts 90% of them within 0.5 s of their words wins:
    /// one offset, an offset and a speed, an offset for each section between
    /// jumps, or else each of those cues moved to its words. A dry run
    /// answers the same and moves nothing. Only the tracks heard
    /// (`tracks`) count.
    #[serde(rename = "subtitles.retime")]
    SubtitlesRetime {
        /// Default: the shown subtitle track.
        #[serde(default)]
        track: Option<String>,
        /// The tracks heard, such as `["A1"]`, muted or not; the others
        /// aren't. Default: the tracks playback hears, those not muted, or
        /// the soloed ones.
        #[serde(default)]
        tracks: Option<Vec<String>>,
        /// Also match the cues' words to the words Parakeet Ultra hears.
        #[serde(default)]
        words: bool,
        /// For `words`: `media` (the default), each heard clip's file's words,
        /// `mix` or `transcript`, as `subtitles.transcribe` takes it.
        #[serde(default)]
        hear: Option<HearFrom>,
        /// The language spoken, as a code such as `en`, for `words`.
        /// Default: the model tells it from the longest stretch of speech.
        #[serde(default)]
        language: Option<String>,
        /// The largest offset looked for, earlier or later. Default: 60 s.
        #[serde(default)]
        max_offset: Option<TimeSpec>,
        /// Download what hearing needs if this computer doesn't have it yet:
        /// transcribe.cpp's library and Silero VAD, and Parakeet Ultra for
        /// `words`, each checked against its SHA-256. Without it, a missing
        /// one is an error that says what would be downloaded.
        #[serde(default)]
        download: bool,
        /// How the speech to hear is found: `sound` (the default), everything
        /// louder than `sound_db`; `voice`, the built-in voice-activity
        /// model, which misses singing, speech over music and shouts such as
        /// a countdown; or `none`, all of the audio. Words kept for a file are used again only when they
        /// were found the same way, at the same `sound_db`; without `find`,
        /// either way does.
        #[serde(default)]
        find: Option<FindSpeech>,
        /// For `find: sound`: sound quieter than this, in dBFS, for half a
        /// second or more is silence and isn't heard. Default: -40.
        #[serde(default)]
        sound_db: Option<f64>,
    },
    /// Synchronize Word Timings: time each word of a subtitle track's cues
    /// to the speech. The words heard, as `subtitles.transcribe` hears them
    /// (`hear`) with Parakeet Ultra, are matched to the cues' words in
    /// order across the cues timed, so cues off from the speech still find
    /// their words. A word matched starts where it is heard, kept inside its
    /// cue; the others are spread between their matched neighbors by their
    /// characters. The answer counts the words matched and names the cues
    /// with none.
    #[serde(rename = "subtitles.time_words")]
    SubtitlesTimeWords {
        /// Default: the shown subtitle track.
        #[serde(default)]
        track: Option<String>,
        /// These cues only (`q3`, or `S1@15s`). Default: the track's cues in
        /// `range`.
        #[serde(default)]
        cues: Option<OneOrMany<String>>,
        /// Default: In–Out when both are set, else everything.
        #[serde(default)]
        range: Option<RangeSpec>,
        /// The tracks heard, such as `["A1"]`, muted or not; the others
        /// aren't. Default: the tracks playback hears, those not muted, or
        /// the soloed ones.
        #[serde(default)]
        tracks: Option<Vec<String>>,
        /// `media` (the default), each heard clip's file's words, `mix` or
        /// `transcript`, as `subtitles.transcribe` takes it.
        #[serde(default)]
        hear: Option<HearFrom>,
        /// The language spoken, as a code such as `en`. Default: the model
        /// tells it from the longest stretch of speech.
        #[serde(default)]
        language: Option<String>,
        /// Download what hearing needs if this computer doesn't have it yet:
        /// transcribe.cpp's library, Silero VAD and Parakeet Ultra, each
        /// checked against its SHA-256. Without it, a missing one is an
        /// error that says what would be downloaded.
        #[serde(default)]
        download: bool,
        /// How the speech to hear is found: `sound` (the default), everything
        /// louder than `sound_db`; `voice`, the built-in voice-activity
        /// model, which misses singing, speech over music and shouts such as
        /// a countdown; or `none`, all of the audio. Words kept for a file are used again only when they
        /// were found the same way, at the same `sound_db`; without `find`,
        /// either way does.
        #[serde(default)]
        find: Option<FindSpeech>,
        /// For `find: sound`: sound quieter than this, in dBFS, for half a
        /// second or more is silence and isn't heard. Default: -40.
        #[serde(default)]
        sound_db: Option<f64>,
    },

    /// Title clips with their times in frames: every video track's, or
    /// `track`'s; those in `range` only, if given.
    #[serde(rename = "titles.list")]
    TitlesList {
        #[serde(default)]
        track: Option<String>,
        #[serde(default)]
        range: Option<RangeSpec>,
    },
    /// Add a title clip: `text` drawn over the picture of the tracks below
    /// it, from `at` (default: the playhead) for `duration` (default: 5 s).
    /// On `track` (default: the video track above the highest one with
    /// clips in those frames, made when there is none, or V1 when nothing
    /// is there), over what is there. Returns its clip id.
    #[serde(rename = "titles.add")]
    TitlesAdd {
        /// Its lines, separated by a line break: two at most for a lower
        /// third. `<b>`, `<i>`, `<u>` and `<font color="…" face="…"
        /// size="…">` style words apart; `&lt;`, `&gt;` and `&amp;` stand for
        /// `<`, `>` and `&`.
        text: String,
        /// `text` (the default), `lower_third` or `scroll`.
        #[serde(default)]
        kind: Option<TitleKind>,
        /// How a lower third or a scroll lines up: `left`, `middle` or
        /// `right` (a lower third's default left, a scroll's middle).
        #[serde(default)]
        align: Option<TitleAlign>,
        /// Its look: `font` (a family name), `size` (the text height as a
        /// fraction of the picture height, above 0, at most 1/2), `color`
        /// (`#rrggbb`, `#rgb` or a name such as `yellow`), `outline` (a
        /// color and a `size` as a fraction of the text height, from 0 to
        /// 1/10, `0` drawing none), `background` (a color and an `opacity`
        /// from 0 to 1, a box behind the whole text) and `shadow` (a
        /// `color`, an `opacity` from 0 to 1, an `offset` `[x, y]` right and
        /// down as fractions of the picture, and a `blur` from 0 to 100). A
        /// field left out keeps the usual look.
        #[serde(default)]
        style: Option<TitleStyle>,
        /// Where it goes, `[x, y]` as fractions of the picture from its top
        /// left: a `text` title's middle, a lower third's corner by its
        /// `align`. Default: the kind's usual place.
        #[serde(default)]
        position: Option<[f64; 2]>,
        /// How much larger its text is drawn, from 1/4 to 4. Default: 1.
        #[serde(default)]
        zoom: Option<f64>,
        /// Degrees turned counterclockwise. Default: 0.
        #[serde(default)]
        rotation: Option<f64>,
        /// How opaque the whole title is, from 0 to 1. Default: 1.
        #[serde(default)]
        opacity: Option<f64>,
        /// A title look's name, built in or yours (`looks.list`): the title
        /// starts from its kind, align, style, place, zoom, rotation and
        /// opacity, and the fields given go on top.
        #[serde(default)]
        look: Option<String>,
        #[serde(default)]
        at: Option<TimeSpec>,
        #[serde(default)]
        duration: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
    },
    /// Change a title clip (`c3`, or `V2@15s` for the clip on V2 at 15 s):
    /// each field given replaces its own. A `style` replaces the whole
    /// style, a field left out going back to the usual look. Switching to
    /// `text` drops the `align`. A `look` replaces all but the text, and the
    /// other fields given go on top. Moves and trims are `edit.*`'s.
    #[serde(rename = "titles.set")]
    TitlesSet {
        clip: ClipRef,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        kind: Option<TitleKind>,
        #[serde(default)]
        align: Option<TitleAlign>,
        #[serde(default)]
        style: Option<TitleStyle>,
        /// `[x, y]` as fractions of the picture, or `[]` for the kind's
        /// usual place.
        #[serde(default)]
        position: Option<Vec<f64>>,
        /// How much larger its text is drawn, from 1/4 to 4; 1 for the usual
        /// size.
        #[serde(default)]
        zoom: Option<f64>,
        /// Degrees turned counterclockwise; 0 for none.
        #[serde(default)]
        rotation: Option<f64>,
        /// How opaque the whole title is, from 0 to 1.
        #[serde(default)]
        opacity: Option<f64>,
        /// A title look's name, built in or yours (`looks.list`).
        #[serde(default)]
        look: Option<String>,
    },

    /// Where a clip's picture goes over the frame (`c3`, or `V2@15s`): the
    /// `position` of its middle, its `zoom` across and down, its `rotation`,
    /// its `anchor`, its `pitch`, its `yaw`, its flips, its `crop` and its
    /// `dynamic_zoom` (null when it has none), every value given, usual ones too.
    #[serde(rename = "picture.get")]
    PictureGet { clip: ClipRef },
    /// Move, zoom, turn, pitch, yaw, flip or crop clips' pictures, or give
    /// them a Dynamic Zoom: footage, stills, image sequences and compound
    /// clips; a title has its own place (`titles.set`). Each field given
    /// replaces its own. At its usual values a picture is fitted into the
    /// frame; the tracks below show wherever it doesn't cover the frame: in
    /// its bars, around it and where it is cropped.
    #[serde(rename = "picture.set")]
    PictureSet {
        clips: OneOrMany<ClipRef>,
        /// Where the middle of the whole picture goes before the zoom and
        /// the rotation, `[x, y]` as fractions of the frame from its top
        /// left, each from -100 to 100, or `[]` for the middle.
        #[serde(default)]
        position: Option<Vec<f64>>,
        /// How much larger it is drawn about its anchor, from 1/100 to 100:
        /// one number for both ways, or `[x, y]` across and down. 1 for its
        /// usual size.
        #[serde(default)]
        zoom: Option<OneOrMany<f64>>,
        /// Degrees turned counterclockwise about its anchor, from -3600 to
        /// 3600; 0 for none.
        #[serde(default)]
        rotation: Option<f64>,
        /// The point of the picture that the zoom and the rotation work
        /// about, `[x, y]` as fractions of the picture from its top left, as
        /// it shows after the flips, each from -100 to 100, or `[]` for the
        /// middle.
        #[serde(default)]
        anchor: Option<Vec<f64>>,
        /// Pitch about the middle of the picture, in its own axes, before
        /// the zoom and the rotation, from -1.5 to 1.5. Positive puts the
        /// top farther away. 0 for none.
        #[serde(default)]
        pitch: Option<f64>,
        /// Yaw about the middle of the picture, in its own axes, before
        /// the zoom and the rotation, from -1.5 to 1.5. Positive puts the
        /// left closer. 0 for none.
        #[serde(default)]
        yaw: Option<f64>,
        /// Mirrored left to right.
        #[serde(default)]
        flip_horizontal: Option<bool>,
        /// Mirrored top to bottom.
        #[serde(default)]
        flip_vertical: Option<bool>,
        /// What is cut off its left side, as a fraction of its width, from
        /// 0 up to what leaves some of it with `crop_right`.
        #[serde(default)]
        crop_left: Option<f64>,
        /// What is cut off its right side, as a fraction of its width.
        #[serde(default)]
        crop_right: Option<f64>,
        /// What is cut off its top, as a fraction of its height, from 0 up
        /// to what leaves some of it with `crop_bottom`.
        #[serde(default)]
        crop_top: Option<f64>,
        /// What is cut off its bottom, as a fraction of its height.
        #[serde(default)]
        crop_bottom: Option<f64>,
        /// Where a change of a value that has keyframes goes, as a
        /// keyframe: the playhead by default (decision D75). A value with
        /// no keyframes ignores it.
        #[serde(default)]
        at: Option<TimeSpec>,
        /// A slow push or pull and pan across the clip, from a start box to
        /// an end box; null takes it off. `{}` turns it on as it starts.
        #[serde(
            default,
            deserialize_with = "given",
            skip_serializing_if = "Option::is_none"
        )]
        dynamic_zoom: Option<Option<DynamicZoomSet>>,
    },
    /// Put clips' pictures back at their usual values: the `transform`
    /// (position, zoom, rotation, anchor, pitch, yaw and flips), the `crop`,
    /// the `dynamic_zoom`, or all of them when no `tool` is given.
    #[serde(rename = "picture.clear")]
    PictureClear {
        clips: OneOrMany<ClipRef>,
        #[serde(default)]
        tool: Option<PictureTool>,
    },

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
    /// Looks: named styles for subtitle tracks and titles, built in and
    /// yours, `kind`'s only if given. A look file that can't be read is left
    /// out and listed in `refused` with why.
    #[serde(rename = "looks.list")]
    LooksList {
        #[serde(default)]
        kind: Option<LookKind>,
    },
    /// Save one of your looks (`~/.config/vidcut/looks/subtitles/NAME.toml`
    /// or `looks/titles/NAME.toml`, on Windows in `%APPDATA%\vidcut`, on
    /// macOS in `~/Library/Application Support/vidcut`): `subtitles`, a
    /// track's `style` with the `chars` and `lines` Create Subtitles from
    /// Audio uses, or `titles`, a title's `kind`, `align`, `style`,
    /// `position`, `zoom`, `rotation` and `opacity`, without its text, as
    /// `titles.add` takes them. Built-in looks can't be replaced.
    #[serde(rename = "looks.save")]
    LooksSave {
        /// Up to 64 characters, without `/ \ : * ? " < > |`; names match
        /// whatever their case.
        name: String,
        #[serde(default)]
        subtitles: Option<SubtitleLook>,
        #[serde(default)]
        titles: Option<TitleLook>,
        /// Replace your look of that name.
        #[serde(default)]
        overwrite: bool,
    },
    /// Delete one of your looks. Built-in looks can't be deleted.
    #[serde(rename = "looks.delete")]
    LooksDelete { kind: LookKind, name: String },
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
    /// Support/vidcut`) and what the disk cache holds, with its limit.
    #[serde(rename = "preferences.get")]
    PreferencesGet {},
    /// Change application preferences; what is left out stays. Turning the
    /// thumbnails' disk cache off empties it, and a lower `cache` limit
    /// evicts at once.
    #[serde(rename = "preferences.set")]
    PreferencesSet(crate::preferences::PreferencesPatch),
    /// Put the preferences back to the defaults. The file is written anew,
    /// and the old one is kept next to it as `preferences.toml.bak`, or
    /// `preferences.toml.bak.2` and on when that is taken: a backup is never
    /// replaced.
    #[serde(rename = "preferences.reset")]
    PreferencesReset {},
    /// The keyboard shortcuts, one action each, with a few plain words
    /// where the action's name doesn't say what it does (spec §11). Ctrl
    /// and Alt are Cmd and Option on macOS.
    #[serde(rename = "keys.list")]
    KeysList {},
    /// Keep a paid speech service's key in the system's keychain (macOS
    /// Keychain, Windows Credential Manager, the Secret Service on Linux).
    /// Without `key`, the CLI reads it from standard input. The key never
    /// goes into a file, a log or an answer.
    #[serde(rename = "services.set_key")]
    ServicesSetKey {
        service: SpeechService,
        #[serde(default)]
        key: Option<String>,
    },
    /// Take a paid speech service's key out of the system's keychain.
    #[serde(rename = "services.remove_key")]
    ServicesRemoveKey { service: SpeechService },

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
    /// Go to a step of the undo history as one action: undo or redo until
    /// `step` steps are done. 0 is the project before them all;
    /// `history.get` numbers the steps.
    #[serde(rename = "history.go")]
    HistoryGo { step: usize },
    /// Name a step of the undo history as a version to come back to:
    /// `step` (default: the step the project is at; 0 is the project before
    /// them all). Naming a step again renames it. The names stay with the
    /// history, listed by `history.get` as `marks`.
    #[serde(rename = "history.mark")]
    HistoryMark {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<usize>,
        name: String,
    },
    /// Take the name off a step (default: the step the project is at).
    #[serde(rename = "history.unmark")]
    HistoryUnmark {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<usize>,
    },
    /// Bring back steps that a new change replaced after an undo: a
    /// version `history.get` lists in `branches`, by its `id`. The steps
    /// after where it branched off become a version of their own, so
    /// nothing is lost, and the project goes to the restored version's
    /// last step. A version inside another comes back with the versions
    /// holding it.
    #[serde(rename = "history.restore")]
    HistoryRestore { branch: u64 },
    /// Undo one done step alone, as a new step, keeping the steps after
    /// it. Refused, saying which, when a later step changed what it
    /// changed.
    #[serde(rename = "history.revert")]
    HistoryRevert { step: usize },
    /// The undo history. `steps` lists them oldest first, numbered from 1:
    /// each with its `label`, who made it (`by`: `gui`, `cli`, `mcp`, `ipc`
    /// or `file`, another program writing the project file, and a `name`
    /// the client gave), when (`time`, milliseconds since 1970 UTC), the
    /// clips and frames it changed (`clips`, `spans`), the commands of an
    /// `apply` (`parts`), when the GUI that made it opened the project
    /// (`opened`), its `id`, the `request` that made it (its `method` and
    /// `params`; none for a `file` step) and its `kind` (that method), and
    /// whether it is `done` (redo steps are not). `at` is the step the
    /// project is at. `groups` are runs of steps one client made in a row
    /// (an agent's edits). `branches` are steps a new change replaced after
    /// an undo, kept as versions to restore (`history.restore`), each
    /// `after` the step it branched from; `marks` are named steps
    /// (`history.mark`). `undo_steps` and `redo_steps` count the steps;
    /// `undo` and `redo` are the newest 100 labels of each, newest first.
    #[serde(rename = "history.get")]
    HistoryGet {
        /// The first step `steps` lists (default: the newest `limit`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<usize>,
        /// How many steps `steps` lists (default 100).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        /// Also say what changed from this step to the project as it is
        /// (`since`): clips added, deleted, moved and trimmed, dissolves,
        /// cross fades, markers, media, tracks, bins, In and Out marks and
        /// settings. For a step still to redo it says what redoing to it
        /// brings, with `redo` true.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<usize>,
        /// With `since`, a version from `branches`, by its `id`: `since`
        /// counts its steps from 1, and the answer says what going from the
        /// project as it is to that step of the version brings.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<u64>,
    },
    /// Apply requests atomically as one undo step.
    #[serde(rename = "apply")]
    Apply {
        commands: Vec<Request>,
        #[serde(default)]
        label: Option<String>,
    },

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
    /// What the viewer shows, and how its timecode field counts.
    /// `mode` is `source_clip`, `source_tape`, or `timeline`. Omitted, the viewer stays.
    /// `display` is the count (decision D91). Omitted, the count stays.
    /// `reset_timer1` stores the playhead on screen as Timer 1's zero for that player.
    /// The zero lasts for this session.
    /// `paste` seeks that playhead to a copied count, read in the display this call leaves.
    /// Give one of `mode`, `display`, `reset_timer1`, or `paste`.
    #[serde(rename = "viewer.set")]
    ViewerSet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<ViewerMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<TimecodeDisplay>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reset_timer1: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paste: Option<String>,
    },
    /// Viewer framing and safe-area guides (decision D90). Stored in
    /// preferences, not the project. A field that is left out stays as it is.
    /// With no fields, toggles framing guides on or off.
    #[serde(rename = "viewer.guides")]
    ViewerGuides {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        framing: Option<crate::preferences::Framing>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extents: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        center: Option<bool>,
    },
    /// Full screen or the enhanced viewer.
    /// `full_screen` fills the display with the viewer (Shift+F).
    /// `enhanced` fills the page with the viewer (Alt+F).
    /// `page` is the usual layout. Esc in the GUI returns to `page`.
    /// The choice lasts for this session. It is not in the project.
    #[serde(rename = "viewer.frame")]
    ViewerFrame { mode: ViewerFrame },
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
    /// `timecode` is the text in the timecode field, or `-`.
    /// `timecode_display` is how that field counts (decision D91).
    /// `timer1_origin` is each player's Timer 1 zero, `source` and
    /// `timeline`, or null until a reset. The zeros last for this session.
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
    /// Loop playback (decision D51): play that reaches the end goes on from
    /// the start, and with both In and Out set, play that reaches the Out
    /// goes on from the In; backward, the other way round. Off by default;
    /// not saved with the project. Without `on`, toggles (Ctrl+/).
    #[serde(rename = "player.loop")]
    PlayerLoop {
        #[serde(default)]
        on: Option<bool>,
    },
    /// Fast Review (decision D97): Play and Space run a long clip faster than
    /// a short one. A clip of 10 seconds or less plays at 1×. A longer clip
    /// plays at its seconds divided by 10, at most 8×. J and L keep their
    /// shuttle speeds. Off by default. Not saved. Without `on`, toggles.
    #[serde(rename = "player.fast_review")]
    PlayerFastReview {
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
        /// The search box: only media whose names hold it (any case), from
        /// the bin and the bins inside it. Empty shows everything.
        #[serde(default)]
        search: Option<String>,
    },
    /// The timeline clips, subtitle cues, pool media and edit point selected
    /// (`edit`).
    #[serde(rename = "selection.get")]
    SelectionGet {},
    /// Select timeline clips, subtitle cues, pool media or an edit point
    /// (omitted parts stay as they are). `,` and `.`, Ctrl+T and Dynamic
    /// Trim act on the edit.
    #[serde(rename = "selection.set")]
    SelectionSet {
        #[serde(default)]
        clips: Option<OneOrMany<ClipRef>>,
        /// Subtitle cues (`q3`, or `S1@15s`). Delete removes them, Ctrl+B
        /// splits those under the playhead.
        #[serde(default)]
        cues: Option<OneOrMany<String>>,
        #[serde(default)]
        media: Option<OneOrMany<String>>,
        /// Select the edit point of `track` nearest this time, as V does
        /// at the playhead (`playhead`) and a click on an edit does.
        #[serde(default)]
        edit: Option<TimeSpec>,
        /// The track of `edit` (default V1; spec D36).
        #[serde(default)]
        track: Option<String>,
        /// What of the edit is selected: `both` (a roll, the default),
        /// `out`, `in`, or `audio`, its audio edit (the J/L handle,
        /// decision D20). Without `edit`, changes the selected edit's, as U
        /// does.
        #[serde(default)]
        side: Option<TrimSide>,
        /// Deselect the timeline's clips, cues and edit first, as
        /// Ctrl+Shift+A does.
        #[serde(default)]
        clear: bool,
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
    /// Change the window: inner `size` and outer `position` in logical pixels
    /// (`[w, h]`, `[x, y]`; Wayland can't place a window), minimize (Wayland
    /// can't un-minimize), maximize, fullscreen.
    #[serde(rename = "gui.window")]
    GuiWindow {
        #[serde(default)]
        size: Option<[f32; 2]>,
        #[serde(default)]
        position: Option<[f32; 2]>,
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
    /// `jobs`, `mixer`, `preferences`, `history` (the undo history),
    /// `help` (the keyboard shortcuts), `transcription` (the words of the
    /// source clip or the timeline), `metadata` (the strip at the top of the
    /// Media Pool), `clip_info` (the media selected in the pool), `titles`,
    /// `inspector` or `keyframes` (tabs in the Media Pool's place), or the
    /// viewer's `transform`, `crop` or `dynamic_zoom` handles on the selected
    /// clip (the topmost clip at the playhead when none is selected), or
    /// `tools`, the viewer's row of Transform, Crop, Dynamic Zoom and their
    /// buttons. Answers what `gui.state` does.
    #[serde(rename = "gui.show")]
    GuiShow { panel: GuiPanel },
    /// Close a window `gui.show` opens. Answers what `gui.state` does.
    #[serde(rename = "gui.close")]
    GuiClose { panel: GuiPanel },
    /// What only the GUI keeps: the edit point selected, the target track,
    /// the timeline's zoom and Timeline Options, and the windows open.
    #[serde(rename = "gui.state")]
    GuiState {},
    /// What an edit takes from the source. The toggles beside the upper timeline.
    /// `video_only` drops the sound. `audio_only` drops the picture.
    /// The two cannot both be on. Setting one on turns the other off.
    /// A field left out stays as it is.
    /// The choice lasts for this session. It is not in the project.
    #[serde(rename = "gui.only")]
    GuiOnly {
        #[serde(default)]
        video_only: Option<bool>,
        #[serde(default)]
        audio_only: Option<bool>,
    },
    /// The target track, as a click on a track header's name picks it:
    /// edit functions put clips there (spec D3). Answers what `gui.state`
    /// does.
    #[serde(rename = "gui.target")]
    GuiTarget { track: String },
    /// The timeline's zoom and Timeline Options, kept between runs.
    /// Answers what `gui.state` does.
    #[serde(rename = "gui.timeline")]
    GuiTimeline(TimelineOptions),
    /// The Trim Editor (spec §8.6): while an edit is selected and the
    /// player is stopped, the viewer shows the outgoing clip's last frame
    /// beside the incoming clip's first, over their filmstrips. This
    /// selects the edit of `track` (default V1) nearest `at`, as a click on
    /// it does, or, without `at`, keeps the selected edit (else the one
    /// nearest the playhead). `transition` shows that edit's dissolve
    /// between the filmstrips, as a click on the dissolve does. `by` then
    /// trims the edit as the editor's buttons do: its Out, its In or both,
    /// a roll (`side`). `close` deselects the edit, closing the editor.
    /// `player.state` says what the two-up shows (`two_up`).
    #[serde(rename = "gui.trim_editor")]
    GuiTrimEditor {
        #[serde(default)]
        at: Option<TimeSpec>,
        #[serde(default)]
        track: Option<String>,
        #[serde(default)]
        by: Option<i64>,
        #[serde(default)]
        side: TrimSide,
        #[serde(default)]
        transition: bool,
        #[serde(default)]
        close: bool,
    },
    /// Dynamic Trim (W, spec D32). `on` turns it on, on the selected edit
    /// or clips (else the edit nearest the playhead), and moves the
    /// playhead there; off applies what it trimmed and deselects the edit.
    /// `side` is what it trims of an edit, and `mode` what it does with
    /// selected clips. `run` trims at that speed, as L and J do (negative
    /// backward, up to 64×), playing with sound what the trim changes under
    /// the playhead, and `0` stops and applies, as K does; forward slides
    /// clips right and slips their content right, to earlier frames.
    /// `step` trims that many frames and applies, as K held with a tap of L
    /// or J does. `player.state` says what the two-up shows (`two_up`).
    #[serde(rename = "gui.dynamic_trim")]
    GuiDynamicTrim {
        #[serde(default)]
        on: Option<bool>,
        #[serde(default)]
        side: Option<TrimSide>,
        #[serde(default)]
        mode: Option<DynamicTrimMode>,
        #[serde(default)]
        run: Option<f64>,
        #[serde(default)]
        step: Option<i64>,
    },
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
    /// Reset vidcut to a fresh install's state: the preferences, the caches
    /// (thumbnails, waveforms, measurements, encoder detection, shaders),
    /// the undo histories (but those of projects a running GUI has open),
    /// the recent projects, view settings, the window's size and place, and
    /// remembered folders. Projects,
    /// media and proxies stay, but those in vidcut's cache: made without a
    /// project, into no proxy folder of your own. It refuses while another vidcut GUI runs, which
    /// would write its settings back: reset from that GUI's Preferences.
    /// `--dry-run` lists what would go.
    #[serde(rename = "reset")]
    Reset {
        /// Also delete what you saved: presets, themes and looks.
        #[serde(default)]
        saved: bool,
        /// Keep what was downloaded or paid for: the speech models, and the
        /// paid services' replies kept to use again.
        #[serde(default)]
        keep_downloads: bool,
    },
}

/// What to export and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExportParams {
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
    /// The subtitle tracks (`S1`) that go out with the export, as
    /// `subtitles_as` says. Files beside media start at zero; beside a
    /// timeline file (`.otio`) they are in its timecode. Default: as files,
    /// none beside media and every track with cues beside a timeline file;
    /// as a stream or burned in, the shown track.
    #[serde(default)]
    pub subtitles: Option<Vec<String>>,
    /// How `subtitles` go out with media. Default: `file`.
    #[serde(default)]
    pub subtitles_as: Option<SubtitlesAs>,
    /// The subtitle files' format. Default: SRT.
    #[serde(default)]
    pub subtitle_format: Option<SubtitleFormat>,
    /// Download Voice Isolation's model if the export needs it and this
    /// computer doesn't have it yet, checked against its SHA-256. Without
    /// it, a missing model is an error that says what would be downloaded.
    #[serde(default)]
    pub download: bool,
}

/// How the speech to transcribe is found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FindSpeech {
    /// The built-in voice-activity model hears where people speak.
    Voice,
    /// All sound louder than a level: singing and speech over music too.
    #[default]
    Sound,
    /// All of the audio, the silences too.
    None,
}

/// How subtitle tracks go out with an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SubtitlesAs {
    /// A file beside the output for each track (`subtitle_format`).
    File,
    /// A subtitle stream in the output for each track: mov_text in MP4, MOV
    /// and M4A, SubRip in Matroska, WebVTT in WebM.
    Stream,
    /// One track drawn into the picture.
    Burn,
}

/// What kind of track `track.add` makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NewTrack {
    Video,
    Audio,
    Subtitle,
}

/// How a subtitle file's times count frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleTimeBase {
    /// `real`, unless the times with fractions of a second fall on timecode
    /// frames and clearly off the frames of real time; round times (whole
    /// seconds, tenths) count for neither. The answer says which, and why
    /// when it is `label`.
    Auto,
    /// Seconds of real time: frame n at n divided by the frame rate (23.976
    /// counts 01:00:00:00 as 01:00:03.600).
    Real,
    /// Timecode read as a clock: whole seconds and frames at the timecode's
    /// rate (01:00:00:00 is 01:00:00.000 at 23.976).
    Label,
}

/// A word from `analyze.words`, sent back to `edit.ripple_delete_words`.
/// The cut uses `start` and `end`; `text` is ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpokenWord {
    #[serde(default)]
    pub text: String,
    pub start: TimeSpec,
    pub end: TimeSpec,
}

/// A speech recognition model, a GGUF file transcribe.cpp runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SpeechModel {
    /// Parakeet Ultra by Moondream (CC BY 4.0), 740 MB, the default: 25
    /// European languages, times per word. It wants a GPU with 2 GB.
    #[default]
    #[serde(rename = "parakeet-ultra")]
    ParakeetUltra,
    /// Qwen3-ASR 1.7B (Apache-2.0), 2.2 GB: 30 languages. It gives no times
    /// until transcribe.cpp runs its aligner, so its text is spread over the
    /// speech in pieces of 10 s at the most, and its cues' times are
    /// estimates. It wants a GPU with 8 GB.
    #[serde(rename = "qwen3-asr")]
    Qwen3Asr,
    /// Whisper large-v3 (Apache-2.0), 1.7 GB: 100 languages, times per
    /// phrase. It wants a GPU with 4 GB.
    #[serde(rename = "whisper-large-v3")]
    WhisperLargeV3,
    /// xAI's `grok-voice-transcribe-2.0`, on xAI's servers: 38 languages,
    /// times per word, $0.10 per hour of audio (October 2026). Everything
    /// heard goes in one request, 500 MB at most. It needs an xAI key and
    /// `send`.
    #[serde(rename = "xai")]
    Xai,
    /// Google's `gemini-3.5-transcribe`, on Google's servers: 85 languages
    /// and more, times per word, about $0.30 per hour of audio (October
    /// 2026), 30 minutes of speech at most with word times, which vidcut
    /// always asks for: more is refused before anything is sent. It tells up
    /// to 8 speakers apart, 3 or more experimentally. It needs a Gemini key
    /// and `send`.
    #[serde(rename = "google")]
    Google,
}

impl SpeechModel {
    pub fn times_words(self) -> bool {
        matches!(self, Self::ParakeetUltra | Self::Xai | Self::Google)
    }

    pub fn paid(self) -> bool {
        matches!(self, Self::Xai | Self::Google)
    }

    pub fn dollars_per_hour(self) -> f64 {
        match self {
            Self::Xai => 0.10,
            Self::Google => 0.30,
            _ => 0.0,
        }
    }

    pub fn longest_secs(self) -> Option<f64> {
        match self {
            Self::Google => Some(30.0 * 60.0),
            _ => None,
        }
    }

    pub fn max_speakers(self) -> Option<u32> {
        match self {
            Self::Google => Some(8),
            _ => None,
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::ParakeetUltra => "Parakeet Ultra",
            Self::Qwen3Asr => "Qwen3-ASR 1.7B",
            Self::WhisperLargeV3 => "Whisper large-v3",
            Self::Xai => "xAI Grok Transcribe",
            Self::Google => "Gemini 3.5 Transcribe",
        }
    }

    pub const fn company(self) -> &'static str {
        match self {
            Self::Xai => "xAI",
            Self::Google => "Google",
            _ => "",
        }
    }
}

/// The model on this computer that tells speakers apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpeakerModel {
    /// MOSS-Transcribe-Diarize in English and Chinese, else Sortformer.
    #[default]
    Auto,
    /// MOSS-Transcribe-Diarize, 987 MB: English and Chinese, many speakers.
    Moss,
    /// NVIDIA's Sortformer, 139 MB: any language, up to 4 speakers.
    Sortformer,
}

impl SpeakerModel {
    pub fn max_speakers(self) -> Option<u32> {
        match self {
            Self::Sortformer => Some(4),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Moss => "moss",
            Self::Sortformer => "sortformer",
        }
    }
}

/// A paid speech service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpeechService {
    Xai,
    Google,
}

impl SpeechService {
    pub fn model(self) -> SpeechModel {
        match self {
            Self::Xai => SpeechModel::Xai,
            Self::Google => SpeechModel::Google,
        }
    }
}

/// xAI's speech-to-text options, by xAI's own names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct XaiOptions {
    /// Default: `grok-voice-transcribe-2.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Names and words to bias the transcript toward: 100 at most, 50
    /// characters each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyterm: Vec<String>,
    /// Numbers as digits ("$100" for "one hundred dollars"). It needs
    /// `language`, and works in Arabic, Chinese, English, French, German,
    /// Japanese, Portuguese, Russian, Spanish, Swedish and Vietnamese.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub format: bool,
    /// Keep filler words such as "uh" and "um". Default: true, so Delete
    /// Filler Words finds them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filler_words: Option<bool>,
    /// xAI's own voice gate, 0 to 1: quieter speech is skipped, and 0
    /// hears everything sent. Default: xAI's, 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_threshold: Option<f64>,
    /// Each channel of the sound on its own, up to 8, the words merged by
    /// time; with `speakers`, a word's speaker is its channel.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multichannel: bool,
}

/// Gemini's transcription options, by Google's own names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GoogleOptions {
    /// Default: `gemini-3.5-transcribe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The languages spoken, as BCP-47 codes such as `fi-FI`, for speech
    /// that switches between them. Default: `language`, else Gemini tells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_codes: Vec<String>,
}

/// Where speech is heard from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HearFrom {
    /// Each heard clip's file on its own, as Transcribe Audio hears it, its
    /// words kept with the file and placed through the clip.
    Media,
    /// The mix of the tracks heard.
    Mix,
    /// The words already kept for each heard clip's file, whichever model
    /// made them, placed through the clips; a file without any is
    /// transcribed first, as with `media`.
    Transcript,
}

/// A subtitle file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleFormat {
    /// SubRip (`.srt`).
    Srt,
    /// WebVTT (`.vtt`).
    Vtt,
    /// TTML, written as IMSC1 (`.ttml`).
    Ttml,
}

/// A part of a clip Ripple Delete Silence cuts: timeline frames
/// `start..end`, `end` excluded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClipPart {
    pub clip: ClipRef,
    pub start: TimeSpec,
    pub end: TimeSpec,
}

/// What Ripple Delete Silence counts as silence (deviation X47).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Detect {
    /// Sound below `threshold_db`.
    #[default]
    Level,
    /// Whatever the speech detector (Silero VAD, on the CPU) doesn't hear as
    /// speech: applause and room noise too.
    Speech,
}

/// The value that keyframes set (decisions D75 and D81).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyParam {
    /// The volume of a clip's sound, in dB.
    #[default]
    Volume,
    /// Position X and Y, one keyframe for both, as fractions of the frame.
    Position,
    /// Zoom X and Y, one keyframe for both. One number in `value` sets both.
    Zoom,
    /// Rotation in degrees.
    Rotation,
    /// Crop Left, as a fraction of the width.
    CropLeft,
    /// Crop Right, as a fraction of the width.
    CropRight,
    /// Crop Top, as a fraction of the height.
    CropTop,
    /// Crop Bottom, as a fraction of the height.
    CropBottom,
}

/// A keyframe value: one number, or x and y for position and zoom.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum KeyValue {
    /// One number.
    Scalar(f64),
    /// X then Y.
    Pair([f64; 2]),
}

impl KeyValue {
    pub fn pair(self) -> [f64; 2] {
        match self {
            KeyValue::Scalar(n) => [n, n],
            KeyValue::Pair(v) => v,
        }
    }

    pub fn scalar(self) -> Option<f64> {
        match self {
            KeyValue::Scalar(n) => Some(n),
            KeyValue::Pair(_) => None,
        }
    }
}

/// What `clip.normalize` measures (deviation X72).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NormalizeMode {
    /// Integrated loudness (EBU R128, ITU-R BS.1770), in LUFS: how loud the
    /// clip sounds over its length.
    #[default]
    Loudness,
    /// The highest sample, in dBFS.
    SamplePeak,
    /// The highest true peak, between the samples too (ITU-R BS.1770), in
    /// dBTP.
    TruePeak,
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

/// How the viewer's timecode field counts (decision D91).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimecodeDisplay {
    #[default]
    Auto,
    SourceTimecode,
    SourceFrame,
    RecordTimecode,
    RecordFrame,
    Timer1,
    Timer2,
}

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

/// How much of the window the viewer fills.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewerFrame {
    #[default]
    Page,
    Enhanced,
    FullScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerFrameKey {
    FullScreen,
    Enhanced,
    Escape,
}

pub fn viewer_frame_after(frame: ViewerFrame, key: ViewerFrameKey) -> ViewerFrame {
    match key {
        ViewerFrameKey::Escape => ViewerFrame::Page,
        ViewerFrameKey::FullScreen => match frame {
            ViewerFrame::FullScreen => ViewerFrame::Page,
            ViewerFrame::Page | ViewerFrame::Enhanced => ViewerFrame::FullScreen,
        },
        ViewerFrameKey::Enhanced => match frame {
            ViewerFrame::Enhanced => ViewerFrame::Page,
            ViewerFrame::Page | ViewerFrame::FullScreen => ViewerFrame::Enhanced,
        },
    }
}

/// What a trim of an edit moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrimSide {
    /// The outgoing clip's Out.
    Out,
    /// The incoming clip's In.
    In,
    /// Both: a roll.
    #[default]
    Both,
    /// Its audio edit: where the outgoing clip's sound ends, the J/L
    /// handle (decision D20).
    Audio,
}

/// What `gui.timeline` changes (omitted: stays as it is).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimelineOptions {
    /// The lower timeline's zoom: pixels a frame, 0.01 to 40.
    #[serde(default)]
    pub zoom: Option<f64>,
    /// The lower timeline's playhead stays in the middle (decision D1).
    #[serde(default)]
    pub fixed_playhead: Option<bool>,
    /// Snap (N).
    #[serde(default)]
    pub snap: Option<bool>,
    /// Trim with Safe Edit: a trim stops at the neighboring clip.
    #[serde(default)]
    pub safe_edit: Option<bool>,
    /// Display Clip Names.
    #[serde(default)]
    pub names: Option<bool>,
    /// Display Clip Durations.
    #[serde(default)]
    pub durations: Option<bool>,
    /// Display Clip Status.
    #[serde(default)]
    pub status: Option<bool>,
    #[serde(default)]
    pub filmstrips: Option<bool>,
    #[serde(default)]
    pub waveforms: Option<bool>,
    /// The Boring Detector (decision D31).
    #[serde(default)]
    pub boring: Option<bool>,
    /// Its Boring Clips: clips longer than this many seconds, 1 to 300.
    #[serde(default)]
    pub boring_secs: Option<f64>,
    /// Its Jump Cuts: clips shorter than this many frames, 1 to 120.
    #[serde(default)]
    pub jump_frames: Option<i64>,
    /// Minimize Subtitle Track: the subtitle tracks drawn shorter.
    #[serde(default)]
    pub minimize_subtitles: Option<bool>,
}

/// What Dynamic Trim does with selected clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DynamicTrimMode {
    /// The clip stays and shows other frames of its media.
    Slip,
    /// The clip keeps its media and moves between its neighbors.
    Slide,
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
    /// The Mixer: track faders, mute, solo and meters.
    Mixer,
    /// Application preferences.
    Preferences,
    /// The keyboard shortcuts.
    Help,
    /// The undo history.
    History,
    /// The Transcription window: the words of the source clip or the
    /// timeline, from their media's transcripts.
    Transcription,
    /// The metadata strip at the top of the Media Pool: the selected
    /// media's format at a glance.
    Metadata,
    /// Clip Info: all that is known of the media selected in the pool, its
    /// file read again (`media.details`).
    ClipInfo,
    /// The Titles tab, in the Media Pool's place: the kinds of title to
    /// drag onto the timeline.
    Titles,
    /// The Inspector tab, in the Media Pool's place: the transform, crop
    /// and Dynamic Zoom of the selected clips, as numbers to type or drag.
    Inspector,
    /// The Keyframes tab, in the Media Pool's place: the keyed values of
    /// the first selected clip.
    Keyframes,
    /// The viewer's Transform handles on the selected clip: move, zoom,
    /// stretch and turn its picture.
    Transform,
    /// The viewer's Crop handles on the selected clip, one on each side.
    Crop,
    /// The viewer's Dynamic Zoom boxes on the selected clip: the start box
    /// and the end box.
    DynamicZoom,
    /// The viewer's Tools row: Transform, Crop and Dynamic Zoom, the flips,
    /// Swap, the ease and Reset.
    Tools,
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
    /// The label of the file's start timecode; media without one count
    /// as 00:00:00:00 (decision D59).
    Timecode,
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

fn silence_threshold() -> f64 {
    -40.0
}

fn speech_threshold() -> f64 {
    0.5
}

fn min_speech() -> TimeSpec {
    TimeSpec::Seconds(0.25)
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
    /// How long a still lasts when edited without a range, e.g. `5s`.
    #[serde(default)]
    pub still_duration: Option<TimeSpec>,
    /// Play fresh proxies in the preview. Export still reads the originals.
    #[serde(default)]
    pub use_proxies: Option<bool>,
    /// Filler words of this project, per language code (`{"en": ["um",
    /// "uh"]}`): a list replaces the preferences' or built-in one for that
    /// language, `null` goes back to them.
    #[serde(default)]
    pub fillers: Option<std::collections::BTreeMap<String, Option<Vec<String>>>>,
}

/// A change to a clip's Dynamic Zoom: each field given replaces its own,
/// and the rest stay as they are. A clip without one starts from a start
/// box 0.8 of the picture and an end box all of it, both in the middle,
/// `linear`: a pull out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DynamicZoomSet {
    /// The box on the clip's first frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<ZoomBoxSet>,
    /// The box a frame after its last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<ZoomBoxSet>,
    /// How it goes from one to the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Ease>,
}

/// A change to a box of a Dynamic Zoom, which has the picture's shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoomBoxSet {
    /// Its middle, `[x, y]` as fractions of the picture from its top left,
    /// as it shows after the flips, each from -100 to 100: `[0.5, 0.5]` is
    /// the picture's middle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    /// Its width and height as a fraction of the picture's, from 1/100 to
    /// 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
}

impl ZoomBoxSet {
    fn on(&self, b: ZoomBox) -> ZoomBox {
        ZoomBox {
            center: self.center.unwrap_or(b.center),
            size: self.size.unwrap_or(b.size),
        }
    }
}

impl DynamicZoomSet {
    pub fn on(&self, z: DynamicZoom) -> DynamicZoom {
        DynamicZoom {
            start: self.start.map_or(z.start, |b| b.on(z.start)),
            end: self.end.map_or(z.end, |b| b.on(z.end)),
            ease: self.ease.unwrap_or(z.ease),
        }
    }
}

/// A change to a track's ducking: each field given replaces its own, and
/// the rest stay as they are. A new one is on, under every other track
/// with sound, at the usual values: Duck Amount 2.7 dB, Lookahead 15 ms,
/// Rise Time 10 ms, Hold 150 ms, Recovery Time 750 ms, by speech, and from
/// -42 dBFS by level.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuckSet {
    /// On or off. Off keeps the settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// The tracks whose sound dips it (`V1`, `A2`), never its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    /// The Duck Amount: how far it dips, in dB, from 0 to 60.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_db: Option<f64>,
    /// How long before a source starts the full dip is reached, in ms,
    /// from 0 to 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookahead_ms: Option<f64>,
    /// The Rise Time: how long the gain takes to fall to the full dip, in
    /// ms, from 1 to 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rise_ms: Option<f64>,
    /// How long the full dip stays after a source stops, in ms, from 0 to
    /// 5000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_ms: Option<f64>,
    /// The Recovery Time: how long the gain takes to come back after the
    /// hold, in ms, from 1 to 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_ms: Option<f64>,
    /// What makes a source active: `speech`, or `level`, its sound at or
    /// above `threshold_db`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detect: Option<DuckDetect>,
    /// With `level`: the peak in dBFS, after the source's clip gain and
    /// fader, from which it is active, from -96 to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold_db: Option<f64>,
}

fn given<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// One value or a list (`"c1"` or `["c1", "c2"]`).
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

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

pub fn protocol_schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::to_value(schemars::schema_for!(Request)).expect("protocol schema")
    })
}

pub fn method_names(variant: &serde_json::Value) -> Vec<&str> {
    let method = &variant["properties"]["method"];
    match method.get("const").and_then(serde_json::Value::as_str) {
        Some(name) => vec![name],
        None => method
            .get("enum")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect(),
    }
}

pub fn resolve_ref<'a>(
    schema: &'a serde_json::Value,
    defs: &'a serde_json::Map<String, serde_json::Value>,
) -> &'a serde_json::Value {
    static NULL: serde_json::Value = serde_json::Value::Null;
    let mut schema = schema;
    for _ in 0..8 {
        match schema
            .get("$ref")
            .and_then(serde_json::Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
        {
            Some(name) => schema = defs.get(name).unwrap_or(&NULL),
            None => break,
        }
    }
    schema
}

fn method_params() -> &'static HashMap<String, HashSet<String>> {
    static PARAMS: OnceLock<HashMap<String, HashSet<String>>> = OnceLock::new();
    PARAMS.get_or_init(|| {
        let root = protocol_schema();
        let empty = serde_json::Map::new();
        let defs = root["$defs"].as_object().unwrap_or(&empty);
        let mut out = HashMap::new();
        for variant in root["oneOf"].as_array().into_iter().flatten() {
            let mut names = HashSet::new();
            if let Some(params) = variant.pointer("/properties/params") {
                parameter_names(params, defs, &mut names, 0);
            }
            for method in method_names(variant) {
                out.insert(method.to_string(), names.clone());
            }
        }
        out
    })
}

fn parameter_names(
    schema: &serde_json::Value,
    defs: &serde_json::Map<String, serde_json::Value>,
    out: &mut HashSet<String>,
    depth: u8,
) {
    if depth == 8 {
        return;
    }
    let schema = resolve_ref(schema, defs);
    if let Some(props) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        out.extend(props.keys().cloned());
    }
    for key in ["allOf", "anyOf", "oneOf"] {
        let Some(items) = schema.get(key).and_then(serde_json::Value::as_array) else {
            continue;
        };
        for item in items {
            parameter_names(item, defs, out, depth + 1);
        }
    }
}

#[derive(Deserialize)]
struct Batch {
    commands: Vec<serde_json::Value>,
    label: Option<String>,
}

fn batch(params: serde_json::Value) -> Result<Request, String> {
    let Batch { commands, label } =
        serde_json::from_value(params).map_err(|e| format!("invalid params for apply: {e}"))?;
    let commands = commands
        .into_iter()
        .enumerate()
        .map(|(i, command)| batched(command).map_err(|e| format!("entry {}: {e}", i + 1)))
        .collect::<Result<_, _>>()?;
    Ok(Request::Apply { commands, label })
}

fn batched(command: serde_json::Value) -> Result<Request, String> {
    let envelope = Envelope::from_json(command)?;
    if envelope.if_rev.is_some() || envelope.dry_run {
        return Err("put dry_run and if_rev on apply, not on its commands".into());
    }
    Ok(envelope.request)
}

impl Request {
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

    pub fn from_parts(method: &str, params: Option<serde_json::Value>) -> Result<Self, String> {
        let Some(known) = method_params().get(method) else {
            return Err(format!("unknown method {method:?}; see `vidcut schema`"));
        };
        let params = match params {
            None | Some(serde_json::Value::Null) => serde_json::json!({}),
            Some(params) => params,
        };
        if let Some(given) = params.as_object() {
            let unknown: Vec<&str> = given
                .iter()
                .filter(|(key, value)| !value.is_null() && !known.contains(key.as_str()))
                .map(|(key, _)| key.as_str())
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "unknown parameter(s) {} for {method}; see `vidcut schema {method}`",
                    unknown.join(", ")
                ));
            }
        }
        if method == "apply" {
            return batch(params);
        }
        serde_json::from_value(serde_json::json!({ "method": method, "params": params }))
            .map_err(|e| format!("invalid params for {method}: {e}"))
    }

    pub fn is_live(&self) -> bool {
        matches!(
            self,
            Self::ProjectOpen { .. }
                | Self::ViewerSet { .. }
                | Self::ViewerFrame { .. }
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
                | Self::PlayerLoop { .. }
                | Self::PlayerFastReview { .. }
                | Self::MixerPreview { .. }
                | Self::PoolSet { .. }
                | Self::SelectionGet {}
                | Self::SelectionSet { .. }
                | Self::GuiScreenshot { .. }
                | Self::GuiLog { .. }
                | Self::GuiWindow { .. }
                | Self::GuiQuit {}
                | Self::GuiShow { .. }
                | Self::GuiClose { .. }
                | Self::GuiState {}
                | Self::GuiOnly { .. }
                | Self::GuiTarget { .. }
                | Self::GuiTimeline(_)
                | Self::GuiTrimEditor { .. }
                | Self::GuiDynamicTrim { .. }
                | Self::JobsList {}
                | Self::JobsGet { .. }
                | Self::JobsCancel { .. }
                | Self::JobsWait { .. }
        )
    }

    pub fn is_projectless(&self) -> bool {
        matches!(
            self,
            Self::Schema { .. }
                | Self::Doctor {}
                | Self::Reset { .. }
                | Self::Probe { .. }
                | Self::ProjectCreate { .. }
                | Self::PresetsList {}
                | Self::PresetsGet { .. }
                | Self::PresetsSave { .. }
                | Self::PresetsDelete { .. }
                | Self::LooksList { .. }
                | Self::LooksSave { .. }
                | Self::LooksDelete { .. }
                | Self::EncodersList { .. }
                | Self::DecodersList {}
                | Self::PreferencesGet {}
                | Self::PreferencesSet(_)
                | Self::PreferencesReset {}
                | Self::ViewerGuides { .. }
                | Self::KeysList {}
                | Self::ServicesSetKey { .. }
                | Self::ServicesRemoveKey { .. }
        )
    }

    pub fn is_mutation(&self) -> bool {
        !self.is_live()
            && !matches!(
                self,
                Self::ProjectGet {}
                    | Self::SpeakersList {}
                    | Self::ProjectSummary {}
                    | Self::ProjectCreate { .. }
                    | Self::TrackList {}
                    | Self::BinsList {}
                    | Self::MarksGet {}
                    | Self::KeyframesList { .. }
                    | Self::MarkersList {}
                    | Self::SubtitlesList { .. }
                    | Self::TitlesList { .. }
                    | Self::PictureGet { .. }
                    | Self::SubtitlesExport { .. }
                    | Self::AnalyzeWords { .. }
                    | Self::MediaTranscribe { .. }
                    | Self::MediaList {}
                    | Self::MediaGet { .. }
                    | Self::MediaDetails { .. }
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
                    | Self::HistoryGet { .. }
                    | Self::Schema { .. }
                    | Self::Doctor {}
                    | Self::Reset { .. }
                    | Self::ExportPlan(_)
                    | Self::ExportStart(_)
                    | Self::FrameRender { .. }
                    | Self::PresetsList {}
                    | Self::PresetsGet { .. }
                    | Self::PresetsSave { .. }
                    | Self::PresetsDelete { .. }
                    | Self::LooksList { .. }
                    | Self::LooksSave { .. }
                    | Self::LooksDelete { .. }
                    | Self::EncodersList { .. }
                    | Self::DecodersList {}
                    | Self::PreferencesGet {}
                    | Self::PreferencesSet(_)
                    | Self::PreferencesReset {}
                    | Self::ViewerGuides { .. }
                    | Self::KeysList {}
                    | Self::ServicesSetKey { .. }
                    | Self::ServicesRemoveKey { .. }
                    | Self::EditCopy { .. }
            )
    }

    pub fn paths_mut(&mut self) -> Vec<&mut PathBuf> {
        match self {
            Self::MediaAdd { paths, .. } => paths.iter_mut().collect(),
            Self::MediaRelink { path, search, .. } => {
                path.iter_mut().chain(search.iter_mut()).collect()
            }
            Self::MediaAddFolder { path, .. }
            | Self::Probe { path }
            | Self::SubtitlesExport { out: path, .. } => vec![path],
            Self::SubtitlesImport { path, .. } => path.iter_mut().collect(),
            Self::SubtitlesTranscribe {
                model_file,
                vad_file,
                ..
            }
            | Self::AnalyzeWords {
                model_file,
                vad_file,
                ..
            }
            | Self::MediaTranscribe {
                model_file,
                vad_file,
                ..
            } => model_file.iter_mut().chain(vad_file.iter_mut()).collect(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    Gui,
    Cli,
    Mcp,
    Ipc,
    File,
    #[serde(other)]
    Other,
}

impl ClientKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Gui => "GUI",
            Self::Cli => "CLI",
            Self::Mcp => "MCP",
            Self::Ipc => "IPC",
            Self::File => "File",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Client {
    pub kind: ClientKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Client {
    pub fn new(kind: ClientKind, name: Option<String>) -> Self {
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        Self { kind, name }
    }

    pub fn shown(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => self.kind.name().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub request: Request,
    pub if_rev: Option<u64>,
    pub dry_run: bool,
    pub client: Option<Client>,
}

impl Envelope {
    pub fn new(request: Request) -> Self {
        Self {
            request,
            if_rev: None,
            dry_run: false,
            client: None,
        }
    }

    pub fn by(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    pub fn normalized(mut self) -> Self {
        if self.dry_run
            && let Request::ExportStart(params) = self.request
        {
            self.request = Request::ExportPlan(params);
        }
        self
    }

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

    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        let serde_json::Value::Object(mut obj) = value else {
            return Err("a request must be a JSON object with \"method\"".into());
        };
        let method = obj
            .remove("method")
            .and_then(|m| m.as_str().map(str::to_string))
            .ok_or("missing \"method\"")?;
        let client = match obj.remove("client") {
            Some(v) => Some(
                serde_json::from_value::<Client>(v)
                    .map_err(|e| format!("client: {e}"))
                    .map(|c| Client::new(c.kind, c.name))?,
            ),
            None => None,
        };
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
            client,
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
    fn an_unknown_method_names_the_method() {
        let e = Request::from_parts("no.such", None).unwrap_err();
        assert_eq!(e, "unknown method \"no.such\"; see `vidcut schema`");
    }

    #[test]
    fn keyframes_parses_as_a_panel() {
        let request = Request::from_parts(
            "gui.show",
            Some(serde_json::json!({ "panel": "keyframes" })),
        )
        .unwrap();
        assert_eq!(
            request,
            Request::GuiShow {
                panel: GuiPanel::Keyframes,
            }
        );
    }

    #[test]
    fn a_bad_panel_is_invalid_params() {
        let e = Request::from_parts("gui.show", Some(serde_json::json!({ "panel": "log" })))
            .unwrap_err();
        assert!(
            e.starts_with("invalid params for gui.show: unknown variant `log`, expected "),
            "{e}"
        );
    }

    #[test]
    fn every_parameter_given_is_known() {
        use serde_json::json;
        for params in [
            json!({ "auto": false }),
            json!({ "media": null, "width": null, "auto": false }),
            json!({ "media": ["m1"], "auto": true }),
        ] {
            Request::from_parts("proxy.generate", Some(params.clone()))
                .unwrap_or_else(|e| panic!("{params}: {e}"));
        }
        for params in [json!({ "autoo": true }), json!({ "autoo": false })] {
            let e = Request::from_parts("proxy.generate", Some(params)).unwrap_err();
            assert_eq!(
                e,
                "unknown parameter(s) autoo for proxy.generate; see `vidcut schema proxy.generate`"
            );
        }
    }

    #[test]
    fn an_empty_search_list_is_a_parameter() {
        let request =
            Request::from_parts("media.relink", Some(serde_json::json!({ "search": [] }))).unwrap();
        assert_eq!(
            request,
            Request::MediaRelink {
                media: None,
                path: None,
                search: Vec::new(),
            }
        );
    }

    #[test]
    fn null_params_are_missing_params() {
        let request = Request::from_parts("project.get", Some(serde_json::Value::Null)).unwrap();
        assert_eq!(request, Request::ProjectGet {});
    }

    #[test]
    fn a_bad_parameter_in_apply_names_the_entry() {
        let e = Request::from_parts(
            "apply",
            Some(serde_json::json!({
                "commands": [
                    {"method": "project.get", "params": {}},
                    {"method": "proxy.generate", "params": {"autoo": true}}
                ]
            })),
        )
        .unwrap_err();
        assert_eq!(
            e,
            "entry 2: unknown parameter(s) autoo for proxy.generate; see `vidcut schema proxy.generate`"
        );
    }

    #[test]
    fn an_unknown_method_in_apply_names_the_entry() {
        let e = Request::from_parts(
            "apply",
            Some(serde_json::json!({
                "commands": [
                    {"method": "project.get", "params": {}},
                    {"method": "no.such", "params": {}}
                ]
            })),
        )
        .unwrap_err();
        assert_eq!(
            e,
            "entry 2: unknown method \"no.such\"; see `vidcut schema`"
        );
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

    #[test]
    fn viewer_set_reads_a_count_without_a_mode() {
        let r = Request::from_parts(
            "viewer.set",
            Some(serde_json::json!({ "display": "timer1" })),
        )
        .unwrap();
        assert_eq!(
            r,
            Request::ViewerSet {
                mode: None,
                display: Some(TimecodeDisplay::Timer1),
                reset_timer1: false,
                paste: None,
            }
        );
    }

    #[test]
    fn viewer_guides_round_trips() {
        let request = Request::from_parts(
            "viewer.guides",
            Some(serde_json::json!({ "framing": "social_9_16", "center": true })),
        )
        .unwrap();
        assert!(matches!(
            request,
            Request::ViewerGuides {
                enabled: None,
                framing: Some(crate::preferences::Framing::Social9_16),
                extents: None,
                action: None,
                title: None,
                center: Some(true),
            }
        ));
        let back = serde_json::to_value(&request).unwrap();
        assert_eq!(back["method"], "viewer.guides");
        assert_eq!(back["params"]["framing"], "social_9_16");
        assert_eq!(back["params"]["center"], true);
        assert!(back["params"].get("enabled").is_none());
    }

    #[test]
    fn viewer_guides_with_no_fields_is_empty() {
        let request = Request::from_parts("viewer.guides", Some(serde_json::json!({}))).unwrap();
        assert!(matches!(
            request,
            Request::ViewerGuides {
                enabled: None,
                framing: None,
                extents: None,
                action: None,
                title: None,
                center: None,
            }
        ));
    }

    #[test]
    fn gui_only_reads_video_only() {
        let request =
            Request::from_parts("gui.only", Some(serde_json::json!({ "video_only": true })))
                .unwrap();
        assert_eq!(
            request,
            Request::GuiOnly {
                video_only: Some(true),
                audio_only: None,
            }
        );
    }

    #[test]
    fn fast_review_with_no_field_toggles() {
        let request =
            Request::from_parts("player.fast_review", Some(serde_json::json!({}))).unwrap();
        assert_eq!(request, Request::PlayerFastReview { on: None });
    }

    #[test]
    fn viewer_frame_sets_full_screen() {
        let request = Request::from_parts(
            "viewer.frame",
            Some(serde_json::json!({ "mode": "full_screen" })),
        )
        .unwrap();
        assert_eq!(
            request,
            Request::ViewerFrame {
                mode: ViewerFrame::FullScreen,
            }
        );
    }

    #[test]
    fn viewer_frame_sets_the_enhanced_viewer() {
        let request = Request::from_parts(
            "viewer.frame",
            Some(serde_json::json!({ "mode": "enhanced" })),
        )
        .unwrap();
        assert_eq!(
            request,
            Request::ViewerFrame {
                mode: ViewerFrame::Enhanced,
            }
        );
    }

    #[test]
    fn shift_f_toggles_full_screen() {
        assert_eq!(
            viewer_frame_after(ViewerFrame::Page, ViewerFrameKey::FullScreen),
            ViewerFrame::FullScreen
        );
    }

    #[test]
    fn shift_f_again_returns_to_the_page() {
        assert_eq!(
            viewer_frame_after(ViewerFrame::FullScreen, ViewerFrameKey::FullScreen),
            ViewerFrame::Page
        );
    }

    #[test]
    fn alt_f_from_full_screen_opens_the_enhanced_viewer() {
        assert_eq!(
            viewer_frame_after(ViewerFrame::FullScreen, ViewerFrameKey::Enhanced),
            ViewerFrame::Enhanced
        );
    }

    #[test]
    fn escape_leaves_the_enhanced_viewer() {
        assert_eq!(
            viewer_frame_after(ViewerFrame::Enhanced, ViewerFrameKey::Escape),
            ViewerFrame::Page
        );
    }

    #[test]
    fn viewer_guides_is_a_projectless_preference() {
        let request = Request::from_parts("viewer.guides", Some(serde_json::json!({}))).unwrap();
        assert!(request.is_projectless());
        assert!(!request.is_live());
        assert!(!request.is_mutation());
    }
}
