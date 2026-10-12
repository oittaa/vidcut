//! The `vidcut` command line. Every subcommand builds one [`Request`]; nothing
//! here edits projects directly.

use std::io::Read;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Error, Request, Result};

#[derive(Parser, Debug)]
#[command(
    name = "vidcut",
    version,
    about = "A simple, fully scriptable video cutting tool.",
    long_about = "A simple, fully scriptable video cutting tool.\n\n\
        Times: 450f (frames), 15s, 1m30s, 250ms, HH:MM:SS:FF (timecode), HH:MM:SS.mmm, start, end, in, out.\n\
        Ranges are half-open: A..B (B excluded) or A..+D. Clips: c17, or V1@<time> for the clip under a frame.\n\
        Every command is also a JSON-RPC method: see `vidcut schema`. With --json, output is the response object.",
    after_help = "Examples:\n  vidcut new talk.vidcut --fps 25 --size 1920x1080\n  vidcut -p talk.vidcut media add cam.mp4 screen.mkv\n  vidcut -p talk.vidcut edit append m1 --range 10s..2m\n  vidcut -p talk.vidcut edit split --at 1m,1m30s\n  vidcut -p talk.vidcut edit delete-range 2m..2m04s\n  vidcut -p talk.vidcut timeline --json"
)]
pub struct Cli {
    /// Project file.
    #[arg(short, long, global = true, env = "VIDCUT_PROJECT")]
    pub project: Option<PathBuf>,
    /// Print the response as JSON (errors too, on stdout).
    #[arg(long, global = true)]
    pub json: bool,
    /// Report what would change without changing anything.
    #[arg(long, global = true)]
    pub dry_run: bool,
    /// Fail unless the project is at this revision.
    #[arg(long, global = true, value_name = "REV")]
    pub if_rev: Option<u64>,
    /// Run here even if a GUI has the project open.
    #[arg(long, global = true, conflicts_with = "instance")]
    pub headless: bool,
    /// Send to this running GUI (pid or socket path) instead of choosing one.
    #[arg(long, global = true, env = "VIDCUT_SOCKET", value_name = "PID|SOCKET")]
    pub instance: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new project file.
    New {
        file: PathBuf,
        /// Frame rate (30000/1001, 25, 29.97). Default: from the first video added.
        #[arg(long)]
        fps: Option<String>,
        /// Frame size WIDTHxHEIGHT. Default: from the first video added.
        #[arg(long)]
        size: Option<String>,
        #[arg(long)]
        sample_rate: Option<u32>,
        #[arg(long)]
        channels: Option<u16>,
    },
    /// Project settings, counts and duration.
    Info,
    /// Change project settings.
    #[command(
        after_help = "Examples:\n  vidcut set --ripple false\n  vidcut set --proxies true\n  vidcut set --still-duration 3s"
    )]
    Set(SetArgs),
    /// Show what the media layer knows about a file.
    Probe { file: PathBuf },
    /// Media pool.
    #[command(subcommand)]
    Media(MediaCmd),
    /// Playback proxies: smaller H.264 copies the preview can play.
    /// Export, frames, analysis and OTIO keep reading the originals.
    #[command(subcommand)]
    Proxy(ProxyCmd),
    /// Show the timeline (clips and gaps).
    #[command(visible_alias = "tl")]
    Timeline {
        /// Only this range.
        range: Option<String>,
    },
    /// What is at a frame.
    At { at: String },
    /// V1 edit points.
    Cuts,
    /// Edit the timeline.
    #[command(subcommand)]
    Edit(EditCmd),
    /// Clip properties.
    #[command(subcommand)]
    Clip(ClipCmd),
    /// Timeline In/Out marks.
    #[command(subcommand)]
    Marks(MarksCmd),
    /// Tracks: add and remove them, and their header and mixer settings
    /// (list them without a subcommand).
    Track {
        #[command(subcommand)]
        cmd: Option<TrackCmd>,
    },
    /// Media Pool bins (list them without a subcommand).
    Bins {
        #[command(subcommand)]
        cmd: Option<BinsCmd>,
    },
    /// Timeline markers.
    #[command(subcommand)]
    Markers(MarkersCmd),
    /// Export the timeline (In–Out if both marks are set, else all of it).
    /// With --dry-run, print the plan instead. A `.otio` output writes the
    /// edit as OpenTimelineIO for other editors (the
    /// whole timeline unless --range); `.otioz` bundles the edit with its
    /// media in one zip file, `.otiod` in a folder.
    #[command(
        after_help = "Examples:\n  vidcut export out.mp4\n  vidcut export out.mp4 --preset youtube-1080p --encoder nvenc\n  vidcut export cut.mov --range 1m..2m --set video.quality=14\n  vidcut export clips/ --per-clip --preset prores-422hq\n  vidcut export voice.wav --audio-only\n  vidcut export cut.otio\n  vidcut export cut.otioz\n  vidcut export out.mp4 --background && vidcut jobs wait j1"
    )]
    Export {
        /// Output file (a directory with --per-clip). The extension picks the container.
        output: PathBuf,
        /// Preset (see `vidcut presets`). Default: from the output extension.
        #[arg(long)]
        preset: Option<String>,
        /// Override a setting, e.g. video.quality=20 (repeatable).
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Timeline range (default: In–Out if both are set, else everything).
        #[arg(long, allow_hyphen_values = true)]
        range: Option<String>,
        /// One file per clip, into the output directory.
        #[arg(long)]
        per_clip: bool,
        /// Leave the video out.
        #[arg(long)]
        audio_only: bool,
        /// auto, cpu, nvenc or vaapi.
        #[arg(long)]
        encoder: Option<String>,
        /// Per-clip file name template: {index}, {index:03}, {name}, {clip}, {media}.
        #[arg(long)]
        name: Option<String>,
        /// Replace existing files.
        #[arg(long)]
        overwrite: bool,
        /// Return a job id at once (a running GUI exports; see `vidcut jobs`).
        #[arg(long)]
        background: bool,
    },
    /// Import an OpenTimelineIO timeline (.otio, or a bundle with its media:
    /// .otioz, .otiod), from another editor: media join
    /// the pool, clips go after the last clip (or replace the timeline).
    #[command(
        after_help = "Examples:\n  vidcut new cut.vidcut && vidcut -p cut.vidcut import their-cut.otio\n  vidcut -p cut.vidcut import cut.otio --replace --search /mnt/footage\n  vidcut -p cut.vidcut import cut.otioz --unpack /mnt/fast/cut-media"
    )]
    Import {
        file: PathBuf,
        /// Replace the timeline's clips, markers and marks.
        #[arg(long)]
        replace: bool,
        /// Look for missing media by file name in this directory (repeatable).
        #[arg(long, value_name = "DIR")]
        search: Vec<PathBuf>,
        /// Unpack an .otioz bundle's media into this directory (default: a
        /// directory named after the bundle, next to it).
        #[arg(long, value_name = "DIR")]
        unpack: Option<PathBuf>,
    },
    /// Export jobs of the running GUI: list them, or get, wait for or cancel one.
    Jobs {
        #[command(subcommand)]
        cmd: Option<JobsCmd>,
    },
    /// Measure media or clips: silences, shots, black, loudness.
    #[command(subcommand)]
    Analyze(AnalyzeCmd),
    /// The Boring Detector: clips too long, and too short (jump cuts).
    Boring {
        /// Longer than this is boring [default: 30s].
        #[arg(long)]
        boring: Option<String>,
        /// Shorter than this is a jump cut [default: 10f].
        #[arg(long)]
        jump: Option<String>,
    },
    /// A storyboard PNG: a labeled picture of every shot (or every --every).
    #[command(
        after_help = "Examples:\n  vidcut storyboard -o board.png\n  vidcut storyboard -o board.png --every 10s --columns 6 --width 240"
    )]
    Storyboard {
        #[arg(short, long)]
        out: PathBuf,
        /// Pictures per row [default: 4].
        #[arg(long)]
        columns: Option<u32>,
        /// Width of each picture [default: 320].
        #[arg(long)]
        width: Option<u32>,
        /// A picture every this often instead of one per shot.
        #[arg(long)]
        every: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        range: Option<String>,
    },
    /// Render one frame as PNG (the timeline, or a media item with --media).
    Frame {
        #[arg(long, allow_hyphen_values = true)]
        at: String,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        media: Option<String>,
        /// Width in pixels; height follows the aspect.
        #[arg(long)]
        width: Option<u32>,
    },
    /// List export presets, show one, or save and delete your own.
    #[command(args_conflicts_with_subcommands = true)]
    Presets {
        #[command(subcommand)]
        cmd: Option<PresetsCmd>,
        /// Show this preset.
        name: Option<String>,
    },
    /// Show or change application preferences
    /// (~/.config/vidcut/preferences.toml).
    #[command(
        after_help = "Examples:\n  vidcut preferences\n  vidcut preferences set thumbnails.cache_limit_mb=256\n  vidcut preferences set thumbnails.disk_cache=false   # also empties the cache"
    )]
    Preferences {
        #[command(subcommand)]
        cmd: Option<PreferencesCmd>,
    },
    /// Which GPU encoders work here (cached).
    Encoders {
        /// Test again instead of using the cache.
        #[arg(long)]
        refresh: bool,
    },
    /// Undo the last change(s).
    Undo {
        #[arg(default_value_t = 1)]
        steps: u32,
    },
    /// Redo undone change(s).
    Redo {
        #[arg(default_value_t = 1)]
        steps: u32,
    },
    /// Undo/redo history.
    History,
    /// Apply requests from a file (JSON Lines or a JSON array; - for stdin)
    /// atomically, as one undo step.
    Apply {
        file: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// JSON Schema of the protocol, or of one method's params.
    Schema { method: Option<String> },
    /// Send one raw request: `vidcut rpc edit.split '{"at": "1m"}'`.
    Rpc {
        method: String,
        params: Option<String>,
    },
    /// Check dependencies (FFmpeg, GPUs).
    Doctor,

    // ------------------------------------------------------------ the GUI
    /// Start the GUI (also what `vidcut` without a command does).
    Gui {
        /// Project to open (created if missing). Default: -p / VIDCUT_PROJECT.
        file: Option<PathBuf>,
    },
    /// List running GUIs (live commands go to them).
    Instances,
    /// Serve the Model Context Protocol on stdin/stdout, so AI agents can use
    /// vidcut as tools (one per request method, routed like these commands).
    #[command(
        after_help = "Example (Claude Code):\n  claude mcp add vidcut -- vidcut mcp -p talk.vidcut"
    )]
    Mcp,
    /// Open a project in the running GUI (--new creates it).
    Open {
        file: PathBuf,
        #[arg(long)]
        new: bool,
    },
    /// Show the viewer as source (Source Clip), tape (Source Tape) or timeline.
    Viewer { mode: String },
    /// Open a media item in the viewer as Source Clip.
    Load {
        media: String,
        /// Source time to show.
        #[arg(long, allow_hyphen_values = true)]
        at: Option<String>,
    },
    /// Play the viewer (negative speed plays backwards).
    Play {
        #[arg(long, allow_hyphen_values = true)]
        speed: Option<f64>,
        /// Switch the viewer first: source, tape or timeline.
        #[arg(long)]
        viewer: Option<String>,
    },
    /// Stop playback.
    Pause {
        #[arg(long)]
        viewer: Option<String>,
    },
    /// Move the playhead and wait until the frame shows (+10f, -2s: relative).
    Seek {
        #[arg(allow_hyphen_values = true)]
        at: String,
        /// Switch the viewer first: source, tape or timeline.
        #[arg(long)]
        viewer: Option<String>,
        /// Seek to a keyframe (fast, inexact).
        #[arg(long)]
        fast: bool,
    },
    /// Step frames (negative: back).
    Step {
        #[arg(default_value_t = 1, allow_hyphen_values = true)]
        frames: i64,
        #[arg(long)]
        viewer: Option<String>,
    },
    /// Viewer mode, playheads, source clip and selection.
    State,
    /// Show or set the selection (timeline clips and/or pool media).
    Select {
        #[arg(long, value_delimiter = ',')]
        clips: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        media: Option<Vec<String>>,
    },
    /// Save a PNG of the viewer as shown (or of the whole window).
    Screenshot {
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        window: bool,
    },
    /// The GUI's command log (requests from every client).
    Log {
        /// Only entries after this sequence number.
        #[arg(long)]
        since: Option<u64>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Resize, minimize, maximize or fullscreen the GUI window.
    Window {
        /// Inner size WIDTHxHEIGHT in logical pixels.
        #[arg(long)]
        size: Option<String>,
        #[arg(long)]
        minimize: bool,
        #[arg(long)]
        maximize: Option<bool>,
        #[arg(long)]
        fullscreen: Option<bool>,
    },
    /// Close the GUI.
    Quit,
    /// The GUI's Media Pool view: the bin shown and the sort order. Source
    /// Tape follows it.
    Pool {
        /// A bin path ("" for the top level).
        #[arg(long)]
        bin: Option<String>,
        /// added, name, duration or date.
        #[arg(long)]
        sort: Option<String>,
        #[arg(long, conflicts_with = "ascending")]
        descending: bool,
        #[arg(long)]
        ascending: bool,
    },
    /// Mute what the GUI's viewer plays (on, off; default: toggle).
    Mute { state: Option<String> },
    /// Hear a track's fader at a level in the GUI's timeline preview now,
    /// without changing the project, as the Mixer's fader while dragged;
    /// `track set --volume` keeps a level.
    #[command(after_help = "Example:\n  vidcut fader A1 -12")]
    Fader {
        track: String,
        /// The level in dB.
        #[arg(allow_hyphen_values = true)]
        volume_db: f64,
    },
    /// Open a window in the GUI: quick-export, export, jobs, log or mixer.
    Show { panel: String },
}

