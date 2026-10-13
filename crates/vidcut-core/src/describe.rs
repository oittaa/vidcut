//! A request in words, for people: the GUI's command log, the undo history
//! and `vidcut gui log`. Times read as timecodes when the project is known,
//! media by their names.

use serde_json::Value;

use crate::model::Project;

/// `method` with `params` (a request's JSON) in words. `project`, as it was
/// before the request, names media and turns frames into timecodes.
pub fn describe(method: &str, params: &Value, project: Option<&Project>) -> String {
    let d = Words { params, project };
    match method {
        "project.set" => format!("Project: {}", d.settings()),
        "project.open" => format!("Open {}", d.file("path")),
        "project.create" => format!("Create {}", d.file("path")),
        "media.add" => format!("Import {}", d.files("paths")),
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
        "timeline.import" if d.flag("replace") => {
            format!("Import timeline {}, replacing the timeline", d.file("path"))
        }
        "timeline.import" => format!("Import timeline {}", d.file("path")),
        "edit.append" => format!("Append {}", d.source()),
        "edit.smart_insert" => format!("Smart Insert {}", d.source()),
        "edit.insert" => format!("Insert {} at {}", d.source(), d.time("at")),
        "edit.overwrite" => format!("Overwrite {} at {}", d.source(), d.time("at")),
        "edit.ripple_overwrite" => format!("Ripple Overwrite {}", d.source()),
        "edit.place_on_top" => format!("Place On Top {} at {}", d.source(), d.time("at")),
        "edit.split" => format!("Split at {}{}", d.time("at"), d.split_clips()),
        "edit.delete" if d.flag("lift") => format!("Delete {}, leaving a gap", d.clips("clips")),
        "edit.delete" => format!("Delete {}", d.clips("clips")),
        "edit.delete_range" => format!("Delete {}", d.range("range")),
        "edit.close_gap" => format!("Close the gap at {}", d.time("at")),
        "edit.move" => format!(
            "Move {} to {}{}",
            d.clips("clips"),
            d.time("to"),
            d.on_track()
        ),
        "edit.replace" => format!("Replace {} with {}", d.clips("onto"), d.clips("clip")),
        "edit.trim" => format!("Trim the {} of {} {}", d.edge(), d.clips("clip"), d.by()),
        "edit.roll" => format!(
            "Roll the cut at the {} of {} {}",
            d.edge(),
            d.clips("clip"),
            d.by()
        ),
        "edit.slip" => format!("Slip {} {}", d.clips("clip"), d.by()),
        "edit.decompose" => format!("Decompose {}", d.clips("clips")),
        "edit.slide" => format!("Slide {} {}", d.clips("clip"), d.by()),
        "edit.audio_roll" => format!(
            "Move the audio cut at the {} of {} {}",
            d.edge(),
            d.clips("clip"),
            d.by()
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
            d.time("duration")
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
        "export.start" => format!("Export {}", d.file("output")),
        "export.plan" => format!("Plan an export to {}", d.file("output")),
        "frame.render" => format!("Render the frame at {}", d.time("at")),
        "storyboard.render" => format!("Storyboard {}", d.file("out")),
        "presets.save" => format!("Save preset {}", d.text("name")),
        "presets.delete" => format!("Delete preset {}", d.text("name")),
        "preferences.set" => format!("Preferences: {}", compact(params)),
        "history.undo" => "Undo".into(),
        "history.redo" => "Redo".into(),
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
        "player.audio_scrubbing" => match params["on"].as_bool() {
            Some(true) => "Audio scrubbing on".into(),
            Some(false) => "Audio scrubbing off".into(),
            None => "Audio scrubbing on or off".into(),
        },
        "mixer.preview" => format!(
            "{} fader at {} (heard, not kept)",
            d.text("track"),
            db(&params["volume_db"])
        ),
        "pool.set" => format!("Media Pool: {}", compact(params)),
        "selection.set" => {
            let clips = d.list("clips");
            let media = d.list("media");
            match (clips.is_empty(), media.is_empty()) {
                (true, true) => "Select nothing".into(),
                (false, _) => format!("Select {}", d.clips("clips")),
                (true, false) => format!("Select {}", d.media("media")),
            }
        }
        "gui.show" => format!("Show {}", d.text("panel")),
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

/// The parameters as `key value` pairs, nulls left out.
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

fn db(v: &Value) -> String {
    v.as_f64().map_or("?".into(), |d| format!("{d:+.1} dB"))
}

/// `n` frames, in words: "1 frame", "-2 frames".
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

    /// A string or a list of them.
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

    /// A time: frames as a timecode, a spec (`10s`, `end`) as written,
    /// several joined; none is the playhead.
    fn time(&self, key: &str) -> String {
        let one = |v: &Value| match v {
            Value::Number(n) => match (n.as_i64(), self.project) {
                (Some(f), Some(p)) => p.settings.timecode(f),
                (Some(f), None) => format!("{f}f"),
                _ => n.to_string(),
            },
            Value::String(s) => s.clone(),
            _ => "the playhead".into(),
        };
        match &self.params[key] {
            Value::Array(a) if !a.is_empty() => a.iter().map(one).collect::<Vec<_>>().join(", "),
            v => one(v),
        }
    }

    fn range(&self, key: &str) -> String {
        match &self.params[key] {
            Value::String(s) => s.clone(),
            Value::Null => "the timeline marks".into(),
            other => other.to_string(),
        }
    }

    /// Media ids by name.
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

    fn clips(&self, key: &str) -> String {
        let ids = self.list(key);
        match ids.len() {
            0 => "the selected clips".into(),
            1..=4 => ids.join(", "),
            n => format!("{} clips", n),
        }
    }

    fn of_clips(&self) -> String {
        if self.list("clips").is_empty() {
            String::new()
        } else {
            format!(" in {}", self.clips("clips"))
        }
    }

    /// The selected clips a split cuts. A split cuts every clip under the
    /// point when none of the selected ones is, so a selected clip is named
    /// only when it lies across a split frame (before the split) or ends at
    /// one (after it, keeping its id).
    fn split_clips(&self) -> String {
        let frames: Option<Vec<i64>> = match &self.params["at"] {
            Value::Array(a) => a.iter().map(Value::as_i64).collect(),
            v => v.as_i64().map(|f| vec![f]),
        };
        let (Some(p), Some(frames)) = (self.project, frames) else {
            return self.of_clips();
        };
        let cut: Vec<String> = self
            .list("clips")
            .into_iter()
            .filter(|id| {
                p.clip(id)
                    .is_some_and(|c| frames.iter().any(|&f| c.start < f && f <= c.end()))
            })
            .collect();
        match cut.len() {
            0 => String::new(),
            1..=4 => format!(" in {}", cut.join(", ")),
            n => format!(" in {n} clips"),
        }
    }

    fn on_track(&self) -> String {
        match self.params["track"].as_str() {
            Some(t) => format!(" on {t}"),
            None => String::new(),
        }
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

    /// What an edit function takes: a media item and its range, or the
    /// Source Tape span; and the track it goes to.
    fn source(&self) -> String {
        let what = if self.flag("tape") {
            "the Source Tape span".to_string()
        } else {
            let range = match self.params["range"].as_str() {
                Some(r) => format!(" {r}"),
                None => String::new(),
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
        format!("{what}{only}{}", self.on_track())
    }

    fn in_out(&self) -> String {
        let part = |key: &str, label: &str| match &self.params[key] {
            Value::Null => None,
            _ => Some(format!("{label} {}", self.time(key))),
        };
        let parts: Vec<String> = [part("in", "In"), part("out", "Out")]
            .into_iter()
            .flatten()
            .collect();
        if parts.is_empty() {
            "unchanged".into()
        } else {
            parts.join(", ")
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
        match self.params["by"].as_i64() {
            Some(n) if n > 0 => format!("by +{}", frames(n)),
            Some(n) => format!("by {}", frames(n)),
            None => format!("to {}", self.time("to")),
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
        if !p["volume_db"].is_null() {
            parts.push(format!("volume {}", db(&p["volume_db"])));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;
    use serde_json::json;

    #[test]
    fn requests_read_as_words() {
        let mut f = Fixture::new(&[(10.0, true)]);
        f.append("m1", 0, 100);
        let p = f.editor.project();
        let name = p.media("m1").unwrap().name.clone();
        let say = |m: &str, params: Value| describe(m, &params, Some(p));
        assert_eq!(
            say("edit.split", json!({"at": [157], "clips": []})),
            "Split at 00:00:06:07"
        );
        let c = p.tracks[0].clips[0].id.clone();
        assert_eq!(
            say("edit.split", json!({"at": [40], "clips": [c]})),
            format!("Split at 00:00:01:15 in {c}")
        );
        // A split past the selected clip cuts whatever is under the point.
        assert_eq!(
            say("edit.split", json!({"at": [140], "clips": [c]})),
            "Split at 00:00:05:15"
        );
        assert_eq!(
            say("edit.delete", json!({"clips": ["c14"], "lift": false})),
            "Delete c14"
        );
        assert_eq!(
            say("edit.delete", json!({"clips": ["c14"], "lift": true})),
            "Delete c14, leaving a gap"
        );
        assert_eq!(
            say(
                "track.set",
                json!({"track": "V1", "video_enabled": false, "locked": null})
            ),
            "V1: video off"
        );
        assert_eq!(
            say("track.set", json!({"track": "A1", "volume_db": -6.0})),
            "A1: volume -6.0 dB"
        );
        assert_eq!(
            say(
                "edit.place_on_top",
                json!({"media": "m1", "range": "0f..147f", "at": 157})
            ),
            format!("Place On Top {name} 0f..147f at 00:00:06:07")
        );
        assert_eq!(
            say("markers.add", json!({"at": 157, "name": ""})),
            "Marker at 00:00:06:07"
        );
        assert_eq!(
            say("viewer.set", json!({"mode": "timeline"})),
            "Viewer: Timeline"
        );
        assert_eq!(
            say("media.add", json!({"paths": ["/a/b/cam.mp4"]})),
            "Import cam.mp4"
        );
        // Unknown ones still say what they are.
        assert_eq!(say("doctor", json!({})), "doctor");
        // Without a project, frames stay frames.
        assert_eq!(
            describe("player.seek", &json!({"at": 30}), None),
            "Go to 30f"
        );
    }
}
