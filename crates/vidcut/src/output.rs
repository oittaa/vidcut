//! Human-readable output. With `--json` the response object is printed as is.

use serde_json::Value;
use vidcut_core::Response;

/// Prints a response for `method`.
pub fn print(method: &str, r: &Response, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
        return;
    }
    for w in &r.warnings {
        eprintln!("vidcut: warning: {w}");
    }
    if let Some(()) = live(method, r) {
        return;
    }
    let mutation = matches!(
        method.split('.').next(),
        Some(
            "edit"
                | "clip"
                | "marks"
                | "markers"
                | "history"
                | "apply"
                | "media"
                | "project"
                | "track"
                | "bins"
        )
    ) && !matches!(
        method,
        "media.list" | "media.get" | "project.get" | "project.summary" | "history.get"
    );
    if mutation {
        if r.dry_run {
            println!(
                "dry run: {method} would make these changes (project stays at rev {})",
                r.rev
            );
        } else {
            println!("ok {method} (rev {})", r.rev);
        }
        for (label, ids) in [
            ("created", &r.changes.created),
            ("modified", &r.changes.modified),
            ("deleted", &r.changes.deleted),
        ] {
            if !ids.is_empty() {
                println!("  {label:<9}{}", ids.join(" "));
            }
        }
        if let Some(media) = r.result.get("media").and_then(Value::as_array) {
            let ids: Vec<&str> = media.iter().filter_map(Value::as_str).collect();
            println!("  media    {}", ids.join(" "));
        }
        if let Some(tracks) = r.result.get("tracks").and_then(Value::as_array) {
            let ids: Vec<&str> = tracks.iter().filter_map(Value::as_str).collect();
            println!("  tracks   {}", ids.join(" "));
        }
        if let Some(track) = r.result.get("track").and_then(Value::as_str) {
            println!("  on       {track}");
        }
        if let Some(removed) = r.result.get("removed").and_then(Value::as_array) {
            println!(
                "  removed  {} frames in {} parts",
                r.result["frames"],
                removed.len()
            );
            for part in removed {
                println!(
                    "    {} -> {}  {}",
                    part["start_tc"].as_str().unwrap_or_default(),
                    part["end_tc"].as_str().unwrap_or_default(),
                    part["clip"].as_str().unwrap_or_default()
                );
            }
        }
        if let Some(cuts) = r.result.get("cuts").and_then(Value::as_array) {
            let tcs: Vec<&str> = cuts.iter().filter_map(|c| c["tc"].as_str()).collect();
            println!(
                "  cuts     {}",
                if tcs.is_empty() {
                    "none".into()
                } else {
                    tcs.join(" ")
                }
            );
        }
        if let Some(undone) = r
            .result
            .get("undone")
            .or_else(|| r.result.get("redone"))
            .and_then(Value::as_array)
        {
            for l in undone {
                println!("  {}", l.as_str().unwrap_or_default());
            }
        }
        return;
    }
    match method {
        "timeline.import" => {
            let n = |k: &str| r.result[k].as_array().map_or(0, Vec::len);
            let verb = if r.dry_run {
                "would import"
            } else {
                "imported"
            };
            println!(
                "{verb} {} clip(s), {} media, {} marker(s) from {:?} at {} fps (rev {})",
                n("clips"),
                n("media"),
                n("markers"),
                r.result["name"].as_str().unwrap_or_default(),
                r.result["fps"].as_str().unwrap_or_default(),
                r.rev
            );
            for w in r.result["warnings"].as_array().into_iter().flatten() {
                println!("  warning: {}", w.as_str().unwrap_or_default());
            }
        }
        "export.start" if r.result.get("id").is_some() => {
            println!("{}", job_line(&r.result));
        }
        "export.start" => {
            for o in r.result["outputs"].as_array().into_iter().flatten() {
                if o["format"] == "otio" {
                    println!(
                        "wrote {} ({} clips, {} frames)",
                        o["path"].as_str().unwrap_or_default(),
                        o["clips"],
                        o["frames"]
                    );
                    continue;
                }
                let enc: Vec<&str> = [&o["video_encoder"], &o["audio_encoder"]]
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect();
                println!(
                    "exported {} ({} frames) with {} in {:.1} s",
                    o["output"].as_str().unwrap_or_default(),
                    o["frames"],
                    enc.join(" + "),
                    o["seconds"].as_f64().unwrap_or(0.0)
                );
            }
        }
        "export.plan" if r.result["format"] == "otio" => {
            for o in r.result["outputs"].as_array().into_iter().flatten() {
                println!(
                    "otio  {}  frames {}..{} ({} clips)",
                    o["path"].as_str().unwrap_or_default(),
                    o["range"][0],
                    o["range"][1],
                    o["clips"]
                );
            }
        }
        "export.plan" => {
            let s = &r.result["settings"];
            println!(
                "preset {}  container {}  video {}  audio {}",
                r.result["preset"].as_str().unwrap_or("custom"),
                s["container"].as_str().unwrap_or_default(),
                match &r.result["video_encoder"] {
                    Value::Null => "none".to_string(),
                    v if v.get("error").is_some() =>
                        format!("unavailable: {}", v["error"].as_str().unwrap_or_default()),
                    v => format!(
                        "{} ({})",
                        v["name"].as_str().unwrap_or_default(),
                        v["pixel"].as_str().unwrap_or_default()
                    ),
                },
                s["audio"]["codec"].as_str().unwrap_or("none")
            );
            for o in r.result["outputs"].as_array().into_iter().flatten() {
                println!(
                    "  {}  frames {}..{} ({} frames, {} video pieces, {} audio lanes)",
                    o["path"].as_str().unwrap_or_default(),
                    o["range"][0],
                    o["range"][1],
                    o["frames"],
                    o["video_pieces"],
                    o["audio_lanes"]
                );
            }
        }
        "analyze.silence" | "analyze.black" | "analyze.scenes" => {
            for target in r.result.as_array().into_iter().flatten() {
                let what = match target["clip"].as_str() {
                    Some(c) => format!("{c} ({})", target["track"].as_str().unwrap_or("?")),
                    None => target["media"].as_str().unwrap_or_default().to_string(),
                };
                println!("{what}");
                let items = target
                    .get("ranges")
                    .or_else(|| target.get("cuts"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if items.is_empty() {
                    println!("  none");
                }
                for i in items {
                    match i.get("score") {
                        Some(score) => println!(
                            "  {}  score {:.1}",
                            i["tc"].as_str().unwrap_or_default(),
                            score.as_f64().unwrap_or(0.0)
                        ),
                        None => println!(
                            "  {} -> {}  ({})",
                            i["start_tc"].as_str().unwrap_or_default(),
                            i["end_tc"].as_str().unwrap_or_default(),
                            i["duration_tc"].as_str().unwrap_or_default()
                        ),
                    }
                }
            }
        }
        "analyze.loudness" => {
            let v = &r.result;
            let db = |k: &str, unit: &str| match v[k].as_f64() {
                Some(x) => format!("{x:.1} {unit}"),
                None => "silent".into(),
            };
            println!(
                "{} integrated, range {:.1} LU, true peak {} ({} -> {})",
                db("integrated", "LUFS"),
                v["range"].as_f64().unwrap_or(0.0),
                db("true_peak", "dBTP"),
                v["from_tc"].as_str().unwrap_or_default(),
                v["to_tc"].as_str().unwrap_or_default()
            );
        }
        "timeline.boring" => {
            for (title, key) in [("boring", "boring"), ("jump cuts", "jump")] {
                let clips = r.result[key].as_array().cloned().unwrap_or_default();
                println!(
                    "{title}: {}",
                    if clips.is_empty() {
                        "none".into()
                    } else {
                        clips.len().to_string()
                    }
                );
                for c in clips {
                    println!(
                        "  {:<5} {}  {}",
                        c["clip"].as_str().unwrap_or_default(),
                        c["start_tc"].as_str().unwrap_or_default(),
                        c["duration_tc"].as_str().unwrap_or_default()
                    );
                }
            }
        }
        "storyboard.render" => println!(
            "wrote {} ({} pictures, {}x{})",
            r.result["path"].as_str().unwrap_or_default(),
            r.result["cells"].as_array().map_or(0, Vec::len),
            r.result["width"],
            r.result["height"]
        ),
        "frame.render" => println!(
            "wrote {} (frame {}, {}x{})",
            r.result["path"].as_str().unwrap_or_default(),
            r.result["frame"],
            r.result["width"],
            r.result["height"]
        ),
        "presets.list" => {
            for p in r.result.as_array().into_iter().flatten() {
                println!(
                    "{:<14} {:<4} {:<16} {}{}",
                    p["name"].as_str().unwrap_or_default(),
                    p["container"].as_str().unwrap_or_default(),
                    p["title"].as_str().unwrap_or_default(),
                    p["description"].as_str().unwrap_or_default(),
                    if p["quick_export"] == true {
                        "  [Quick Export]"
                    } else {
                        ""
                    }
                );
            }
        }
        "presets.save" => println!(
            "saved preset {} ({})",
            r.result["preset"]["name"].as_str().unwrap_or_default(),
            r.result["path"].as_str().unwrap_or_default()
        ),
        "presets.delete" => println!(
            "deleted preset {}",
            r.result["deleted"].as_str().unwrap_or_default()
        ),
        "preferences.get" | "preferences.set" => {
            let mut lines = Vec::new();
            flatten("", &r.result["preferences"], &mut lines);
            for line in lines {
                println!("{line}");
            }
            let c = &r.result["thumbnail_cache"];
            println!(
                "thumbnail cache: {} MB in {} files ({})",
                c["mb"],
                c["files"],
                c["path"].as_str().unwrap_or_default()
            );
            println!("({})", r.result["path"].as_str().unwrap_or_default());
        }
        "encoders.list" => {
            for e in r.result["encoders"].as_array().into_iter().flatten() {
                println!(
                    "{:<11} {:<8} {:<20} {}",
                    e["encoder"].as_str().unwrap_or_default(),
                    e["pixel"].as_str().unwrap_or_default(),
                    e["device"].as_str().unwrap_or("-"),
                    if e["ok"] == true {
                        "ok".to_string()
                    } else {
                        format!("no ({})", e["error"].as_str().unwrap_or_default())
                    }
                );
            }
        }
        "proxy.generate" => {
            let proxies = r.result["proxies"].as_array().cloned().unwrap_or_default();
            let verb = if r.dry_run { "would write" } else { "proxy" };
            if proxies.is_empty() {
                println!("no video to proxy (rev {})", r.rev);
            }
            for p in proxies {
                println!(
                    "{verb} {}  {}  {}x{} (rev {})",
                    p["media"].as_str().unwrap_or_default(),
                    p["path"].as_str().unwrap_or_default(),
                    p["width"],
                    p["height"],
                    r.rev
                );
            }
        }
        "proxy.remove" => {
            let cleared = r.result["cleared"].as_array().cloned().unwrap_or_default();
            let ids: Vec<&str> = cleared.iter().filter_map(|v| v.as_str()).collect();
            let verb = if r.dry_run { "would remove" } else { "removed" };
            println!(
                "{verb} {} proxy file(s){} (rev {})",
                ids.len(),
                if ids.is_empty() {
                    String::new()
                } else {
                    format!(": {}", ids.join(" "))
                },
                r.rev
            );
        }
        "timeline.get" => timeline(&r.result),
        "media.list" => media_list(&r.result),
        "project.summary" => summary(&r.result),
        "timeline.at" => {
            println!(
                "{} ({})",
                r.result["at_tc"].as_str().unwrap_or_default(),
                r.result["at"]
            );
            for c in r.result["clips"].as_array().into_iter().flatten() {
                println!(
                    "  {} {}  source frame {}",
                    c["track"].as_str().unwrap_or_default(),
                    clip_line(c),
                    c["source_frame"]
                );
            }
        }
        "timeline.edit_points" => {
            for p in r.result.as_array().into_iter().flatten() {
                println!("{}  {}", p["tc"].as_str().unwrap_or_default(), p["frame"]);
            }
        }
        "history.get" => {
            for (title, key) in [("undo (newest first)", "undo"), ("redo", "redo")] {
                let items = r.result[key].as_array().cloned().unwrap_or_default();
                println!(
                    "{title}: {}",
                    if items.is_empty() {
                        "-".into()
                    } else {
                        items.len().to_string()
                    }
                );
                for l in items {
                    println!("  {}", l.as_str().unwrap_or_default());
                }
            }
        }
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&r.result).unwrap_or_default()
        ),
    }
}

