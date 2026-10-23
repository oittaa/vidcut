use std::io::Read;
use std::path::PathBuf;
use std::sync::OnceLock;

use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, Value, json};
use vidcut_core::request::{Client, ClientKind, method_names, protocol_schema, resolve_ref};
use vidcut_core::{Envelope, Error, Request, Result};

pub const POSITIONAL: &[(&str, &[&str])] = &[
    ("probe", &["path"]),
    ("schema", &["method"]),
    ("project.create", &["path"]),
    ("project.open", &["path"]),
    ("media.add", &["paths"]),
    ("media.add_folder", &["path"]),
    ("media.get", &["media"]),
    ("media.details", &["media"]),
    ("media.move", &["media"]),
    ("media.remove", &["media"]),
    ("media.relink", &["media", "path"]),
    ("media.rename", &["media", "name"]),
    ("media.set", &["media"]),
    ("media.marks.set", &["media"]),
    ("media.marks.clear", &["media"]),
    ("proxy.generate", &["media"]),
    ("proxy.remove", &["media"]),
    ("bins.add", &["path"]),
    ("bins.remove", &["path"]),
    ("bins.rename", &["path", "to"]),
    ("timeline.get", &["range"]),
    ("timeline.at", &["at"]),
    ("timeline.import", &["path"]),
    ("track.add", &["kind"]),
    ("track.remove", &["track"]),
    ("track.set", &["track"]),
    ("clip.set", &["clips"]),
    ("keyframes.list", &["clips"]),
    ("keyframes.add", &["clips"]),
    ("keyframes.set", &["clips"]),
    ("keyframes.move", &["clips"]),
    ("keyframes.remove", &["clips"]),
    ("markers.move", &["marker"]),
    ("markers.remove", &["marker"]),
    ("markers.set", &["marker"]),
    ("subtitles.list", &["track"]),
    ("subtitles.add", &["text"]),
    ("subtitles.set", &["cue"]),
    ("subtitles.move", &["cues"]),
    ("subtitles.remove", &["cues"]),
    ("subtitles.split", &["cue"]),
    ("subtitles.join", &["cues"]),
    ("subtitles.match_speaker", &["cue"]),
    ("speakers.set", &["person"]),
    ("speakers.join", &["person"]),
    ("subtitles.import", &["path"]),
    ("subtitles.export", &["out"]),
    ("titles.list", &["track"]),
    ("titles.add", &["text"]),
    ("titles.set", &["clip"]),
    ("picture.get", &["clip"]),
    ("picture.set", &["clips"]),
    ("picture.clear", &["clips"]),
    ("edit.append", &["media"]),
    ("edit.smart_insert", &["media"]),
    ("edit.insert", &["media"]),
    ("edit.overwrite", &["media"]),
    ("edit.ripple_overwrite", &["media"]),
    ("edit.place_on_top", &["media"]),
    ("edit.delete", &["clips"]),
    ("edit.delete_range", &["range"]),
    ("edit.move", &["clips"]),
    ("edit.replace", &["clip"]),
    ("edit.trim", &["clip"]),
    ("edit.roll", &["clip"]),
    ("edit.slip", &["clip"]),
    ("edit.slide", &["clip"]),
    ("edit.audio_roll", &["clip"]),
    ("edit.ripple_delete_silence", &["clips"]),
    ("edit.detect_scene_cuts", &["clips"]),
    ("edit.copy", &["clips"]),
    ("edit.cut", &["clips"]),
    ("edit.change_duration", &["clips"]),
    ("edit.change_speed", &["clips"]),
    ("export.start", &["output"]),
    ("export.plan", &["output"]),
    ("jobs.get", &["job"]),
    ("jobs.wait", &["job"]),
    ("jobs.cancel", &["job"]),
    ("presets.get", &["name"]),
    ("presets.save", &["name"]),
    ("presets.delete", &["name"]),
    ("history.undo", &["steps"]),
    ("history.redo", &["steps"]),
    ("history.go", &["step"]),
    ("history.mark", &["name"]),
    ("history.unmark", &["step"]),
    ("history.restore", &["branch"]),
    ("history.revert", &["step"]),
    ("viewer.set", &["mode"]),
    ("viewer.load", &["media"]),
    ("player.seek", &["at"]),
    ("player.step", &["frames"]),
    ("player.mute", &["mute"]),
    ("player.stats", &["show"]),
    ("player.audio_scrubbing", &["on"]),
    ("player.loop", &["on"]),
    ("mixer.preview", &["track", "volume_db"]),
    ("gui.show", &["panel"]),
    ("gui.close", &["panel"]),
    ("gui.target", &["track"]),
    ("gui.trim_editor", &["at"]),
];