/// What to analyze: a media item, clips, or (neither) every clip.
#[derive(Args, Debug)]
pub struct AnalyzeTarget {
    /// A media item (positions are its source frames).
    #[arg(long, conflicts_with = "clips")]
    media: Option<String>,
    /// Clips (positions are timeline frames).
    #[arg(long, value_delimiter = ',')]
    clips: Vec<String>,
}

impl AnalyzeTarget {
    fn params(&self) -> Value {
        json!({
            "media": self.media,
            "clips": (!self.clips.is_empty()).then_some(&self.clips),
        })
    }
}

#[derive(Subcommand, Debug)]
pub enum AnalyzeCmd {
    /// Silent stretches (as Ripple Delete Silence finds them).
    Silence {
        #[command(flatten)]
        target: AnalyzeTarget,
        /// Sound below this level (dBFS) is silence [default: -40].
        #[arg(long, allow_hyphen_values = true)]
        threshold: Option<f64>,
        /// The shortest silence reported [default: 0.5s].
        #[arg(long)]
        min: Option<String>,
    },
    /// Shot changes (as Detect Scene Cuts finds them), with their scores.
    Scenes {
        #[command(flatten)]
        target: AnalyzeTarget,
        /// Scene score (0..100) from which a frame starts a new shot [default: 10].
        #[arg(long)]
        threshold: Option<f64>,
    },
    /// Black stretches.
    Black {
        #[command(flatten)]
        target: AnalyzeTarget,
        /// Share of a frame's pixels that must be black [default: 0.98].
        #[arg(long)]
        share: Option<f64>,
        /// The shortest stretch reported [default: 0.5s].
        #[arg(long)]
        min: Option<String>,
    },
    /// Loudness of the timeline's mix (EBU R128): In–Out, or everything.
    Loudness {
        #[arg(long, allow_hyphen_values = true)]
        range: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TrackCmd {
    /// List the tracks with their settings.
    #[command(visible_alias = "ls")]
    List,
    /// Add tracks: video tracks go on top, audio tracks at the bottom.
    Add {
        /// video or audio.
        kind: String,
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
    /// Remove a track (--force also deletes its clips). Later tracks are renumbered.
    #[command(visible_alias = "rm")]
    Remove {
        track: String,
        #[arg(long)]
        force: bool,
    },
    /// Track header buttons and mixer settings.
    #[command(
        after_help = "Examples:\n  vidcut track set A1 --volume -6 --name Music\n  vidcut track set V2 --lock\n  vidcut track set A2 --solo"
    )]
    Set {
        track: String,
        #[arg(long, conflicts_with = "unlock")]
        lock: bool,
        #[arg(long)]
        unlock: bool,
        #[arg(long, conflicts_with = "unmute")]
        mute: bool,
        #[arg(long)]
        unmute: bool,
        #[arg(long, conflicts_with = "unsolo")]
        solo: bool,
        #[arg(long)]
        unsolo: bool,
        /// Fader in dB (-60..24).
        #[arg(long, allow_hyphen_values = true)]
        volume: Option<f64>,
        /// Show the track's video (true/false).
        #[arg(long)]
        video: Option<bool>,
        /// A name ("" clears it).
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum BinsCmd {
    /// Add a bin, like "Interviews/Day 1" (parents too).
    Add { path: String },
    /// Remove a bin and the bins in it; their media move up a level.
    #[command(visible_alias = "rm")]
    Remove { path: String },
    /// Rename or move a bin.
    Rename { path: String, to: String },
}

#[derive(Subcommand, Debug)]
pub enum JobsCmd {
    /// List jobs, oldest first.
    #[command(visible_alias = "ls")]
    List,
    /// One job's state and progress.
    Get { job: String },
    /// Wait until a job ends; fails like the job did.
    Wait {
        job: String,
        /// Give up after this many seconds.
        #[arg(long)]
        timeout: Option<f64>,
    },
    /// Cancel a running job.
    Cancel { job: String },
}

#[derive(Subcommand, Debug)]
pub enum PreferencesCmd {
    /// Change preferences, e.g. thumbnails.cache_limit_mb=256.
    Set {
        #[arg(value_name = "KEY=VALUE", required = true)]
        values: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PresetsCmd {
    /// Save a user preset (~/.config/vidcut/presets/NAME.toml).
    #[command(
        after_help = "Examples:\n  vidcut presets save web --from youtube-1080p --set video.quality=22 --quick-export\n  vidcut presets save master --settings @settings.json --title \"Our master\""
    )]
    Save {
        name: String,
        /// Start from this preset.
        #[arg(long, conflicts_with = "settings")]
        from: Option<String>,
        /// Full settings as JSON (inline, @FILE or - for stdin).
        #[arg(long)]
        settings: Option<String>,
        /// Override a setting, e.g. video.quality=20 (repeatable).
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Show it in Quick Export.
        #[arg(long)]
        quick_export: bool,
        /// Replace an existing preset of that name.
        #[arg(long)]
        overwrite: bool,
    },
    /// Delete a user preset.
    #[command(visible_alias = "rm")]
    Delete { name: String },
}

#[derive(Args, Debug)]
pub struct SetArgs {
    #[arg(long)]
    fps: Option<String>,
    #[arg(long)]
    size: Option<String>,
    #[arg(long)]
    sample_rate: Option<u32>,
    #[arg(long)]
    channels: Option<u16>,
    /// Ripple On for Track 1 (true/false).
    #[arg(long)]
    ripple: Option<bool>,
    /// Record timecode of the first frame, e.g. 01:00:00:00.
    #[arg(long)]
    start_tc: Option<String>,
    /// How long a still lasts when edited without a range, e.g. 5s.
    #[arg(long)]
    still_duration: Option<String>,
    /// Play fresh proxies in the preview (true/false). Export still reads
    /// the originals.
    #[arg(long)]
    proxies: Option<bool>,
}

#[derive(Subcommand, Debug)]
pub enum ProxyCmd {
    /// Make proxies for video and image sequences. With no ids, every such
    /// clip in the pool. Stills and audio-only files are skipped then;
    /// naming one is an error.
    #[command(
        after_help = "Examples:\n  vidcut proxy generate\n  vidcut proxy generate m1 m2 --width 1280\n  vidcut proxy generate --auto\n  vidcut set --proxies true"
    )]
    Generate {
        /// Media ids. Default: every video and image sequence.
        media: Vec<String>,
        /// Longest side in pixels (even). Default: the preferences' proxy
        /// size, 960 unless changed. Smaller pictures stay their size.
        #[arg(long)]
        width: Option<u32>,
        /// Only what the automatic proxy rules pick (`vidcut preferences`:
        /// image sequences, very large or high-bitrate video) and has no
        /// fresh proxy yet.
        #[arg(long)]
        auto: bool,
    },
    /// Delete proxy files and forget them. With no ids, every proxy.
    Remove {
        /// Media ids. Default: every item that has a proxy.
        media: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum MediaCmd {
    /// Add files to the pool. An image is a still; with --sequence, a
    /// numbered image brings the files numbered in a row around it as one
    /// clip.
    Add {
        files: Vec<PathBuf>,
        /// Into this bin.
        #[arg(long)]
        bin: Option<String>,
        /// Each file is one of a numbered image sequence (frame_000001.png).
        #[arg(long)]
        sequence: bool,
        /// The sequence's frame rate, e.g. 24 or 30000/1001 (default: the
        /// project's).
        #[arg(long, requires = "sequence")]
        fps: Option<String>,
    },
    /// Put media into a bin ("" for the top level).
    Move {
        #[arg(required = true)]
        media: Vec<String>,
        #[arg(long)]
        bin: String,
    },
    /// List the pool.
    #[command(visible_alias = "ls")]
    List,
    /// Details of one media item.
    Show {
        media: String,
    },
    /// Remove from the pool (--force also deletes its clips).
    #[command(visible_alias = "rm")]
    Remove {
        media: String,
        #[arg(long)]
        force: bool,
    },
    /// Point a media item at another file.
    Relink {
        media: String,
        file: PathBuf,
    },
    Rename {
        media: String,
        name: String,
    },
    /// Set or clear a media item's source In/Out (Out is exclusive).
    Marks {
        media: String,
        #[arg(long = "in", allow_hyphen_values = true)]
        mark_in: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        out: Option<String>,
        #[arg(long, conflicts_with_all = ["mark_in", "out"])]
        clear: bool,
    },
}

#[derive(Args, Debug)]
pub struct SourceArgs {
    /// Media id (m1).
    media: String,
    /// Source range, e.g. 10s..+20s (default: all of it).
    #[arg(long, allow_hyphen_values = true)]
    range: Option<String>,
    /// Leave the audio out (Video Only).
    #[arg(long, conflicts_with = "audio_only")]
    video_only: bool,
    /// Leave the video out (Audio Only).
    #[arg(long)]
    audio_only: bool,
    /// Target track (default: V1, or A1 for audio-only clips). The next new
    /// track, like V3 above V2, is made.
    #[arg(long)]
    track: Option<String>,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct EdgeArgs {
    /// The clip's start.
    #[arg(long)]
    head: bool,
    /// The clip's end.
    #[arg(long)]
    tail: bool,
}

impl EdgeArgs {
    fn name(&self) -> &'static str {
        if self.head { "head" } else { "tail" }
    }
}

#[derive(Subcommand, Debug)]
pub enum EditCmd {
    /// Append after the last clip (Shift+F12).
    Append(SourceArgs),
    /// Insert at the edit nearest the playhead (Smart Insert, F9).
    SmartInsert {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, allow_hyphen_values = true)]
        playhead: String,
    },
    /// Insert at an exact frame (splits a clip there).
    Insert {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, allow_hyphen_values = true)]
        at: String,
    },
    /// Overwrite (F10) at the timeline In, before a lone Out, or at --at.
    Overwrite {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, allow_hyphen_values = true)]
        at: Option<String>,
    },
    /// Replace the In–Out range or the clip at --at (Shift+F10).
    RippleOverwrite {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, allow_hyphen_values = true)]
        at: Option<String>,
    },
    /// Place On Top (F12): on the first track above everything there.
    PlaceOnTop {
        #[command(flatten)]
        source: SourceArgs,
        #[arg(long, allow_hyphen_values = true)]
        at: Option<String>,
    },
    /// Split at one or more frames (comma-separated).
    Split {
        #[arg(
            long,
            value_delimiter = ',',
            required = true,
            allow_hyphen_values = true
        )]
        at: Vec<String>,
        /// Only split these clips.
        #[arg(long, value_delimiter = ',')]
        clips: Vec<String>,
    },
    /// Delete clips (ripples Track 1 unless --lift).
    Delete {
        #[arg(required = true)]
        clips: Vec<String>,
        #[arg(long)]
        lift: bool,
    },
    /// Delete a range, e.g. 2m..2m04s (ripples unless --lift).
    DeleteRange {
        range: String,
        #[arg(long)]
        lift: bool,
    },
    /// Close the gap containing a frame.
    CloseGap {
        #[arg(long, allow_hyphen_values = true)]
        at: String,
    },
    /// Move clips to an edit point (or over whatever is there with --overwrite).
    Move {
        #[arg(required = true)]
        clips: Vec<String>,
        #[arg(long, allow_hyphen_values = true)]
        to: String,
        #[arg(long)]
        overwrite: bool,
        /// Move them to this track.
        #[arg(long)]
        track: Option<String>,
    },
    /// Replace a clip with another clip from the timeline.
    Replace {
        clip: String,
        #[arg(long)]
        onto: String,
    },
    /// Resize a clip edge.
    Trim {
        clip: String,
        #[command(flatten)]
        edge: EdgeArgs,
        /// Move the edge by this much (-12f, +1s).
        #[arg(long, allow_hyphen_values = true, conflicts_with = "to")]
        by: Option<String>,
        /// Move the edge to this frame.
        #[arg(long, allow_hyphen_values = true)]
        to: Option<String>,
        /// Don't ripple (Alt): trim in place.
        #[arg(long)]
        no_ripple: bool,
    },
    /// Roll the edit at a clip edge.
    Roll {
        clip: String,
        #[command(flatten)]
        edge: EdgeArgs,
        #[arg(long, allow_hyphen_values = true)]
        by: String,
    },
    /// Slip a clip's content.
    Slip {
        clip: String,
        #[arg(long, allow_hyphen_values = true)]
        by: String,
    },
    /// Slide a clip: it keeps its media and moves; its neighbors give and take.
    Slide {
        clip: String,
        #[arg(long, allow_hyphen_values = true)]
        by: String,
    },
    /// Cut the silent parts out of clips and close the gaps (one undo step).
    /// With --dry-run, list the parts instead.
    #[command(
        after_help = "Examples:\n  vidcut edit ripple-delete-silence c1 --dry-run          # silences of 0.5 s and more\n  vidcut edit ripple-delete-silence c1 c2 --threshold -35 --pre-head 0.2s --post-tail 0.2s"
    )]
    RippleDeleteSilence {
        #[arg(required = true)]
        clips: Vec<String>,
        /// Sound below this level (dBFS) is silence [default: -40].
        #[arg(long, allow_hyphen_values = true)]
        threshold: Option<f64>,
        /// Silence kept before each audible part [default: 0.15s].
        #[arg(long)]
        pre_head: Option<String>,
        /// Silence kept after each audible part [default: 0.15s].
        #[arg(long)]
        post_tail: Option<String>,
        /// The shortest silence removed [default: 0.5s].
        #[arg(long)]
        min: Option<String>,
    },
    /// Split clips where their shots change.
    DetectSceneCuts {
        #[arg(required = true)]
        clips: Vec<String>,
        /// Scene score (0..100) from which a frame starts a new shot [default: 10].
        #[arg(long)]
        threshold: Option<f64>,
    },
    /// Move only the audio edit at a clip edge (J/L cut).
    AudioRoll {
        clip: String,
        #[command(flatten)]
        edge: EdgeArgs,
        #[arg(long, allow_hyphen_values = true)]
        by: String,
    },
    /// Move clips' head or tail to a frame.
    TrimToPlayhead {
        #[command(flatten)]
        edge: EdgeArgs,
        #[arg(long, allow_hyphen_values = true)]
        at: String,
        #[arg(long, value_delimiter = ',')]
        clips: Vec<String>,
        #[arg(long)]
        no_ripple: bool,
    },
    /// Copy clips (prints them for `edit paste --clips`).
    Copy {
        #[arg(required = true)]
        clips: Vec<String>,
    },
    /// Cut clips: copy, then delete with ripple.
    Cut {
        #[arg(required = true)]
        clips: Vec<String>,
    },
    /// Paste clips over Track 1 (default: the GUI's clipboard at its playhead).
    Paste {
        #[arg(long, allow_hyphen_values = true)]
        at: Option<String>,
        /// Clips as JSON (the `clips` of `edit copy --json`), or @FILE, or - for stdin.
        #[arg(long)]
        clips: Option<String>,
    },
    /// Set clips' duration.
    Duration {
        #[arg(required = true)]
        clips: Vec<String>,
        #[arg(long)]
        set: String,
        #[arg(long)]
        no_ripple: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ClipCmd {
    /// Set clip properties.
    Set {
        #[arg(required = true)]
        clips: Vec<String>,
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
        #[arg(long, conflicts_with = "unmute")]
        mute: bool,
        #[arg(long)]
        unmute: bool,
        /// Clip volume in dB (-60..24).
        #[arg(long, allow_hyphen_values = true)]
        gain: Option<f64>,
        /// Clip name ("" clears it).
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum MarksCmd {
    /// Set In and/or Out (Out is exclusive).
    Set {
        #[arg(long = "in", allow_hyphen_values = true)]
        mark_in: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        out: Option<String>,
    },
    /// Clear both marks.
    Clear,
    Show,
}

#[derive(Subcommand, Debug)]
pub enum MarkersCmd {
    Add {
        #[arg(long, allow_hyphen_values = true)]
        at: String,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long)]
        color: Option<String>,
        #[arg(long, default_value = "")]
        note: String,
    },
    #[command(visible_alias = "mv")]
    Move {
        marker: String,
        #[arg(long, allow_hyphen_values = true)]
        to: String,
    },
    #[command(visible_alias = "rm")]
    Remove { marker: String },
    #[command(visible_alias = "ls")]
    List,
}

/// What `main` should do for a parsed command line.
pub enum Action {
    /// Serve MCP on stdin/stdout.
    Mcp,
    /// Create a project file.
    New {
        file: PathBuf,
        settings: vidcut_core::model::Settings,
    },
    /// Run one request.
    Run(Envelope),
    /// Print the timeline's marks (a projection of `project.summary`).
    ShowMarks(Envelope),
    /// List markers (from `timeline.get`).
    ListMarkers(Envelope),
    /// List tracks (from `timeline.get`).
    ListTracks(Envelope),
    /// List bins (from `project.summary`).
    ListBins(Envelope),
    /// Start the GUI.
    Gui { project: Option<PathBuf> },
    /// List running GUIs.
    Instances,
}

fn time(s: &str) -> Value {
    json!(s)
}

fn source(s: &SourceArgs) -> Value {
    let mut v = json!({ "media": s.media, "video_only": s.video_only, "audio_only": s.audio_only });
    if let Some(r) = &s.range {
        v["range"] = json!(r);
    }
    if let Some(t) = &s.track {
        v["track"] = json!(t);
    }
    v
}

fn with(mut base: Value, extra: Value) -> Value {
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}

fn ripple(no_ripple: bool) -> Value {
    if no_ripple { json!(false) } else { Value::Null }
}

fn req(method: &str, params: Value) -> Result<Request> {
    // Drop nulls so optional params fall back to their defaults.
    let params = match params {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).collect()),
        other => other,
    };
    Request::from_parts(method, Some(params)).map_err(Error::usage)
}

fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once('x')
        .ok_or_else(|| Error::usage(format!("size {s:?}: expected WIDTHxHEIGHT")))?;
    match (w.parse(), h.parse()) {
        (Ok(w), Ok(h)) => Ok((w, h)),
        _ => Err(Error::usage(format!("size {s:?}: expected WIDTHxHEIGHT"))),
    }
}

/// `params` without its `null` fields, so the request's defaults apply.
fn without_nulls(mut params: Value) -> Value {
    if let Some(map) = params.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }
    params
}

/// Paths made absolute: a running GUI may answer from another directory.
fn absolute(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    paths
        .iter()
        .map(|p| std::path::absolute(p).map_err(|e| Error::usage(format!("{}: {e}", p.display()))))
        .collect()
}

/// A JSON argument given inline, as `@FILE`, or as `-` (stdin).
fn read_json_arg(arg: &str) -> Result<Value> {
    let text = if arg == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| Error::usage(format!("stdin: {e}")))?;
        s
    } else if let Some(file) = arg.strip_prefix('@') {
        std::fs::read_to_string(file).map_err(|e| Error::usage(format!("{file}: {e}")))?
    } else {
        arg.to_string()
    };
    serde_json::from_str(&text).map_err(|e| Error::usage(format!("not JSON: {e}")))
}