/// Live (GUI) requests; `None` for everything else.
fn live(method: &str, r: &Response) -> Option<()> {
    let v = &r.result;
    match method {
        "player.state" | "player.play" | "player.pause" | "player.toggle" | "player.seek"
        | "player.step" | "player.mute" | "viewer.set" | "viewer.load" | "project.open" => {
            println!("{}", state_line(v));
        }
        "gui.screenshot" => println!(
            "wrote {} ({}x{})",
            v["path"].as_str().unwrap_or_default(),
            v["width"],
            v["height"]
        ),
        "gui.log" => {
            for e in v.as_array().into_iter().flatten() {
                let params = match &e["params"] {
                    p if p.as_object().is_some_and(|o| o.is_empty()) => String::new(),
                    p => p.to_string(),
                };
                let status = match e["error"].as_str() {
                    Some(err) => format!("  ERROR {err}"),
                    None => String::new(),
                };
                println!(
                    "#{:<5} {:<7} {} {}{}",
                    e["seq"],
                    e["origin"].as_str().unwrap_or_default(),
                    e["method"].as_str().unwrap_or_default(),
                    params,
                    status
                );
            }
        }
        "selection.get" | "selection.set" => {
            let ids = |k: &str| {
                v[k].as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "-".into())
            };
            println!("clips {}  media {}", ids("clips"), ids("media"));
        }
        "edit.copy" => println!(
            "copied {} clip(s); paste with `vidcut edit paste` (GUI) or --clips",
            v["clips"].as_array().map_or(0, Vec::len)
        ),
        "gui.quit" | "gui.window" | "gui.show" => println!("ok {method}"),
        "pool.set" => println!(
            "pool: bin {:?}, sorted by {}{}",
            v["bin"].as_str().unwrap_or_default(),
            v["sort"].as_str().unwrap_or_default(),
            if v["descending"] == true {
                " (descending)"
            } else {
                ""
            }
        ),
        "jobs.list" => {
            let jobs = v.as_array().cloned().unwrap_or_default();
            if jobs.is_empty() {
                println!("no jobs");
            }
            for j in jobs {
                println!("{}", job_line(&j));
            }
        }
        "jobs.get" | "jobs.wait" | "jobs.cancel" => println!("{}", job_line(v)),
        _ => return None,
    }
    Some(())
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// `j1  running    42%  ETA 0:12  out.mp4`
fn job_line(j: &Value) -> String {
    let state = j["state"].as_str().unwrap_or_default();
    let time = match (state, j["eta"].as_f64()) {
        ("running", Some(eta)) => format!("ETA {}", clock(eta)),
        ("running", None) => "starting".into(),
        _ => format!("took {}", clock(j["elapsed"].as_f64().unwrap_or(0.0))),
    };
    let error = j["error"]["message"]
        .as_str()
        .map(|e| format!("  ({e})"))
        .unwrap_or_default();
    format!(
        "{:<4} {:<9} {:>3}%  {:<10} {}{error}",
        j["id"].as_str().unwrap_or_default(),
        state,
        (j["progress"].as_f64().unwrap_or(0.0) * 100.0).floor(),
        time,
        j["title"].as_str().unwrap_or_default()
    )
}