const GROUPS: &[(&str, &str)] = &[
    (
        "analyze",
        "Measure media or clips: silences, shots, black, loudness, the words said",
    ),
    ("bins", "Media Pool bins"),
    ("clip", "Clip properties"),
    ("decoders", "Hardware decoders that work here"),
    ("edit", "Edit the timeline"),
    ("encoders", "Hardware encoders that work here"),
    (
        "export",
        "Export the timeline: a video, audio, frames per clip, or OpenTimelineIO",
    ),
    ("frame", "Single frames as PNG"),
    (
        "gui",
        "The running GUI: its window, command log, screenshots, panels, target track and timeline view",
    ),
    ("history", "Undo and redo, across runs"),
    (
        "jobs",
        "Export, proxy and transcription jobs of the running GUI",
    ),
    (
        "keyframes",
        "Volume keyframes of clips: the curve of a clip's volume",
    ),
    ("keys", "Keyboard shortcuts"),
    (
        "looks",
        "Looks: named styles for subtitle tracks and titles, built-in and your own",
    ),
    ("markers", "Timeline markers"),
    ("marks", "Timeline In and Out marks"),
    ("media", "The Media Pool"),
    ("media.marks", "A media item's source In and Out marks"),
    ("mixer", "The Mixer"),
    (
        "picture",
        "A clip's picture over the frame: moved, zoomed, turned, flipped, cropped",
    ),
    ("player", "The viewer's playback (running GUI)"),
    ("pool", "The GUI's Media Pool view"),
    (
        "preferences",
        "Application preferences (~/.config/vidcut/preferences.toml; on Windows in %APPDATA%\\vidcut, on macOS in ~/Library/Application Support/vidcut)",
    ),
    ("presets", "Export presets: built-in and your own"),
    ("project", "Project files and settings"),
    (
        "proxy",
        "Playback proxies: smaller copies the preview plays",
    ),
    (
        "selection",
        "The GUI's selection: timeline clips, subtitle cues, pool media and an edit point",
    ),
    ("services", "The paid speech services' keys: xAI and Google"),
    (
        "speakers",
        "Who speaks: names, colors, files' speakers joined, words reassigned",
    ),
    ("storyboard", "Storyboards: a labeled picture of every shot"),
    (
        "subtitles",
        "Subtitle cues: transcribe the speech, import and export SRT, WebVTT and TTML, add and change cues",
    ),
    ("tape", "The Source Tape"),
    ("tape.marks", "The Source Tape's In and Out marks"),
    (
        "titles",
        "Titles: text over the picture, as clips on the video tracks",
    ),
    (
        "timeline",
        "The timeline: clips and gaps, edit points, the Boring Detector, OTIO import",
    ),
    ("track", "Tracks: their header and mixer settings"),
    (
        "viewer",
        "What the viewer shows. Framing guides are a preference; the mode needs a running GUI",
    ),
];

