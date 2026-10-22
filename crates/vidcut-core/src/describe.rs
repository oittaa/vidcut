use serde_json::Value;

use crate::model::Project;
use crate::time::{DurationSpec, Keyword, RangeSpec, TimeSpec};

pub fn describe(method: &str, params: &Value, project: Option<&Project>) -> String {
    describe_at(method, params, project, None)
}

pub fn describe_at(
    method: &str,
    params: &Value,
    project: Option<&Project>,
    playhead: Option<i64>,
) -> String {
    let d = Words {
        params,
        project,
        playhead,
    };
    match method {
        "project.set" => format!("Project: {}", d.settings()),
        "project.open" => format!("Open {}", d.file("path")),
        "project.create" => format!("Create {}", d.file("path")),
        "media.add" => format!("Import {}", d.files("paths")),
        "media.add_folder" => format!("Import folder {}", d.file("path")),
        "media.remove" => format!("Remove {}", d.media("media")),
        "media.relink" if d.params.get("path").is_some_and(|v| !v.is_null()) => {
            format!("Relink {} to {}", d.media("media"), d.file("path"))
        }
        "media.relink" => format!(
            "Relink {} from {}",
            d.media_or("media", "missing media"),
            d.files("search")
        ),
        "media.rename" => format!("Rename {} to {}", d.media("media"), d.text("name")),
        "media.set" if d.text("color").is_empty() => {
            format!("Clear the color of {}", d.media("media"))
        }
        "media.set" if d.params["color"].is_null() => {
            let space = match d.text("color_space").as_str() {
                "bt709" => "BT.709",
                "bt601" => "BT.601",
                "pq" => "BT.2100 PQ",
                "hlg" => "BT.2100 HLG",
                _ => "its tags say",
            };
            format!("Read the colors of {} as {space}", d.media("media"))
        }
        "media.set" => format!("Color {} {}", d.media("media"), d.text("color")),
        "media.marks.set" => format!("Source marks of {}: {}", d.media("media"), d.in_out()),
        "media.marks.clear" => format!("Clear the source marks of {}", d.media("media")),
        "tape.marks.set" => format!("Source Tape marks: {}", d.in_out()),
        "tape.marks.clear" => "Clear the Source Tape marks".into(),
        "media.move" => format!("Move {} to bin {}", d.media("media"), d.bin("bin")),
        "proxy.generate" if d.flag("auto") => format!(
            "Make proxies by themselves: {}",
            d.media_or("media", "the pool")
        ),
        "proxy.generate" => format!("Make proxies: {}", d.media_or("media", "every video")),
        "proxy.remove" => format!("Remove proxies: {}", d.media_or("media", "all")),
        "bins.add" => format!("New bin {}", d.text("path")),
        "bins.remove" => format!("Remove bin {}", d.text("path")),
        "bins.rename" => format!("Rename bin {} to {}", d.text("path"), d.text("to")),
        "timeline.import" => match d.text("mode").as_str() {
            "replace" => format!("Import timeline {}, replacing the timeline", d.file("path")),
            "compound" => format!("Import timeline {} as a compound clip", d.file("path")),
            "media" => format!("Import the media of {}", d.file("path")),
            _ => format!("Import timeline {}", d.file("path")),
        },
        "edit.append" => format!("Append {}", d.source()),
        "edit.smart_insert" => format!("Smart Insert {}", d.source()),
        "edit.insert" => format!("Insert {} at {}", d.source(), d.time("at")),
        "edit.overwrite" => format!("Overwrite {} at {}", d.source(), d.time("at")),
        "edit.ripple_overwrite" => format!("Ripple Overwrite {}", d.source()),
        "edit.place_on_top" => format!("Place On Top {} at {}", d.source(), d.time("at")),
        "edit.split" => d.split(),
        "edit.join" if d.list("clips").len() > 1 => format!("Join {}", d.clips("clips")),
        "edit.join" => format!("Join the clips at {}", d.time("at")),
        "edit.dissolve" if d.flag("all") => "Add dissolves to all edits".to_string(),
        "edit.dissolve" if !d.list("clips").is_empty() => {
            format!("Add dissolves to {}", d.clips("clips"))
        }
        "edit.dissolve" => format!("Add a dissolve at {}{}", d.time("at"), d.track_or_v1()),
        "edit.remove_transition" if d.flag("all") => {
            "Clear the transitions from all edits".to_string()
        }
        "edit.remove_transition" => {
            format!("Make the edit at {}{} a cut", d.time("at"), d.track_or_v1())
        }
        "edit.delete" if d.flag("lift") => format!(
            "Delete {}{}, leaving a gap",
            d.clips("clips"),
            d.place("clips", None)
        ),
        "edit.delete" => format!("Delete {}{}", d.clips("clips"), d.place("clips", None)),
        "edit.delete_range" => format!("Delete {}", d.range("range")),
        "edit.close_gap" => format!("Close the gap at {}", d.time("at")),
        "edit.move" => format!(
            "Move {} to {}{}",
            d.clips("clips"),
            d.time("to"),
            d.on_track()
                .or_else(|| d.track_of("clips"))
                .unwrap_or_default()
        ),
        "edit.replace" => format!("Replace {} with {}", d.clips("onto"), d.clips("clip")),
        "edit.trim" => format!(
            "Trim the {} of {} {}{}",
            d.edge(),
            d.clips("clip"),
            d.by(),
            d.place("clip", d.params["edge"].as_str())
        ),
        "edit.roll" => format!(
            "Roll the cut {} {} {}{}",
            match d.params["edge"].as_str() {
                Some("head") => "before",
                _ => "after",
            },
            d.clips("clip"),
            d.by(),
            d.place("clip", d.params["edge"].as_str())
        ),
        "edit.slip" => format!(
            "Slip {} {}{}",
            d.clips("clip"),
            d.by(),
            d.place("clip", None)
        ),
        "edit.decompose" => format!("Decompose {}", d.clips("clips")),
        "edit.slide" => format!(
            "Slide {} {}{}",
            d.clips("clip"),
            d.by(),
            d.place("clip", None)
        ),
        "edit.audio_roll" => format!(
            "Move the audio cut at the {} of {} {}{}",
            d.edge(),
            d.clips("clip"),
            d.by(),
            d.place("clip", d.params["edge"].as_str())
        ),
        "edit.ripple_delete_silence" => format!("Ripple Delete Silence in {}", d.clips("clips")),
        "edit.detect_scene_cuts" => format!("Detect Scene Cuts in {}", d.clips("clips")),
        "edit.trim_to_playhead" => format!(
            "Trim the {} of {} to the playhead",
            d.edge(),
            d.clips("clips")
        ),
        "edit.change_duration" => format!(
            "Change the duration of {} to {}",
            d.clips("clips"),
            d.length("duration")
        ),
        "edit.change_speed" => format!(
            "Change the speed of {} to {}",
            d.clips("clips"),
            match d.params["speed"].as_f64() {
                Some(0.0) => "a freeze frame".to_string(),
                Some(x) => format!("{}%", (x * 100.0).round()),
                None => "?".to_string(),
            }
        ),
        "edit.copy" => format!("Copy {}", d.clips("clips")),
        "edit.cut" => format!("Cut {}", d.clips("clips")),
        "edit.paste" => format!("Paste at {}", d.time("at")),
        "track.add" => {
            let n = params["count"].as_u64().unwrap_or(1);
            let kind = d.text("kind");
            if n == 1 {
                format!("Add a {kind} track")
            } else {
                format!("Add {n} {kind} tracks")
            }
        }
        "track.remove" => format!("Remove track {}", d.text("track")),
        "track.set" => format!("{}: {}", d.text("track"), d.track_settings()),
        "clip.set" => format!("{}: {}", d.clips("clips"), d.clip_settings()),
        "marks.set" => format!("Timeline marks: {}", d.in_out()),
        "marks.clear" => "Clear the timeline marks".into(),
        "markers.add" => format!("Marker at {}{}", d.time("at"), d.named()),
        "markers.move" => format!("Move marker {} to {}", d.text("marker"), d.time("to")),
        "markers.set" => format!("Change marker {}{}", d.text("marker"), d.named()),
        "markers.remove" => format!("Remove marker {}", d.text("marker")),
        "subtitles.add" => format!("Subtitle {} at {}", quoted(&d.text("text")), d.time("at")),
        "subtitles.set" => format!("Change subtitle {}", d.cues("cue")),
        "subtitles.move" if d.params["track"].is_string() => {
            format!("Move {} to {}", d.cues("cues"), d.text("track"))
        }
        "subtitles.move" => format!("Move {} {}", d.cues("cues"), d.by()),
        "subtitles.remove" => format!("Remove {}", d.cues("cues")),
        "subtitles.split" => format!("Split {} at {}", d.cues("cue"), d.time("at")),
        "subtitles.join" => format!("Join {}", d.cues("cues")),
        "subtitles.match_speaker" => format!("Match the speaker's cues to {}", d.cues("cue")),
        "titles.add" => format!("Title {} at {}", quoted(&d.text("text")), d.time("at")),
        "titles.set" => format!("Change title {}", d.clips("clip")),
        "picture.set" => {
            let crops = ["crop_left", "crop_right", "crop_top", "crop_bottom"]
                .iter()
                .any(|k| !d.params[*k].is_null());
            let moves = [
                "position",
                "zoom",
                "rotation",
                "flip_horizontal",
                "flip_vertical",
            ]
            .iter()
            .any(|k| !d.params[*k].is_null());
            let clips = d.clips("clips");
            match (moves, crops, d.params.get("dynamic_zoom")) {
                (false, false, Some(Value::Null)) => format!("Reset the Dynamic Zoom of {clips}"),
                (false, false, Some(_)) => format!("Change the Dynamic Zoom of {clips}"),
                (true, true, _) => format!("Transform and crop {clips}"),
                (false, true, _) => format!("Crop {clips}"),
                _ => format!("Transform {clips}"),
            }
        }
        "picture.clear" => match d.params["tool"].as_str() {
            Some("crop") => format!("Reset the crop of {}", d.clips("clips")),
            Some("dynamic_zoom") => format!("Reset the Dynamic Zoom of {}", d.clips("clips")),
            Some(_) => format!("Reset the transform of {}", d.clips("clips")),
            None => format!("Reset the picture of {}", d.clips("clips")),
        },
        "subtitles.import" if d.params["media"].is_string() => {
            format!("Insert subtitles {}", d.media("media"))
        }
        "subtitles.import" => format!("Import subtitles {}", d.file("path")),
        "subtitles.export" => format!("Export subtitles to {}", d.file("out")),
        "analyze.words" if d.params["range"].is_string() => {
            format!("Words in {}", d.range("range"))
        }
        "analyze.words" => "Words in the speech".into(),
        "media.transcribe" => format!("Transcribe {}", d.media("media")),
        "media.leave_out" => format!(
            "{} {} word(s) of {}",
            if d.params["back"].as_bool() == Some(true) {
                "Bring back"
            } else {
                "Leave out"
            },
            d.params["words"].as_array().map_or(0, Vec::len),
            d.media("media")
        ),
        "edit.ripple_delete_words" => match d.params["words"].as_array().map(Vec::as_slice) {
            Some([one]) if one["text"].as_str().is_some_and(|t| !t.trim().is_empty()) => {
                format!(
                    "Ripple Delete {}",
                    quoted(one["text"].as_str().unwrap_or_default())
                )
            }
            words => format!("Ripple Delete {} word(s)", words.map_or(0, <[Value]>::len)),
        },
        "subtitles.transcribe" if d.params["range"].is_string() => {
            format!("Create Subtitles from Audio in {}", d.range("range"))
        }
        "subtitles.transcribe" => "Create Subtitles from Audio".into(),
        "subtitles.retime" if d.params["track"].is_string() => {
            format!("Re-time {} to the speech", d.text("track"))
        }
        "subtitles.retime" => "Re-time subtitles to the speech".into(),
        "subtitles.time_words" if d.params["track"].is_string() => {
            format!("Synchronize Word Timings on {}", d.text("track"))
        }
        "subtitles.time_words" => "Synchronize Word Timings".into(),
        "export.start" => format!("Export {}", d.file("output")),
        "export.plan" => format!("Plan an export to {}", d.file("output")),
        "frame.render" => format!("Render the frame at {}", d.time("at")),
        "storyboard.render" => format!("Storyboard {}", d.file("out")),
        "presets.save" => format!("Save preset {}", d.text("name")),
        "presets.delete" => format!("Delete preset {}", d.text("name")),
        "preferences.set" => format!("Preferences: {}", compact(params)),
        "history.undo" => "Undo".into(),
        "history.redo" => "Redo".into(),
        "history.mark" => match params["step"].as_u64() {
            Some(n) => format!("Name step {n} \u{201c}{}\u{201d}", d.text("name")),
            None => format!("Name this version \u{201c}{}\u{201d}", d.text("name")),
        },
        "history.unmark" => match params["step"].as_u64() {
            Some(n) => format!("Take the name off step {n}"),
            None => "Take the name off this version".into(),
        },
        "history.restore" => format!("Restore version {}", d.text("branch")),
        "history.revert" => format!("Revert step {}", d.text("step")),
        "history.go" => match params["step"].as_u64() {
            Some(0) => "Go back to the start of the undo history".into(),
            Some(n) => format!("Go to step {n} of the undo history"),
            None => "Go to a step of the undo history".into(),
        },
        "apply" => match params["label"].as_str() {
            Some(l) => l.to_string(),
            None => format!(
                "{} commands as one",
                params["commands"].as_array().map_or(0, Vec::len)
            ),
        },
        "viewer.set" => format!("Viewer: {}", d.viewer()),
        "viewer.load" => format!("Show {} in the viewer", d.media("media")),
        "player.play" => "Play".into(),
        "player.pause" => "Pause".into(),
        "player.toggle" => "Play or pause".into(),
        "player.seek" => format!("Go to {}", d.time("at")),
        "player.step" => match params["frames"].as_i64() {
            Some(n) => format!("Step {}", frames(n)),
            None => format!("Step {} frames", params["frames"]),
        },
        "player.mute" => match params["mute"].as_bool() {
            Some(true) => "Mute the viewer".into(),
            Some(false) => "Unmute the viewer".into(),
            None => "Mute or unmute the viewer".into(),
        },
        "player.stats" => match params["show"].as_bool() {
            Some(true) => "Show stats for nerds".into(),
            Some(false) => "Hide stats for nerds".into(),
            None => "Show or hide stats for nerds".into(),
        },
        "player.audio_scrubbing" => match params["on"].as_bool() {
            Some(true) => "Audio scrubbing on".into(),
            Some(false) => "Audio scrubbing off".into(),
            None => "Audio scrubbing on or off".into(),
        },
        "player.loop" => match params["on"].as_bool() {
            Some(true) => "Loop playback on".into(),
            Some(false) => "Loop playback off".into(),
            None => "Loop playback on or off".into(),
        },
        "mixer.preview" => format!(
            "{} fader at {} (heard, not kept)",
            d.text("track"),
            db(&params["volume_db"])
        ),
        "pool.set" => format!("Media Pool: {}", compact(params)),
        "selection.set" if !params["edit"].is_null() => format!(
            "Select {} nearest {} on {}",
            edit_side(&params["side"]),
            d.time("edit"),
            params["track"].as_str().unwrap_or("V1")
        ),
        "selection.set" if !params["side"].is_null() => {
            format!("Select {}", edit_side(&params["side"]))
        }
        "selection.set" => {
            let clips = d.list("clips");
            let cues = d.list("cues");
            let media = d.list("media");
            match (clips.is_empty(), cues.is_empty(), media.is_empty()) {
                (true, true, true) => "Select nothing".into(),
                (false, _, _) => format!("Select {}", d.clips("clips")),
                (true, false, _) => format!("Select {}", d.cues("cues")),
                (true, true, false) => format!("Select {}", d.media("media")),
            }
        }
        "gui.show" => format!("Show {}", d.text("panel")),
        "gui.close" => format!("Close {}", d.text("panel")),
        "gui.target" => format!("Target track {}", d.text("track")),
        "gui.timeline" => format!("Timeline view: {}", compact(params)),
        "gui.screenshot" => "Screenshot".into(),
        "jobs.cancel" => format!("Cancel job {}", d.text("job")),
        _ => {
            let p = compact(params);
            if p.is_empty() {
                method.to_string()
            } else {
                format!("{method} {p}")
            }
        }
    }
}

