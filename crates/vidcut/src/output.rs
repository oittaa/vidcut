//! Human-readable output. With `--json` the response object is printed as is.

use serde_json::Value;
use vidcut_core::Response;

/// Prints a response for `method`.
pub fn print(method: &str, r: &Response, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
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