const EXAMPLES: &[(&str, &str)] = &[
    (
        "",
        "vidcut project create talk.vidcut --fps 25 --width 1920 --height 1080\n  \
         vidcut -p talk.vidcut media add cam.mp4 screen.mkv\n  \
         vidcut -p talk.vidcut edit append m1 --range 10s..2m\n  \
         vidcut -p talk.vidcut edit split --at 1m,1m30s\n  \
         vidcut -p talk.vidcut edit delete-range 2m..2m04s\n  \
         vidcut -p talk.vidcut timeline get --json",
    ),
    (
        "project.create",
        "vidcut project create talk.vidcut\n  \
         vidcut project create talk.vidcut --fps 30000/1001 --width 1920 --height 1080\n  \
         vidcut project create cut.vidcut --from cut.otio",
    ),
    (
        "project.set",
        "vidcut project set --ripple-on false\n  vidcut project set --use-proxies true\n  vidcut project set --still-duration 3s",
    ),
    (
        "media.add",
        "vidcut media add cam.mp4 screen.mkv --bin Interviews\n  vidcut media add renders/frame_000001.png --sequence --fps 60",
    ),
    (
        "media.add_folder",
        "vidcut media add-folder ~/footage/shoot\n  vidcut media add-folder renders --bin VFX --sequence --fps 24",
    ),
    (
        "media.marks.set",
        "vidcut media marks set m1 --in 5s --out 10s",
    ),
    (
        "viewer.guides",
        "vidcut viewer guides\n  \
         vidcut viewer guides --enabled true --framing social_9_16\n  \
         vidcut viewer guides --center true\n  \
         vidcut preferences set viewer.guides.action=false",
    ),
    ("media.set", "vidcut media set m1 m2 --color teal"),
    (
        "clip.set",
        "vidcut clip set c4 --color pink\n  vidcut clip set c4 --color \"\"\n  vidcut clip set c4 --voice-isolation 75 --download\n  vidcut clip set c4 --voice-isolation 0",
    ),
    (
        "keyframes.add",
        "vidcut keyframes add c4 --at 10s\n  vidcut keyframes add c4 --at 12s --value -20 --ease ease_in\n  vidcut keyframes list c4",
    ),
    (
        "proxy.generate",
        "vidcut proxy generate\n  vidcut proxy generate m1 m2 --width 1280\n  vidcut proxy generate --auto\n  vidcut project set --use-proxies true",
    ),
    (
        "edit.append",
        "vidcut edit append m1\n  vidcut edit append m1 --range 10s..+20s --track V2",
    ),
    ("edit.split", "vidcut edit split --at 1m,1m30s"),
    (
        "edit.join",
        "vidcut edit join --at 1m
  vidcut edit join --clips c3,c4,c5",
    ),
    (
        "edit.dissolve",
        "vidcut edit dissolve\n  vidcut edit dissolve --at 1m --length 12f --align start\n  vidcut edit dissolve --at 1m --before 10f --after 17f\n  vidcut edit dissolve --clips c3,c4\n  vidcut edit dissolve --all",
    ),
    (
        "edit.remove_transition",
        "vidcut edit remove-transition --at 1m\n  vidcut edit remove-transition --all",
    ),
    (
        "edit.trim",
        "vidcut edit trim c3 --edge tail --by -12f\n  vidcut edit trim c3 --edge head --to 1m --ripple false",
    ),
    (
        "edit.move",
        "vidcut edit move c7 --to 2m --track V2 --mode overwrite",
    ),
    (
        "edit.ripple_delete_silence",
        "vidcut edit ripple-delete-silence c1 --dry-run\n  vidcut edit ripple-delete-silence c1 c2 --threshold-db -35 --pre-head 0.2s --post-tail 0.2s\n  vidcut edit ripple-delete-silence c1 --detect speech --speech-threshold 0.6",
    ),
    (
        "edit.paste",
        "vidcut edit copy c3 --json > clips.json\n  vidcut edit paste --clips @clips.json --at 1m",
    ),
    (
        "track.set",
        "vidcut track set A1 --volume-db -6 --name Music\n  vidcut track set V2 --locked true\n  vidcut track set A2 --solo true\n  vidcut track set A1 --voice-isolation 75\n  \
         vidcut track set A2 --duck '{}' --download\n  \
         vidcut track set A2 --duck '{\"sources\":[\"V1\"],\"amount_db\":6,\"detect\":\"level\"}'\n  \
         vidcut track set A2 --duck '{\"on\":false}'\n  \
         vidcut track set A2 --duck null",
    ),
    (
        "titles.add",
        "vidcut titles add \"Opening night\" --at 10s\n  \
         vidcut titles add \"Ada Lovelace\" --kind lower_third --align right --at 1m --duration 4s\n  \
         vidcut titles add \"The end\" --at 2m --position 0.5,0.8 --style '{\"color\":\"yellow\",\"size\":0.1}'",
    ),
    (
        "titles.set",
        "vidcut titles set c4 --text \"Opening night\"\n  \
         vidcut titles set V2@10s --kind lower_third --align middle\n  \
         vidcut titles set c4 --position 0.5,0.2",
    ),
    (
        "picture.set",
        "vidcut picture set c3 --zoom 0.33 --position 0.8,0.2\n  \
         vidcut picture set c3 --zoom 1.5,1 --rotation 10 --flip-horizontal true\n  \
         vidcut picture set V2@10s --crop-left 0.25 --crop-right 0.25\n  \
         vidcut picture set c3 --dynamic-zoom '{}'\n  \
         vidcut picture set c3 --dynamic-zoom '{\"end\":{\"center\":[0.3,0.4],\"size\":0.5},\"ease\":\"ease_in_out\"}'\n  \
         vidcut picture set c3 --dynamic-zoom null",
    ),
    (
        "picture.clear",
        "vidcut picture clear c3\n  vidcut picture clear c3 --tool crop\n  \
         vidcut picture clear c3 --tool dynamic_zoom",
    ),
    (
        "export.start",
        "vidcut export start out.mp4\n  vidcut export start out.mp4 --preset youtube-1080p --encoder nvenc\n  \
         vidcut export start cut.mov --range 1m..2m --set video.quality=14\n  \
         vidcut export start clips/ --per-clip --preset prores-422hq\n  \
         vidcut export start voice.wav --audio-only\n  vidcut export start cut.otio\n  vidcut export start cut.otioz\n  \
         vidcut export start out.mp4 --background && vidcut jobs wait j1",
    ),
    (
        "timeline.import",
        "vidcut -p cut.vidcut timeline import their-cut.otio\n  \
         vidcut -p cut.vidcut timeline import cut.otio --mode replace --search /mnt/footage\n  \
         vidcut -p cut.vidcut timeline import cut.otioz --unpack /mnt/fast/cut-media",
    ),
    (
        "storyboard.render",
        "vidcut storyboard render --out board.png\n  vidcut storyboard render --out board.png --every 10s --columns 6 --width 240",
    ),
    (
        "frame.render",
        "vidcut frame render --at 1m --out f.png --width 640",
    ),
    (
        "presets.save",
        "vidcut presets save web --from youtube-1080p --set video.quality=22 --quick-export\n  \
         vidcut presets save master --settings @settings.json --title \"Our master\"",
    ),
    (
        "preferences.set",
        "vidcut preferences set cache.percent=5\n  \
         vidcut preferences set cache.limit=gb cache.gb=50   # a fixed size\n  \
         vidcut preferences set thumbnails.disk_cache=false   # also deletes those kept\n  \
         vidcut preferences set proxies.codec=auto\n  \
         vidcut preferences set proxies.folder=\"$HOME/proxies\"   # one folder for every project",
    ),
    (
        "player.seek",
        "vidcut player seek 10s\n  vidcut player seek -2s",
    ),
    ("mixer.preview", "vidcut mixer preview A1 -12"),
    ("mcp", "claude mcp add vidcut -- vidcut mcp -p talk.vidcut"),
];

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Flag,
    Bool,
    Int,
    Float,
    Text,
    Choice(Vec<String>),
    List(Box<Kind>),
    Json,
}