/// Reads requests for `vidcut apply`: JSON Lines or one JSON array.
fn read_batch(file: &str) -> Result<Vec<Request>> {
    let text = if file == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| Error::usage(format!("stdin: {e}")))?;
        s
    } else {
        std::fs::read_to_string(file).map_err(|e| Error::usage(format!("{file}: {e}")))?
    };
    let values: Vec<(usize, Value)> = if text.trim_start().starts_with('[') {
        let v: Vec<Value> =
            serde_json::from_str(&text).map_err(|e| Error::usage(format!("{file}: {e}")))?;
        v.into_iter().enumerate().collect()
    } else {
        text.lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
            .map(|(i, l)| {
                serde_json::from_str(l)
                    .map(|v| (i, v))
                    .map_err(|e| Error::usage(format!("{file}:{}: {e}", i + 1)))
            })
            .collect::<Result<_>>()?
    };
    values
        .into_iter()
        .map(|(i, v)| {
            let e = Envelope::from_json(v)
                .map_err(|e| Error::usage(format!("{file} entry {}: {e}", i + 1)))?;
            if e.if_rev.is_some() || e.dry_run {
                return Err(Error::usage(format!(
                    "{file} entry {}: put --if-rev/--dry-run on the command line",
                    i + 1
                )));
            }
            Ok(e.request)
        })
        .collect()
}