/// `viewer timeline | timeline 00:00:10:00 (300/450) stopped | source ...`
fn state_line(v: &Value) -> String {
    let player = |p: &Value| {
        let what = if p["playing"].as_bool() == Some(true) {
            match p["speed"].as_f64() {
                Some(s) if s != 1.0 => format!("playing {s:+}x"),
                _ => "playing".to_string(),
            }
        } else {
            "stopped".to_string()
        };
        let mut s = format!(
            "{} ({}/{}) {what}",
            p["position_tc"].as_str().unwrap_or("-"),
            p["position"],
            p["frames"]
        );
        if let Some(e) = p["error"].as_str() {
            s.push_str(&format!(" ERROR {e}"));
        }
        s
    };
    let source = &v["source"];
    let source_text = match source["media"].as_str() {
        Some(m) => format!(
            "{} {m} @{}",
            source["content"].as_str().unwrap_or("source"),
            source["source_frame"]
        ),
        None => "source -".to_string(),
    };
    format!(
        "viewer {} | timeline {} | {} {}",
        v["viewer"].as_str().unwrap_or("-"),
        player(&v["timeline"]),
        source_text,
        if source["loaded"].as_bool() == Some(true) {
            player(source)
        } else {
            String::new()
        }
    )
    .trim_end()
    .to_string()
}