#[derive(Debug, Clone)]
struct Param {
    name: String,
    kind: Kind,
    required: bool,
    help: String,
    position: Option<usize>,
}

#[derive(Debug, Clone)]
struct Method {
    name: String,
    description: String,
    params: Vec<Param>,
}

const CUSTOM: &[&str] = &["apply", "preferences.set"];

fn methods() -> &'static [Method] {
    static METHODS: OnceLock<Vec<Method>> = OnceLock::new();
    METHODS.get_or_init(|| {
        let root = protocol_schema();
        let empty = Map::new();
        let defs = root["$defs"].as_object().unwrap_or(&empty);
        let mut out = Vec::new();
        for variant in root["oneOf"].as_array().into_iter().flatten() {
            let Some(&name) = method_names(variant).first() else {
                continue;
            };
            let positional = POSITIONAL
                .iter()
                .find(|(m, _)| *m == name)
                .map_or(&[][..], |(_, p)| *p);
            let params_schema = resolve_ref(
                variant["properties"].get("params").unwrap_or(&Value::Null),
                defs,
            );
            let required: Vec<&str> = params_schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let mut params: Vec<Param> = params_schema["properties"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(p, schema)| Param {
                    name: p.clone(),
                    kind: kind(schema, defs),
                    required: required.contains(&p.as_str()),
                    help: help(schema),
                    position: positional.iter().position(|x| x == p),
                })
                .collect();
            params.sort_by_key(|p| (p.position.is_none(), p.position));
            out.push(Method {
                name: name.to_string(),
                description: variant["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                params,
            });
        }
        out
    })
}

fn method(name: &str) -> Option<&'static Method> {
    methods().iter().find(|m| m.name == name)
}

fn not_null(schema: &Value) -> Value {
    if let Some(types) = schema["type"].as_array() {
        let rest: Vec<&Value> = types.iter().filter(|t| *t != "null").collect();
        if rest.len() == 1 {
            let mut s = schema.clone();
            s["type"] = rest[0].clone();
            return s;
        }
    }
    if let Some(any) = schema["anyOf"].as_array() {
        let rest: Vec<&Value> = any.iter().filter(|a| a["type"] != "null").collect();
        if rest.len() == 1 && rest.len() < any.len() {
            return rest[0].clone();
        }
    }
    schema.clone()
}