impl Cli {
    /// Turns the command line into an action.
    pub fn action(&self) -> Result<Option<Action>> {
        let Some(command) = &self.command else {
            return Ok(None);
        };
        let request = match command {
            Command::New {
                file,
                fps,
                size,
                sample_rate,
                channels,
            } => {
                let mut settings = vidcut_core::model::Settings::default();
                if let Some(f) = fps {
                    settings.fps = f
                        .parse::<Rational>()
                        .map_err(|e| Error::usage(e.to_string()))?;
                    settings.auto_format = false;
                }
                if let Some(s) = size {
                    (settings.width, settings.height) = parse_size(s)?;
                    settings.auto_format = false;
                }
                if let Some(sr) = sample_rate {
                    settings.sample_rate = *sr;
                }
                if let Some(c) = channels {
                    settings.channels = *c;
                }
                return Ok(Some(Action::New {
                    file: file.clone(),
                    settings,
                }));
            }
            Command::Info => req("project.summary", json!({}))?,
            Command::Set(s) => {
                let (width, height) = match &s.size {
                    Some(sz) => parse_size(sz).map(|(w, h)| (Some(w), Some(h)))?,
                    None => (None, None),
                };
                req(
                    "project.set",
                    json!({ "fps": s.fps, "width": width, "height": height, "sample_rate": s.sample_rate,
                            "channels": s.channels, "ripple_on": s.ripple, "start_tc": s.start_tc,
                            "still_duration": s.still_duration, "use_proxies": s.proxies }),
                )?
            }
            Command::Probe { file } => req(
                "probe",
                json!({ "path": absolute(std::slice::from_ref(file))?[0] }),
            )?,
            Command::Media(m) => match m {
                MediaCmd::Add {
                    files,
                    bin,
                    sequence,
                    fps,
                } => req(
                    "media.add",
                    json!({ "paths": absolute(files)?, "bin": bin, "sequence": sequence, "fps": fps }),
                )?,
                MediaCmd::Move { media, bin } => {
                    req("media.move", json!({ "media": media, "bin": bin }))?
                }
                MediaCmd::List => req("media.list", json!({}))?,
                MediaCmd::Show { media } => req("media.get", json!({ "media": media }))?,
                MediaCmd::Remove { media, force } => {
                    req("media.remove", json!({ "media": media, "force": force }))?
                }
                MediaCmd::Relink { media, file } => req(
                    "media.relink",
                    json!({ "media": media, "path": absolute(std::slice::from_ref(file))?[0] }),
                )?,
                MediaCmd::Rename { media, name } => {
                    req("media.rename", json!({ "media": media, "name": name }))?
                }
                MediaCmd::Marks {
                    media,
                    mark_in,
                    out,
                    clear,
                } => {
                    if *clear {
                        req("media.marks.clear", json!({ "media": media }))?
                    } else {
                        req(
                            "media.marks.set",
                            json!({ "media": media, "in": mark_in, "out": out }),
                        )?
                    }
                }
            },
            Command::Proxy(p) => match p {
                ProxyCmd::Generate { media, width, auto } => {
                    let media = if media.is_empty() {
                        Value::Null
                    } else {
                        json!(media)
                    };
                    req(
                        "proxy.generate",
                        json!({ "media": media, "width": width, "auto": auto }),
                    )?
                }
                ProxyCmd::Remove { media } => {
                    let media = if media.is_empty() {
                        Value::Null
                    } else {
                        json!(media)
                    };
                    req("proxy.remove", json!({ "media": media }))?
                }
            },
            Command::Timeline { range } => req("timeline.get", json!({ "range": range }))?,
            Command::Track { cmd } => match cmd.as_ref().unwrap_or(&TrackCmd::List) {
                TrackCmd::List => {
                    return Ok(Some(Action::ListTracks(
                        self.envelope(req("timeline.get", json!({}))?),
                    )));
                }
                TrackCmd::Add { kind, count } => {
                    req("track.add", json!({ "kind": kind, "count": count }))?
                }
                TrackCmd::Remove { track, force } => {
                    req("track.remove", json!({ "track": track, "force": force }))?
                }
                TrackCmd::Set {
                    track,
                    lock,
                    unlock,
                    mute,
                    unmute,
                    solo,
                    unsolo,
                    volume,
                    video,
                    name,
                } => {
                    let flag = |on: bool, off: bool| match (on, off) {
                        (true, _) => json!(true),
                        (_, true) => json!(false),
                        _ => Value::Null,
                    };
                    req(
                        "track.set",
                        json!({ "track": track, "locked": flag(*lock, *unlock),
                                "muted": flag(*mute, *unmute), "solo": flag(*solo, *unsolo),
                                "volume_db": volume, "video_enabled": video, "name": name }),
                    )?
                }
            },
            Command::Bins { cmd } => match cmd {
                None => {
                    return Ok(Some(Action::ListBins(
                        self.envelope(req("project.summary", json!({}))?),
                    )));
                }
                Some(BinsCmd::Add { path }) => req("bins.add", json!({ "path": path }))?,
                Some(BinsCmd::Remove { path }) => req("bins.remove", json!({ "path": path }))?,
                Some(BinsCmd::Rename { path, to }) => {
                    req("bins.rename", json!({ "path": path, "to": to }))?
                }
            },
            Command::At { at } => req("timeline.at", json!({ "at": time(at) }))?,
            Command::Cuts => req("timeline.edit_points", json!({}))?,
            Command::Edit(e) => match e {
                EditCmd::Append(s) => req("edit.append", source(s))?,
                EditCmd::SmartInsert {
                    source: s,
                    playhead,
                } => req(
                    "edit.smart_insert",
                    with(source(s), json!({ "playhead": time(playhead) })),
                )?,
                EditCmd::Insert { source: s, at } => {
                    req("edit.insert", with(source(s), json!({ "at": time(at) })))?
                }
                EditCmd::Overwrite { source: s, at } => {
                    req("edit.overwrite", with(source(s), json!({ "at": at })))?
                }
                EditCmd::RippleOverwrite { source: s, at } => req(
                    "edit.ripple_overwrite",
                    with(source(s), json!({ "at": at })),
                )?,
                EditCmd::PlaceOnTop { source: s, at } => {
                    req("edit.place_on_top", with(source(s), json!({ "at": at })))?
                }
                EditCmd::Split { at, clips } => {
                    req("edit.split", json!({ "at": at, "clips": clips }))?
                }
                EditCmd::Delete { clips, lift } => {
                    req("edit.delete", json!({ "clips": clips, "lift": lift }))?
                }
                EditCmd::DeleteRange { range, lift } => {
                    req("edit.delete_range", json!({ "range": range, "lift": lift }))?
                }
                EditCmd::CloseGap { at } => req("edit.close_gap", json!({ "at": time(at) }))?,
                EditCmd::Move {
                    clips,
                    to,
                    overwrite,
                    track,
                } => req(
                    "edit.move",
                    json!({ "clips": clips, "to": time(to), "track": track,
                            "mode": if *overwrite { "overwrite" } else { "insert" } }),
                )?,
                EditCmd::Replace { clip, onto } => {
                    req("edit.replace", json!({ "clip": clip, "onto": onto }))?
                }
                EditCmd::Trim {
                    clip,
                    edge,
                    by,
                    to,
                    no_ripple,
                } => req(
                    "edit.trim",
                    json!({ "clip": clip, "edge": edge.name(), "by": by, "to": to, "ripple": ripple(*no_ripple) }),
                )?,
                EditCmd::Roll { clip, edge, by } => req(
                    "edit.roll",
                    json!({ "clip": clip, "edge": edge.name(), "by": by }),
                )?,
                EditCmd::Slip { clip, by } => req("edit.slip", json!({ "clip": clip, "by": by }))?,
                EditCmd::Slide { clip, by } => {
                    req("edit.slide", json!({ "clip": clip, "by": by }))?
                }
                EditCmd::RippleDeleteSilence {
                    clips,
                    threshold,
                    pre_head,
                    post_tail,
                    min,
                } => req(
                    "edit.ripple_delete_silence",
                    without_nulls(json!({
                        "clips": clips, "threshold_db": threshold,
                        "pre_head": pre_head, "post_tail": post_tail, "min": min,
                    })),
                )?,
                EditCmd::DetectSceneCuts { clips, threshold } => req(
                    "edit.detect_scene_cuts",
                    without_nulls(json!({ "clips": clips, "threshold": threshold })),
                )?,
                EditCmd::AudioRoll { clip, edge, by } => req(
                    "edit.audio_roll",
                    json!({ "clip": clip, "edge": edge.name(), "by": by }),
                )?,
                EditCmd::TrimToPlayhead {
                    edge,
                    at,
                    clips,
                    no_ripple,
                } => req(
                    "edit.trim_to_playhead",
                    json!({ "edge": edge.name(), "at": time(at), "clips": clips, "ripple": ripple(*no_ripple) }),
                )?,
                EditCmd::Copy { clips } => req("edit.copy", json!({ "clips": clips }))?,
                EditCmd::Cut { clips } => req("edit.cut", json!({ "clips": clips }))?,
                EditCmd::Paste { at, clips } => {
                    let clips = match clips {
                        Some(text) => Some(read_json_arg(text)?),
                        None => None,
                    };
                    req("edit.paste", json!({ "at": at, "clips": clips }))?
                }
                EditCmd::Duration {
                    clips,
                    set,
                    no_ripple,
                } => req(
                    "edit.change_duration",
                    json!({ "clips": clips, "duration": set, "ripple": ripple(*no_ripple) }),
                )?,
            },
            Command::Clip(ClipCmd::Set {
                clips,
                enable,
                disable,
                mute,
                unmute,
                gain,
                name,
            }) => req(
                "clip.set",
                json!({
                    "clips": clips,
                    "enabled": if *enable { json!(true) } else if *disable { json!(false) } else { Value::Null },
                    "muted": if *mute { json!(true) } else if *unmute { json!(false) } else { Value::Null },
                    "gain_db": gain,
                    "name": name,
                }),
            )?,
            Command::Marks(m) => match m {
                MarksCmd::Set { mark_in, out } => {
                    req("marks.set", json!({ "in": mark_in, "out": out }))?
                }
                MarksCmd::Clear => req("marks.clear", json!({}))?,
                MarksCmd::Show => {
                    return Ok(Some(Action::ShowMarks(
                        self.envelope(req("project.summary", json!({}))?),
                    )));
                }
            },
            Command::Markers(m) => match m {
                MarkersCmd::Add {
                    at,
                    name,
                    color,
                    note,
                } => req(
                    "markers.add",
                    json!({ "at": time(at), "name": name, "color": color, "note": note }),
                )?,
                MarkersCmd::Move { marker, to } => {
                    req("markers.move", json!({ "marker": marker, "to": time(to) }))?
                }
                MarkersCmd::Remove { marker } => {
                    req("markers.remove", json!({ "marker": marker }))?
                }
                MarkersCmd::List => {
                    return Ok(Some(Action::ListMarkers(
                        self.envelope(req("timeline.get", json!({}))?),
                    )));
                }
            },
            Command::Export {
                output,
                preset,
                set,
                range,
                per_clip,
                audio_only,
                encoder,
                name,
                overwrite,
                background,
            } => req(
                if self.dry_run {
                    "export.plan"
                } else {
                    "export.start"
                },
                json!({ "output": absolute(std::slice::from_ref(output))?[0], "preset": preset, "set": set, "range": range, "per_clip": per_clip,
                        "audio_only": audio_only, "encoder": encoder, "name": name, "overwrite": overwrite,
                        "background": *background && !self.dry_run }),
            )?,
            Command::Import {
                file,
                replace,
                search,
                unpack,
            } => req(
                "timeline.import",
                json!({ "path": absolute(std::slice::from_ref(file))?[0], "replace": replace,
                        "search": absolute(search)?,
                        "unpack": unpack.as_ref().map(|u| absolute(std::slice::from_ref(u))).transpose()?.map(|u| u[0].clone()) }),
            )?,
            Command::Jobs { cmd } => match cmd {
                None | Some(JobsCmd::List) => req("jobs.list", json!({}))?,
                Some(JobsCmd::Get { job }) => req("jobs.get", json!({ "job": job }))?,
                Some(JobsCmd::Wait { job, timeout }) => {
                    req("jobs.wait", json!({ "job": job, "timeout": timeout }))?
                }
                Some(JobsCmd::Cancel { job }) => req("jobs.cancel", json!({ "job": job }))?,
            },
            Command::Analyze(cmd) => match cmd {
                AnalyzeCmd::Silence {
                    target,
                    threshold,
                    min,
                } => {
                    let mut params = target.params();
                    params["threshold_db"] = json!(threshold);
                    params["min"] = json!(min);
                    req("analyze.silence", without_nulls(params))?
                }
                AnalyzeCmd::Scenes { target, threshold } => {
                    let mut params = target.params();
                    params["threshold"] = json!(threshold);
                    req("analyze.scenes", without_nulls(params))?
                }
                AnalyzeCmd::Black { target, share, min } => {
                    let mut params = target.params();
                    params["share"] = json!(share);
                    params["min"] = json!(min);
                    req("analyze.black", without_nulls(params))?
                }
                AnalyzeCmd::Loudness { range } => {
                    req("analyze.loudness", without_nulls(json!({ "range": range })))?
                }
            },
            Command::Boring { boring, jump } => req(
                "timeline.boring",
                without_nulls(json!({ "boring": boring, "jump": jump })),
            )?,
            Command::Storyboard {
                out,
                columns,
                width,
                every,
                range,
            } => {
                let out = absolute(std::slice::from_ref(out))?.remove(0);
                req(
                    "storyboard.render",
                    without_nulls(json!({
                        "out": out, "columns": columns, "width": width,
                        "every": every, "range": range,
                    })),
                )?
            }
            Command::Frame {
                at,
                out,
                media,
                width,
            } => {
                let out = absolute(std::slice::from_ref(out))?.remove(0);
                req(
                    "frame.render",
                    json!({ "at": time(at), "out": out, "media": media, "width": width }),
                )?
            }
            Command::Presets { cmd, name } => match (cmd, name) {
                (
                    Some(PresetsCmd::Save {
                        name,
                        from,
                        settings,
                        set,
                        title,
                        description,
                        quick_export,
                        overwrite,
                    }),
                    _,
                ) => {
                    let settings = settings.as_deref().map(read_json_arg).transpose()?;
                    req(
                        "presets.save",
                        json!({ "name": name, "from": from, "settings": settings, "set": set,
                                "title": title, "description": description,
                                "quick_export": quick_export, "overwrite": overwrite }),
                    )?
                }
                (Some(PresetsCmd::Delete { name }), _) => {
                    req("presets.delete", json!({ "name": name }))?
                }
                (None, Some(n)) => req("presets.get", json!({ "name": n }))?,
                (None, None) => req("presets.list", json!({}))?,
            },
            Command::Preferences { cmd } => match cmd {
                None => req("preferences.get", json!({}))?,
                Some(PreferencesCmd::Set { values }) => {
                    let patch = vidcut_core::preferences::PreferencesPatch::from_pairs(values)?;
                    req("preferences.set", json!(patch))?
                }
            },
            Command::Encoders { refresh } => req("encoders.list", json!({ "refresh": refresh }))?,
            Command::Undo { steps } => req("history.undo", json!({ "steps": steps }))?,
            Command::Redo { steps } => req("history.redo", json!({ "steps": steps }))?,
            Command::History => req("history.get", json!({}))?,
            Command::Apply { file, label } => {
                let mut batch = Request::Apply {
                    commands: read_batch(file)?,
                    label: Some(label.clone().unwrap_or_else(|| format!("apply {file}"))),
                };
                for p in batch.paths_mut() {
                    *p = absolute(std::slice::from_ref(p))?.remove(0);
                }
                batch
            }
            Command::Schema { method } => req("schema", json!({ "method": method }))?,
            Command::Rpc { method, params } => {
                let params: Option<Value> = match params {
                    Some(p) => Some(
                        serde_json::from_str(p)
                            .map_err(|e| Error::usage(format!("params: {e}")))?,
                    ),
                    None => None,
                };
                let mut v = json!({ "method": method });
                if let Some(p) = params {
                    v["params"] = p;
                }
                let mut env = Envelope::from_json(v).map_err(Error::usage)?;
                env.if_rev = env.if_rev.or(self.if_rev);
                env.dry_run |= self.dry_run;
                return Ok(Some(Action::Run(env)));
            }
            Command::Doctor => req("doctor", json!({}))?,
            Command::Gui { file } => {
                return Ok(Some(Action::Gui {
                    project: file.clone().or_else(|| self.project.clone()),
                }));
            }
            Command::Instances => return Ok(Some(Action::Instances)),
            Command::Mcp => return Ok(Some(Action::Mcp)),
            Command::Open { file, new } => {
                let path = absolute(std::slice::from_ref(file))?.remove(0);
                req("project.open", json!({ "path": path, "create": new }))?
            }
            Command::Viewer { mode } => req("viewer.set", json!({ "mode": mode }))?,
            Command::Load { media, at } => req("viewer.load", json!({ "media": media, "at": at }))?,
            Command::Play { speed, viewer } => {
                req("player.play", json!({ "speed": speed, "viewer": viewer }))?
            }
            Command::Pause { viewer } => req("player.pause", json!({ "viewer": viewer }))?,
            Command::Seek { at, viewer, fast } => req(
                "player.seek",
                json!({ "at": time(at), "viewer": viewer, "fast": fast }),
            )?,
            Command::Step { frames, viewer } => {
                req("player.step", json!({ "frames": frames, "viewer": viewer }))?
            }
            Command::State => req("player.state", json!({}))?,
            Command::Select { clips, media } => {
                if clips.is_none() && media.is_none() {
                    req("selection.get", json!({}))?
                } else {
                    req("selection.set", json!({ "clips": clips, "media": media }))?
                }
            }
            Command::Screenshot { out, window } => {
                let path = absolute(std::slice::from_ref(out))?.remove(0);
                req(
                    "gui.screenshot",
                    json!({ "out": path, "target": if *window { "window" } else { "viewer" } }),
                )?
            }
            Command::Log { since, limit } => {
                req("gui.log", json!({ "since": since, "limit": limit }))?
            }
            Command::Window {
                size,
                minimize,
                maximize,
                fullscreen,
            } => {
                let size = match size {
                    Some(s) => {
                        let (w, h) = parse_size(s)?;
                        Some([w as f32, h as f32])
                    }
                    None => None,
                };
                req(
                    "gui.window",
                    json!({ "size": size, "minimized": minimize.then_some(true),
                            "maximized": maximize, "fullscreen": fullscreen }),
                )?
            }
            Command::Quit => req("gui.quit", json!({}))?,
            Command::Pool {
                bin,
                sort,
                descending,
                ascending,
            } => {
                let order = match (*descending, *ascending) {
                    (true, _) => json!(true),
                    (_, true) => json!(false),
                    _ => Value::Null,
                };
                req(
                    "pool.set",
                    json!({ "bin": bin, "sort": sort, "descending": order }),
                )?
            }
            Command::Fader { track, volume_db } => req(
                "mixer.preview",
                json!({ "track": track, "volume_db": volume_db }),
            )?,
            Command::Mute { state } => {
                let mute = match state.as_deref() {
                    None => Value::Null,
                    Some("on") => json!(true),
                    Some("off") => json!(false),
                    Some(other) => {
                        return Err(Error::usage(format!("mute {other:?}: use on or off")));
                    }
                };
                req("player.mute", json!({ "mute": mute }))?
            }
            Command::Show { panel } => req("gui.show", json!({ "panel": panel }))?,
        };
        Ok(Some(Action::Run(self.envelope(request))))
    }

    fn envelope(&self, request: Request) -> Envelope {
        Envelope {
            request,
            if_rev: self.if_rev,
            dry_run: self.dry_run,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;
    use vidcut_core::Request;

    use super::{Action, Cli};

    fn probe(file: &str) -> super::Result<PathBuf> {
        let cli = Cli::try_parse_from(["vidcut", "probe", file]).unwrap();
        let Some(Action::Run(envelope)) = cli.action()? else {
            panic!("probe runs a request");
        };
        match envelope.request {
            Request::Probe { path } => Ok(path),
            other => panic!("probe built {other:?}"),
        }
    }

    /// `vidcut probe` names its file absolutely, the same as every other path
    /// the CLI sends. `apply` and MCP already do this through `paths_mut`.
    #[test]
    fn probe_makes_its_path_absolute() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(probe("clip.mp4").unwrap(), cwd.join("clip.mp4"));
        assert_eq!(
            probe("/srv/clip.mp4").unwrap(),
            PathBuf::from("/srv/clip.mp4")
        );
    }
}