fn edit_side(side: &Value) -> &'static str {
    match side.as_str() {
        Some("out") => "the Out of the edit",
        Some("in") => "the In of the edit",
        Some("audio") => "the audio edit",
        _ => "the edit",
    }
}

fn compact(params: &Value) -> String {
    let Some(o) = params.as_object() else {
        return String::new();
    };
    o.iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| match v {
            Value::String(s) => format!("{k} {s}"),
            other => format!("{k} {other}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn quoted(text: &str) -> String {
    let plain = crate::subtitles::plain(text).replace('\n', " ");
    let short: String = plain.chars().take(40).collect();
    if short.len() < plain.len() {
        format!("\u{201c}{}\u{2026}\u{201d}", short.trim_end())
    } else {
        format!("\u{201c}{short}\u{201d}")
    }
}

fn db(v: &Value) -> String {
    v.as_f64().map_or("?".into(), |d| format!("{d:+.1} dB"))
}

fn frames(n: i64) -> String {
    if n.abs() == 1 {
        format!("{n} frame")
    } else {
        format!("{n} frames")
    }
}

struct Words<'a> {
    params: &'a Value,
    project: Option<&'a Project>,
    playhead: Option<i64>,
}

impl Words<'_> {
    fn text(&self, key: &str) -> String {
        match &self.params[key] {
            Value::String(s) => s.clone(),
            Value::Null => "?".into(),
            other => other.to_string(),
        }
    }

    fn flag(&self, key: &str) -> bool {
        self.params[key] == true
    }

    fn list(&self, key: &str) -> Vec<String> {
        match &self.params[key] {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn time(&self, key: &str) -> String {
        let one = |v: &Value| match v {
            Value::Number(n) => match (n.as_i64(), self.project) {
                (Some(f), Some(p)) => p.settings.timecode(f),
                (Some(f), None) => format!("{f}f"),
                _ => n.to_string(),
            },
            Value::String(s) => match (TimeSpec::parse(s), self.project) {
                (Ok(t), Some(p)) => p
                    .time_context(self.playhead)
                    .resolve(&t)
                    .map_or_else(|_| self.unresolved(s), |f| p.settings.timecode(f)),
                _ => self.unresolved(s),
            },
            _ => match (self.playhead, self.project) {
                (Some(f), Some(p)) => p.settings.timecode(f),
                _ => "the playhead".into(),
            },
        };
        match &self.params[key] {
            Value::Array(a) if !a.is_empty() => a.iter().map(one).collect::<Vec<_>>().join(", "),
            v => one(v),
        }
    }

    fn length(&self, key: &str) -> String {
        let frames = match (&self.params[key], self.project) {
            (Value::Number(n), _) => n.as_i64(),
            (Value::String(s), Some(p)) => TimeSpec::parse(s)
                .ok()
                .and_then(|t| p.time_context(None).duration(&t).ok()),
            _ => None,
        };
        match (frames, self.project) {
            (Some(f), Some(p)) => p.settings.length_timecode(f),
            _ => self.text(key),
        }
    }

    fn range(&self, key: &str) -> String {
        match (&self.params[key], self.project) {
            (Value::String(s), Some(p)) => RangeSpec::parse(s)
                .ok()
                .and_then(|r| p.time_context(None).range(&r).ok())
                .map_or_else(
                    || s.clone(),
                    |(a, b)| {
                        format!(
                            "{}\u{2013}{}",
                            p.settings.timecode(a),
                            p.settings.timecode(b)
                        )
                    },
                ),
            (Value::String(s), None) => s.clone(),
            (Value::Null, _) => "the timeline marks".into(),
            (other, _) => other.to_string(),
        }
    }

    fn source_range(&self) -> Option<String> {
        let s = self.params["range"].as_str()?;
        let p = self.project?;
        let media = p.media(self.list("media").first()?)?;
        let (a, b) = crate::editor::media_context(p, media, None)
            .range(&RangeSpec::parse(s).ok()?)
            .ok()?;
        Some(format!(
            "{}\u{2013}{}",
            p.settings.length_timecode(a),
            p.settings.length_timecode(b)
        ))
    }

    fn media(&self, key: &str) -> String {
        let ids = self.list(key);
        if ids.is_empty() {
            return "?".into();
        }
        let name = |id: &String| {
            self.project
                .and_then(|p| p.media(id))
                .map_or_else(|| id.clone(), |m| m.name.clone())
        };
        match ids.len() {
            1..=3 => ids.iter().map(name).collect::<Vec<_>>().join(", "),
            n => format!("{} and {} more", name(&ids[0]), n - 1),
        }
    }

    fn media_or(&self, key: &str, all: &str) -> String {
        if self.list(key).is_empty() {
            all.into()
        } else {
            self.media(key)
        }
    }

    fn ids(&self, key: &str) -> Vec<String> {
        self.list(key)
            .into_iter()
            .map(|r| match self.project {
                Some(p) if r.contains('@') => crate::editor::resolve_clip(p, &r, None).unwrap_or(r),
                _ => r,
            })
            .collect()
    }

    fn clip_name(&self, id: &str) -> String {
        let Some(p) = self.project else {
            return id.to_string();
        };
        match p.clip(id) {
            Some(c) => match (&c.name, &c.title) {
                (Some(n), _) if !n.is_empty() => n.clone(),
                (_, Some(t)) => quoted(&t.label()),
                _ => p
                    .media(&c.media)
                    .map_or_else(|| id.to_string(), |m| m.name.clone()),
            },
            None => id.to_string(),
        }
    }

    fn names_of(&self, ids: &[String]) -> String {
        match ids {
            [] => "the selected clips".into(),
            [one] => self.clip_name(one),
            [a, b] if self.clip_name(a) != self.clip_name(b) => {
                format!("{} and {}", self.clip_name(a), self.clip_name(b))
            }
            _ => format!("{} clips", ids.len()),
        }
    }

    fn clips(&self, key: &str) -> String {
        self.names_of(&self.ids(key))
    }

    fn place(&self, key: &str, edge: Option<&str>) -> String {
        let ids = self.ids(key);
        let (Some(p), [id]) = (self.project, ids.as_slice()) else {
            return String::new();
        };
        let Some((t, c)) = p.find_clip(id) else {
            return String::new();
        };
        let clip = &p.tracks[t].clips[c];
        let at = match edge {
            Some("tail") => clip.end(),
            _ => clip.start,
        };
        format!(" at {} on {}", p.settings.timecode(at), p.tracks[t].id)
    }

    fn track_of(&self, key: &str) -> Option<String> {
        let p = self.project?;
        let ids = self.ids(key);
        let (t, _) = p.find_clip(ids.first()?)?;
        Some(format!(" on {}", p.tracks[t].id))
    }

    fn track_or_v1(&self) -> String {
        format!(" on {}", self.params["track"].as_str().unwrap_or("V1"))
    }

    fn unresolved(&self, s: &str) -> String {
        match TimeSpec::parse(s) {
            Ok(TimeSpec::Keyword(Keyword::Playhead)) => "the playhead".into(),
            _ => s.to_string(),
        }
    }

    fn frame(&self, v: &Value) -> Option<i64> {
        match v {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => self
                .project?
                .time_context(self.playhead)
                .resolve(&TimeSpec::parse(s).ok()?)
                .ok(),
            Value::Null => self.playhead,
            _ => None,
        }
    }

    fn split(&self) -> String {
        let at = self.time("at");
        let only = self.ids("clips");
        let frames: Option<Vec<i64>> = match &self.params["at"] {
            Value::Array(a) => a.iter().map(|v| self.frame(v)).collect(),
            v => self.frame(v).map(|f| vec![f]),
        };
        let (Some(p), Some(frames)) = (self.project, frames) else {
            return match only.as_slice() {
                [] => format!("Split at {at}"),
                ids => format!("Split {} at {at}", self.names_of(ids)),
            };
        };
        let cut: Vec<(String, &str)> = p
            .tracks
            .iter()
            .filter(|t| !only.is_empty() || !t.locked)
            .flat_map(|t| t.clips.iter().map(move |c| (c, t.id.as_str())))
            .filter(|(c, _)| only.is_empty() || only.contains(&c.id))
            .filter(|(c, _)| frames.iter().any(|&f| c.start < f && f < c.end()))
            .map(|(c, t)| (c.id.clone(), t))
            .collect();
        match cut.as_slice() {
            [] => format!("Split at {at}"),
            [(id, track)] => format!("Split {} at {at} on {track}", self.clip_name(id)),
            _ => {
                let ids: Vec<String> = cut.iter().map(|(id, _)| id.clone()).collect();
                format!("Split {} at {at}", self.names_of(&ids))
            }
        }
    }

    fn on_track(&self) -> Option<String> {
        self.params["track"].as_str().map(|t| format!(" on {t}"))
    }

    fn files(&self, key: &str) -> String {
        let paths = self.list(key);
        let name = |p: &String| {
            std::path::Path::new(p)
                .file_name()
                .map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned())
        };
        match paths.len() {
            0 => "?".into(),
            1..=3 => paths.iter().map(name).collect::<Vec<_>>().join(", "),
            n => format!("{} files", n),
        }
    }

    fn file(&self, key: &str) -> String {
        match self.params[key].as_str() {
            Some(p) => std::path::Path::new(p)
                .file_name()
                .map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned()),
            None => "?".into(),
        }
    }

    fn bin(&self, key: &str) -> String {
        match self.params[key].as_str() {
            Some("") | None => "Master".into(),
            Some(b) => b.to_string(),
        }
    }

    fn source(&self) -> String {
        let what = if self.flag("tape") {
            "the Source Tape span".to_string()
        } else {
            let range = match (self.source_range(), self.params["range"].as_str()) {
                (Some(r), _) => format!(" {r}"),
                (None, Some(r)) => format!(" {r}"),
                (None, None) => String::new(),
            };
            format!("{}{range}", self.media("media"))
        };
        let only = if self.flag("audio_only") {
            " (audio only)"
        } else if self.flag("video_only") {
            " (video only)"
        } else {
            ""
        };
        format!("{what}{only}{}", self.on_track().unwrap_or_default())
    }

    fn in_out(&self) -> String {
        let part = |key: &str, label: &str| match &self.params[key] {
            Value::Null => None,
            _ => Some(format!("{label} {}", self.time(key))),
        };
        let duration = match &self.params["duration"] {
            Value::Null => None,
            _ => Some(format!("Duration {}", self.marked_duration())),
        };
        let parts: Vec<String> = [part("in", "In"), part("out", "Out"), duration]
            .into_iter()
            .flatten()
            .collect();
        if parts.is_empty() {
            "unchanged".into()
        } else {
            parts.join(", ")
        }
    }

    fn marked_duration(&self) -> String {
        let spec = match &self.params["duration"] {
            Value::Number(n) => n.as_i64().map(|n| DurationSpec(TimeSpec::Frames(n))),
            Value::String(s) => DurationSpec::parse(s).ok(),
            _ => None,
        };
        let (Some(DurationSpec(spec)), Some(p)) = (spec, self.project) else {
            return self.text("duration");
        };
        match p.time_context(None).duration(&spec) {
            Ok(n) if spec.is_relative() && n > 0 => format!("+{}", frames(n)),
            Ok(n) if spec.is_relative() => frames(n),
            Ok(n) => p.settings.length_timecode(n),
            Err(_) => self.text("duration"),
        }
    }

    fn edge(&self) -> String {
        match self.params["edge"].as_str() {
            Some("head") => "start".into(),
            Some("tail") => "end".into(),
            Some(e) => e.to_string(),
            None => "edge".into(),
        }
    }

    fn by(&self) -> String {
        let n = match &self.params["by"] {
            Value::String(s) => match (crate::time::TimeSpec::parse(s), self.project) {
                (Ok(t), Some(p)) => p.time_context(None).duration(&t).ok(),
                _ => return format!("by {s}"),
            },
            v => v.as_i64(),
        };
        match n {
            Some(n) if n > 0 => format!("by +{}", frames(n)),
            Some(n) => format!("by {}", frames(n)),
            None => format!("to {}", self.time("to")),
        }
    }

    fn cues(&self, key: &str) -> String {
        let names: Vec<String> = self
            .list(key)
            .iter()
            .map(|r| {
                let cue = self.project.and_then(|p| {
                    let id = crate::editor::resolve_cue(p, r, self.playhead).ok()?;
                    p.cue(&id)
                });
                match cue {
                    Some(c) => quoted(&c.text),
                    None => r.clone(),
                }
            })
            .collect();
        match names.as_slice() {
            [] => "?".into(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} and {} more", rest.len()),
        }
    }

    fn named(&self) -> String {
        match self.params["name"].as_str() {
            Some(n) if !n.is_empty() => format!(" \u{201c}{n}\u{201d}"),
            _ => String::new(),
        }
    }

    fn viewer(&self) -> String {
        match self.params["mode"].as_str() {
            Some("timeline") => "Timeline".into(),
            Some("source_clip") => "Source Clip".into(),
            Some("source_tape") => "Source Tape".into(),
            Some(m) => m.to_string(),
            None => "?".into(),
        }
    }

    fn settings(&self) -> String {
        let p = self.params;
        let mut parts = Vec::new();
        if let Some(on) = p["use_proxies"].as_bool() {
            parts.push(if on { "proxies on" } else { "proxies off" }.to_string());
        }
        if let Some(on) = p["ripple_on"].as_bool() {
            parts.push(if on { "Ripple On" } else { "Ripple Off" }.to_string());
        }
        for key in [
            "fps",
            "width",
            "height",
            "sample_rate",
            "channels",
            "start_tc",
            "still_duration",
        ] {
            if !p[key].is_null() {
                parts.push(format!("{} {}", key.replace('_', " "), self.text(key)));
            }
        }
        for (language, words) in p["fillers"].as_object().into_iter().flatten() {
            let language = language.trim().to_lowercase();
            parts.push(match words.is_null() {
                true => format!("{language} filler words reset"),
                false => format!("{language} filler words"),
            });
        }
        if parts.is_empty() {
            "unchanged".into()
        } else {
            parts.join(", ")
        }
    }

    fn track_settings(&self) -> String {
        let p = self.params;
        let mut parts = Vec::new();
        let on_off = |key: &str, on: &str, off: &str| {
            p[key]
                .as_bool()
                .map(|b| if b { on } else { off }.to_string())
        };
        parts.extend(on_off("locked", "locked", "unlocked"));
        parts.extend(on_off("muted", "audio off", "audio on"));
        parts.extend(on_off("solo", "solo on", "solo off"));
        parts.extend(on_off("video_enabled", "video on", "video off"));
        parts.extend(on_off("shown", "shown", "hidden"));
        match p["language"].as_str() {
            Some("") => parts.push("no language".to_string()),
            Some(l) => parts.push(format!("language {l}")),
            None => {}
        }
        if !p["volume_db"].is_null() {
            parts.push(format!("volume {}", db(&p["volume_db"])));
        }
        match p.get("duck") {
            Some(Value::Null) => parts.push("no ducking".to_string()),
            Some(Value::Object(d)) => {
                let before = parts.len();
                match d.get("on").and_then(Value::as_bool) {
                    Some(false) => parts.push("ducking off".to_string()),
                    Some(true) => parts.push("ducking on".to_string()),
                    None => {}
                }
                if let Some(s) = d.get("sources").and_then(Value::as_array) {
                    let names: Vec<&str> = s.iter().filter_map(Value::as_str).collect();
                    parts.push(match names.as_slice() {
                        [] => "ducked under nothing".to_string(),
                        _ => format!("ducked under {}", names.join(", ")),
                    });
                }
                if let Some(v) = d.get("amount_db").and_then(Value::as_f64) {
                    parts.push(format!("ducking by {v:.1} dB"));
                }
                let more = [
                    "lookahead_ms",
                    "rise_ms",
                    "hold_ms",
                    "recovery_ms",
                    "detect",
                    "threshold_db",
                ];
                if more.iter().any(|k| d.contains_key(*k)) {
                    parts.push("ducking settings".to_string());
                }
                if parts.len() == before {
                    parts.push("ducking on".to_string());
                }
            }
            _ => {}
        }
        if let Some(n) = p["name"].as_str() {
            parts.push(format!("named \u{201c}{n}\u{201d}"));
        }
        if p["style"].is_object() {
            parts.push("style".to_string());
        }
        if let Some(l) = p["look"].as_str() {
            parts.push(format!("look {l}"));
        }
        match p["color"].as_str() {
            Some("") => parts.push("no color".to_string()),
            Some(c) => parts.push(c.to_string()),
            None => {}
        }
        if parts.is_empty() {
            "unchanged".into()
        } else {
            parts.join(", ")
        }
    }

    fn clip_settings(&self) -> String {
        let p = self.params;
        let mut parts = Vec::new();
        if let Some(b) = p["enabled"].as_bool() {
            parts.push(if b { "enabled" } else { "disabled" }.to_string());
        }
        if let Some(b) = p["muted"].as_bool() {
            parts.push(if b { "muted" } else { "unmuted" }.to_string());
        }
        if !p["gain_db"].is_null() {
            parts.push(format!("gain {}", db(&p["gain_db"])));
        }
        if let Some(n) = p["name"].as_str() {
            parts.push(format!("named \u{201c}{n}\u{201d}"));
        }
        if parts.is_empty() {
            "unchanged".into()
        } else {
            parts.join(", ")
        }
    }
}