fn nullable(schema: &Value) -> bool {
    schema["type"]
        .as_array()
        .is_some_and(|t| t.iter().any(|t| t == "null"))
        || schema["anyOf"]
            .as_array()
            .is_some_and(|a| a.iter().any(|a| a["type"] == "null"))
}

fn kind(schema: &Value, defs: &Map<String, Value>) -> Kind {
    let may_be_unset = nullable(schema) || schema.get("default") == Some(&Value::Null);
    let base = not_null(schema);
    let s = resolve_ref(&base, defs);
    match s["type"].as_str() {
        Some("boolean") => {
            if !may_be_unset && s["default"] != true && schema["default"] != true {
                Kind::Flag
            } else {
                Kind::Bool
            }
        }
        Some("integer") => Kind::Int,
        Some("number") => Kind::Float,
        Some("string") => match choices(s) {
            Some(c) => Kind::Choice(c),
            None => Kind::Text,
        },
        Some("array") => match kind(&s["items"], defs) {
            k @ (Kind::Text | Kind::Int | Kind::Float | Kind::Choice(_)) => Kind::List(Box::new(k)),
            _ => Kind::Json,
        },
        Some(_) => Kind::Json,
        None => {
            if let Some(c) = choices(s) {
                return Kind::Choice(c);
            }
            let alts: Vec<&Value> = s["anyOf"]
                .as_array()
                .or_else(|| s["oneOf"].as_array())
                .into_iter()
                .flatten()
                .map(|a| resolve_ref(a, defs))
                .collect();
            if let Some(list) = alts.iter().find(|a| a["type"] == "array") {
                return match kind(list, defs) {
                    Kind::List(k) => Kind::List(k),
                    _ => Kind::Json,
                };
            }
            if !alts.is_empty()
                && alts
                    .iter()
                    .all(|a| matches!(a["type"].as_str(), Some("string" | "integer" | "number")))
            {
                return Kind::Text;
            }
            Kind::Json
        }
    }
}

fn choices(s: &Value) -> Option<Vec<String>> {
    if let Some(e) = s["enum"].as_array() {
        return Some(
            e.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
        );
    }
    let alts = s["oneOf"].as_array()?;
    let mut out = Vec::new();
    for a in alts {
        if let Some(c) = a["const"].as_str() {
            out.push(c.to_string());
            continue;
        }
        let e = a["enum"].as_array()?;
        out.extend(e.iter().filter_map(|v| v.as_str().map(String::from)));
    }
    Some(out)
}

fn help(schema: &Value) -> String {
    let mut h = schema["description"]
        .as_str()
        .unwrap_or_default()
        .replace('\n', " ");
    match &schema["default"] {
        Value::Null | Value::Bool(false) => {}
        Value::String(s) if s.is_empty() => {}
        Value::Array(a) if a.is_empty() => {}
        Value::String(s) => h.push_str(&format!(" [default: {s}]")),
        d => h.push_str(&format!(" [default: {d}]")),
    }
    h
}

fn cli_name(s: &str) -> String {
    s.replace('_', "-")
}