fn clip_line(c: &Value) -> String {
    let s = |k: &str| c[k].as_str().unwrap_or_default().to_string();
    let audio = match &c["audio"] {
        Value::Null => "no audio".to_string(),
        a if a["head"] != 0 || a["tail"] != 0 => format!("audio {:+}/{:+}", a["head"], a["tail"]),
        _ => "audio".to_string(),
    };
    format!(
        "{:<5} {} -> {} {:>6}f  {}  [{} src {}..{}, {}{}]",
        s("id"),
        s("start_tc"),
        s("end_tc"),
        c["duration"],
        s("name"),
        s("media"),
        c["source_in"],
        c["source_out"],
        audio,
        if c["enabled"] == false {
            ", disabled"
        } else {
            ""
        }
    )
}

fn timeline(t: &Value) {
    println!(
        "duration {} ({} frames at {} fps)",
        t["duration_tc"].as_str().unwrap_or_default(),
        t["duration"],
        t["fps"].as_str().unwrap_or_default()
    );
    for track in t["tracks"].as_array().into_iter().flatten() {
        println!("{}", track["id"].as_str().unwrap_or_default());
        let items = track["items"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            println!("  (empty)");
        }
        for item in items {
            if item["type"] == "gap" {
                println!(
                    "  {:<5} {} -> {} {:>6}f  gap",
                    "--",
                    item["start_tc"].as_str().unwrap_or_default(),
                    item["end_tc"].as_str().unwrap_or_default(),
                    item["duration"]
                );
            } else {
                println!("  {}", clip_line(&item));
            }
        }
    }
    let marks = &t["marks"];
    if marks.get("in").is_some() || marks.get("out").is_some() {
        println!(
            "marks in {} out {}",
            marks.get("in").unwrap_or(&Value::Null),
            marks.get("out").unwrap_or(&Value::Null)
        );
    }
    for k in t["markers"].as_array().into_iter().flatten() {
        println!(
            "marker {} at {} {}",
            k["id"].as_str().unwrap_or_default(),
            k["at"],
            k["name"].as_str().unwrap_or_default()
        );
    }
}

