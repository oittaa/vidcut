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
    about = "A simple, fully scriptable video cutting tool modeled on DaVinci Resolve's Cut page.",
    long_about = "A simple, fully scriptable video cutting tool modeled on DaVinci Resolve's Cut page.\n\n\
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
    Set(SetArgs),
    /// Show what the media layer knows about a file.
    Probe { file: PathBuf },
    /// Media pool.
    #[command(subcommand)]
    Media(MediaCmd),
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
    /// Timeline markers.
    #[command(subcommand)]
    Markers(MarkersCmd),
    /// Export the timeline (In–Out if both marks are set, else all of it).
    /// With --dry-run, print the plan instead.
    #[command(
        after_help = "Examples:\n  vidcut export out.mp4\n  vidcut export out.mp4 --preset youtube-1080p --encoder nvenc\n  vidcut export cut.mov --range 1m..2m --set video.quality=14\n  vidcut export clips/ --per-clip --preset prores-422hq\n  vidcut export voice.wav --audio-only"
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
    /// List export presets, or show one.
    Presets { name: Option<String> },
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
    /// Check dependencies (FFmpeg, libmpv, GPUs).
    Doctor,

    // ------------------------------------------------------------ the GUI
    /// Start the GUI (also what `vidcut` without a command does).
    Gui {
        /// Project to open (created if missing). Default: -p / VIDCUT_PROJECT.
        file: Option<PathBuf>,
    },
    /// List running GUIs (live commands go to them).
    Instances,
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
}

#[derive(Subcommand, Debug)]
pub enum MediaCmd {
    /// Add files to the pool.
    Add {
        files: Vec<PathBuf>,
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
    /// Leave the audio out.
    #[arg(long)]
    video_only: bool,
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
    /// Start the GUI.
    Gui { project: Option<PathBuf> },
    /// List running GUIs.
    Instances,
}

fn time(s: &str) -> Value {
    json!(s)
}

fn source(s: &SourceArgs) -> Value {
    let mut v = json!({ "media": s.media, "video_only": s.video_only });
    if let Some(r) = &s.range {
        v["range"] = json!(r);
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
                            "channels": s.channels, "ripple_on": s.ripple, "start_tc": s.start_tc }),
                )?
            }
            Command::Probe { file } => req("probe", json!({ "path": file }))?,
            Command::Media(m) => match m {
                MediaCmd::Add { files } => req("media.add", json!({ "paths": absolute(files)? }))?,
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
            Command::Timeline { range } => req("timeline.get", json!({ "range": range }))?,
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
                } => req(
                    "edit.move",
                    json!({ "clips": clips, "to": time(to), "mode": if *overwrite { "overwrite" } else { "insert" } }),
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
            } => req(
                if self.dry_run {
                    "export.plan"
                } else {
                    "export.start"
                },
                json!({ "output": absolute(std::slice::from_ref(output))?[0], "preset": preset, "set": set, "range": range, "per_clip": per_clip,
                        "audio_only": audio_only, "encoder": encoder, "name": name, "overwrite": overwrite }),
            )?,
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
            Command::Presets { name } => match name {
                Some(n) => req("presets.get", json!({ "name": n }))?,
                None => req("presets.list", json!({}))?,
            },
            Command::Encoders { refresh } => req("encoders.list", json!({ "refresh": refresh }))?,
            Command::Undo { steps } => req("history.undo", json!({ "steps": steps }))?,
            Command::Redo { steps } => req("history.redo", json!({ "steps": steps }))?,
            Command::History => req("history.get", json!({}))?,
            Command::Apply { file, label } => Request::Apply {
                commands: read_batch(file)?,
                label: Some(label.clone().unwrap_or_else(|| format!("apply {file}"))),
            },
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