pub fn command() -> Command {
    let mut root = Command::new("vidcut")
        .version(format!(
            "{} (FFmpeg {})",
            env!("CARGO_PKG_VERSION"),
            vidcut_av::ffmpeg_version()
        ))
        .about("A simple, fully scriptable video cutting tool.")
        .long_about(
            "A simple, fully scriptable video cutting tool. Without a command, the GUI starts.\n\n\
             Every command is a request method of the protocol: `vidcut edit split` is `edit.split` \
             (see `vidcut schema`), and its flags are the method's parameters.\n\
             Times: 450f (frames), 15s, 1m30s, 250ms, HH:MM:SS:FF (timecode), HH:MM:SS.mmm, start, end, in, out.\n\
             Ranges are half-open: A..B (B excluded) or A..+D. Clips: c17, or V1@<time> for the clip under a frame.\n\
             With --json, output is the response object.",
        )
        .after_help(format!("Examples:\n  {}", examples("").unwrap_or_default()))
        .subcommand_required(false)
        .arg(
            Arg::new("project")
                .help_heading("Global options")
                .short('p')
                .long("project")
                .global(true)
                .env("VIDCUT_PROJECT")
                .value_parser(clap::value_parser!(PathBuf))
                .help("Project file"),
        )
        .arg(
            Arg::new("json")
                .help_heading("Global options")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Print the response as JSON (errors too, on stdout)"),
        )
        .arg(
            Arg::new("dry_run")
                .help_heading("Global options")
                .long("dry-run")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Report what would change without changing anything"),
        )
        .arg(
            Arg::new("if_rev")
                .help_heading("Global options")
                .long("if-rev")
                .global(true)
                .value_name("REV")
                .value_parser(clap::value_parser!(u64))
                .help("Fail unless the project is at this revision"),
        )
        .arg(
            Arg::new("headless")
                .help_heading("Global options")
                .long("headless")
                .global(true)
                .action(ArgAction::SetTrue)
                .conflicts_with("instance")
                .help("Run here even if a GUI has the project open"),
        )
        .arg(
            Arg::new("instance")
                .help_heading("Global options")
                .long("instance")
                .global(true)
                .env("VIDCUT_SOCKET")
                .value_name("PID|SOCKET")
                .help("Send to this running GUI instead of choosing one"),
        )
        .arg(
            Arg::new("client")
                .help_heading("Global options")
                .long("client")
                .global(true)
                .env("VIDCUT_CLIENT")
                .value_name("NAME")
                .help("Who the undo history says made the change, like an agent's or a script's name"),
        );
    let mut groups: Vec<(String, Vec<&Method>)> = Vec::new();
    for m in methods() {
        let first = m.name.split('.').next().unwrap_or_default().to_string();
        match groups.iter_mut().find(|(g, _)| *g == first) {
            Some((_, list)) => list.push(m),
            None => groups.push((first, vec![m])),
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    for (group, list) in groups {
        root = root.subcommand(subtree(&group, &list, 1));
    }
    root.subcommand(
        Command::new("instances")
            .about("List running GUIs (commands for their projects go to them)"),
    )
    .subcommand(
        Command::new("mcp")
            .about(
                "Serve the Model Context Protocol on stdin/stdout, so AI agents can use vidcut \
                     as tools (one per request method, routed like these commands)",
            )
            .after_help(format!(
                "Example (Claude Code):\n  {}",
                examples("mcp").unwrap_or_default()
            )),
    )
    .subcommand(
        Command::new("rpc")
            .about("Send one raw request: `vidcut rpc edit.split '{\"at\": \"1m\"}'`")
            .arg(Arg::new("method").required(true))
            .arg(Arg::new("params").help("The params as JSON")),
    )
}

fn subtree(prefix: &str, list: &[&Method], depth: usize) -> Command {
    let last = prefix.rsplit('.').next().unwrap_or(prefix);
    if let [m] = list
        && m.name == prefix
    {
        return leaf(last, m);
    }
    let about = GROUPS
        .iter()
        .find(|(g, _)| *g == prefix)
        .map_or(String::new(), |(_, a)| a.to_string());
    let mut cmd = Command::new(cli_name(last))
        .about(about)
        .subcommand_required(true)
        .arg_required_else_help(true);
    let mut children: Vec<(String, Vec<&Method>)> = Vec::new();
    for m in list {
        let parts: Vec<&str> = m.name.split('.').collect();
        let Some(next) = parts.get(depth) else {
            continue;
        };
        let child = parts[..=depth].join(".");
        debug_assert!(!next.is_empty());
        match children.iter_mut().find(|(c, _)| *c == child) {
            Some((_, l)) => l.push(m),
            None => children.push((child, vec![*m])),
        }
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));
    for (child, l) in children {
        cmd = cmd.subcommand(subtree(&child, &l, depth + 1));
    }
    cmd
}

fn leaf(name: &str, m: &Method) -> Command {
    let first = m
        .description
        .split("\n\n")
        .next()
        .unwrap_or_default()
        .replace('\n', " ");
    let mut cmd = Command::new(cli_name(name))
        .about(first)
        .long_about(m.description.clone());
    if let Some(e) = examples(&m.name) {
        cmd = cmd.after_help(format!("Examples:\n  {e}"));
    }
    match m.name.as_str() {
        "apply" => {
            return cmd
                .arg(
                    Arg::new("file")
                        .required(true)
                        .help("Requests as JSON Lines or one JSON array; - for stdin"),
                )
                .arg(Arg::new("label").long("label").help("The undo step's name"));
        }
        "preferences.set" => {
            return cmd.arg(
                Arg::new("values")
                    .value_name("KEY=VALUE")
                    .required(true)
                    .num_args(1..)
                    .help("Preferences to change, like cache.percent=5"),
            );
        }
        _ => {}
    }
    let last = m.params.iter().filter_map(|p| p.position).max();
    for p in &m.params {
        cmd = cmd.arg(arg(p, p.position < last));
    }
    cmd
}

fn arg(p: &Param, followed: bool) -> Arg {
    let mut a = Arg::new(p.name.clone()).help(p.help.clone());
    a = match p.position {
        Some(i) => a.index(i + 1),
        None => a.long(cli_name(&p.name)),
    }
    .value_name(p.name.to_uppercase());
    if p.required && p.kind != Kind::Flag {
        a = a.required(true);
    }
    let one = |a: Arg, k: &Kind, hyphen: bool| match k {
        Kind::Int => a
            .value_parser(clap::value_parser!(i64))
            .allow_hyphen_values(hyphen),
        Kind::Float => a
            .value_parser(clap::value_parser!(f64))
            .allow_hyphen_values(hyphen),
        Kind::Choice(c) => a.value_parser(clap::builder::PossibleValuesParser::new(c.clone())),
        _ => a.allow_hyphen_values(hyphen),
    };
    match &p.kind {
        Kind::Flag => a.action(ArgAction::SetTrue),
        Kind::Bool => a
            .value_parser(clap::value_parser!(bool))
            .value_name("true|false"),
        Kind::List(item) if followed && p.position.is_some() => one(a, item, false)
            .value_delimiter(',')
            .action(ArgAction::Set),
        Kind::List(item) => one(a, item, false)
            .num_args(1..)
            .value_delimiter(',')
            .action(ArgAction::Append),
        Kind::Json => a.value_name("JSON|@FILE|-").allow_hyphen_values(true),
        k => one(a, k, true),
    }
}

fn examples(method: &str) -> Option<&'static str> {
    EXAMPLES.iter().find(|(m, _)| *m == method).map(|(_, e)| *e)
}

