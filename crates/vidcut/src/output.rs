//! Human-readable output. With `--json` the response object is printed as is.

use serde_json::Value;
use vidcut_core::Response;

/// Prints a response for `method`.
pub fn print(method: &str, r: &Response, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
        return;
    }
    if let Some(()) = live(method, r) {
        return;
    }
    let mutation = matches!(
        method.split('.').next(),
        Some("edit" | "clip" | "marks" | "markers" | "history" | "apply" | "media" | "project")
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
        "export.start" => {
            for o in r.result["outputs"].as_array().into_iter().flatten() {
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
        | "player.step" | "viewer.set" | "viewer.load" | "project.open" => {
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
        "gui.quit" | "gui.window" => println!("ok {method}"),
        _ => return None,
    }
    Some(())
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
        println!(
            "{:<5} {:<30} {} ({} frames)  {}, {} audio",
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