fn media_list(m: &Value) {
    let items = m.as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("(media pool is empty)");
    }
    for i in items {
        let video = match &i["video"] {
            Value::Null => "audio only".to_string(),
            v => format!(
                "{}x{} {} fps {}",
                v["width"],
                v["height"],
                v["fps"].as_str().unwrap_or_default(),
                v["codec"].as_str().unwrap_or_default()
            ),
        };
        let bin = match i["bin"].as_str() {
            Some(b) if !b.is_empty() => format!("  [{b}]"),
            _ => String::new(),
        };
        let proxy = match i["proxy"]["stale"].as_bool() {
            Some(false) => "  proxy",
            Some(true) => "  proxy (stale)",
            None => "",
        };
        println!(
            "{:<5} {:<30} {} ({} frames)  {}, {} audio{bin}{proxy}",
            i["id"].as_str().unwrap_or_default(),
            i["name"].as_str().unwrap_or_default(),
            i["duration_tc"].as_str().unwrap_or_default(),
            i["frames"],
            video,
            i["audio_streams"]
        );
    }
}

fn summary(s: &Value) {
    let st = &s["settings"];
    println!(
        "rev {}  {}x{} at {} fps, {} Hz {} ch, ripple {}",
        s["rev"],
        st["width"],
        st["height"],
        st["fps"].as_str().unwrap_or_default(),
        st["sample_rate"],
        st["channels"],
        if st["ripple_on"] == true { "on" } else { "off" }
    );
    if st["auto_format"] == true {
        println!("format: taken from the first video added");
    }
    println!(
        "duration {} ({} frames); {} media, {} clips, {} markers; tracks {}",
        s["duration_tc"].as_str().unwrap_or_default(),
        s["duration"],
        s["media"],
        s["clips"],
        s["markers"],
        s["tracks"]
            .as_array()
            .map(|t| t
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "))
            .unwrap_or_default()
    );
}

/// `group.key = value` lines of a JSON object.
fn flatten(prefix: &str, v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        v => out.push(format!("{prefix} = {v}")),
    }
}