pub enum Action {
    Mcp,
    Run(Box<Envelope>),
    Gui { project: Option<PathBuf> },
    Instances,
}

pub struct Cli {
    pub project: Option<PathBuf>,
    pub json: bool,
    pub dry_run: bool,
    pub if_rev: Option<u64>,
    pub headless: bool,
    pub instance: Option<String>,
    pub client: Option<String>,
    chosen: Option<(Vec<String>, ArgMatches)>,
}

impl Cli {
    pub fn parse() -> Self {
        Self::from_matches(command().get_matches())
    }

    pub fn try_parse_from<I, T>(args: I) -> std::result::Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        Ok(Self::from_matches(command().try_get_matches_from(args)?))
    }

    fn from_matches(m: ArgMatches) -> Self {
        let mut names = Vec::new();
        let mut leaf = &m;
        while let Some((name, sub)) = leaf.subcommand() {
            names.push(name.to_string());
            leaf = sub;
        }
        Cli {
            project: m.get_one::<PathBuf>("project").cloned(),
            json: m.get_flag("json"),
            dry_run: m.get_flag("dry_run"),
            if_rev: m.get_one::<u64>("if_rev").copied(),
            headless: m.get_flag("headless"),
            instance: m.get_one::<String>("instance").cloned(),
            client: m.get_one::<String>("client").cloned(),
            chosen: (!names.is_empty()).then(|| (names, leaf.clone())),
        }
    }

    pub fn action(&self) -> Result<Option<Action>> {
        let Some((names, m)) = &self.chosen else {
            return Ok(None);
        };
        let request = match names.join(" ").as_str() {
            "instances" => return Ok(Some(Action::Instances)),
            "mcp" => return Ok(Some(Action::Mcp)),
            "rpc" => {
                let method = m.get_one::<String>("method").expect("required");
                let mut v = json!({ "method": method });
                if let Some(p) = m.get_one::<String>("params") {
                    v["params"] = serde_json::from_str(p)
                        .map_err(|e| Error::usage(format!("params: {e}")))?;
                }
                let mut env = Envelope::from_json(v).map_err(Error::usage)?;
                env.if_rev = env.if_rev.or(self.if_rev);
                env.dry_run |= self.dry_run;
                env.client.get_or_insert_with(|| self.client());
                return Ok(Some(Action::Run(Box::new(env))));
            }
            "apply" => {
                let file = m.get_one::<String>("file").expect("required");
                Request::Apply {
                    commands: read_batch(file)?,
                    label: Some(
                        m.get_one::<String>("label")
                            .cloned()
                            .unwrap_or_else(|| format!("apply {file}")),
                    ),
                }
            }
            "preferences set" => {
                let values: Vec<String> = m
                    .get_many::<String>("values")
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                let patch = vidcut_core::preferences::PreferencesPatch::from_pairs(&values)?;
                Request::from_parts("preferences.set", Some(json!(patch))).map_err(Error::usage)?
            }
            _ => {
                let name = names
                    .iter()
                    .map(|n| n.replace('-', "_"))
                    .collect::<Vec<_>>()
                    .join(".");
                let method = method(&name)
                    .ok_or_else(|| Error::usage(format!("unknown command {}", names.join(" "))))?;
                let mut params = Map::new();
                for p in &method.params {
                    if let Some(v) = value(m, p)? {
                        params.insert(p.name.clone(), v);
                    }
                }
                if method.name == "services.set_key" && !params.contains_key("key") {
                    let mut key = String::new();
                    std::io::stdin()
                        .read_line(&mut key)
                        .map_err(|e| Error::usage(format!("stdin: {e}")))?;
                    params.insert("key".into(), Value::String(key.trim().to_string()));
                }
                Request::from_parts(&method.name, Some(Value::Object(params)))
                    .map_err(Error::usage)?
            }
        };
        Ok(Some(Action::Run(Box::new(self.envelope(request)?))))
    }

    fn envelope(&self, mut request: Request) -> Result<Envelope> {
        for p in request.paths_mut() {
            *p = std::path::absolute(&*p)
                .map_err(|e| Error::usage(format!("{}: {e}", p.display())))?;
        }
        Ok(Envelope {
            request,
            if_rev: self.if_rev,
            dry_run: self.dry_run,
            client: Some(self.client()),
        })
    }

    fn client(&self) -> Client {
        Client::new(ClientKind::Cli, self.client.clone())
    }
}

fn value(m: &ArgMatches, p: &Param) -> Result<Option<Value>> {
    let id = p.name.as_str();
    Ok(match &p.kind {
        Kind::Flag => m.get_flag(id).then_some(Value::Bool(true)),
        Kind::Bool => m.get_one::<bool>(id).map(|b| json!(b)),
        Kind::Int => m.get_one::<i64>(id).map(|n| json!(n)),
        Kind::Float => m.get_one::<f64>(id).map(|n| json!(n)),
        Kind::Text | Kind::Choice(_) => m.get_one::<String>(id).map(|s| json!(s)),
        Kind::List(item) => match **item {
            Kind::Int => m
                .get_many::<i64>(id)
                .map(|v| json!(v.copied().collect::<Vec<_>>())),
            Kind::Float => m
                .get_many::<f64>(id)
                .map(|v| json!(v.copied().collect::<Vec<_>>())),
            _ => m
                .get_many::<String>(id)
                .map(|v| json!(v.cloned().collect::<Vec<_>>())),
        },
        Kind::Json => match m.get_one::<String>(id) {
            Some(text) => Some(read_json_arg(text)?),
            None => None,
        },
    })
}

pub fn command_line(project: &std::path::Path, method: &str, params: &Value) -> Option<String> {
    if CUSTOM.contains(&method) {
        return None;
    }
    let m = self::method(method)?;
    let mut words: Vec<String> = vec!["vidcut".into(), "-p".into(), project.display().to_string()];
    words.extend(m.name.split('.').map(cli_name));
    let mut flags = Vec::new();
    for p in &m.params {
        let v = &params[&p.name];
        if v.is_null() {
            continue;
        }
        let text = |v: &Value| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let values: Vec<String> = match (&p.kind, v) {
            (Kind::Flag, Value::Bool(true)) => vec![],
            (Kind::Flag, _) => continue,
            (Kind::List(_), Value::Array(a)) => a.iter().map(text).collect(),
            (Kind::Json, v) => vec![v.to_string()],
            (_, v) => vec![text(v)],
        };
        match p.position {
            Some(_) => words.extend(values),
            None if p.kind == Kind::Flag => flags.push(format!("--{}", cli_name(&p.name))),
            None => {
                for value in values {
                    flags.push(format!("--{}", cli_name(&p.name)));
                    flags.push(value);
                }
            }
        }
    }
    words.extend(flags);
    shlex::try_join(words.iter().map(String::as_str)).ok()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_group_says_what_it_is_for() {
        fn unexplained(cmd: &Command, out: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                if sub.has_subcommands() && sub.get_about().is_none_or(|a| a.to_string().is_empty())
                {
                    out.push(sub.get_name().to_string());
                }
                unexplained(sub, out);
            }
        }
        let mut out = Vec::new();
        unexplained(&command(), &mut out);
        assert_eq!(out, Vec::<String>::new());
    }
}
