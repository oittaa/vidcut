use serde_json::Value;
use vidcut_core::Response;

pub fn print(method: &str, r: &Response, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
        return;
    }
    for w in &r.warnings {
        eprintln!("vidcut: warning: {w}");
    }
    for n in r.result["notes"].as_array().into_iter().flatten() {
        eprintln!("vidcut: {}", n.as_str().unwrap_or_default());
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
                | "picture"
        )
    ) && !matches!(
        method,
        "media.list"
            | "media.get"
            | "media.details"
            | "media.transcribe"
            | "project.get"
            | "project.summary"
            | "project.create"
            | "history.get"
            | "track.list"
            | "bins.list"
            | "marks.get"
            | "markers.list"
            | "picture.get"
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
        if let Some(bin) = r.result.get("bin").and_then(Value::as_str) {
            println!("  bin      {bin}");
        }
        for x in r.result["skipped"].as_array().into_iter().flatten() {
            let field = |k: &str| x[k].as_str().unwrap_or_default();
            println!("  skipped  {}: {}", field("path"), field("why"));
        }
        if let Some(tracks) = r.result.get("tracks").and_then(Value::as_array) {
            let ids: Vec<&str> = tracks.iter().filter_map(Value::as_str).collect();
            println!("  tracks   {}", ids.join(" "));
        }
        if let Some(track) = r.result.get("track").and_then(Value::as_str) {
            println!("  on       {track}");
        }
        if let Some(removed) = r.result.get("removed").and_then(Value::as_array) {
            if let Some(frames) = r.result.get("frames") {
                println!("  removed  {frames} frames in {} parts", removed.len());
                for part in removed {
                    let at = |tc: &str, frame: &str| {
                        part[tc]
                            .as_str()
                            .map_or_else(|| format!("{}f", part[frame]), String::from)
                    };
                    println!(
                        "    {} -> {}  {}",
                        at("start_tc", "start"),
                        at("end_tc", "end"),
                        part["clip"].as_str().unwrap_or_default()
                    );
                }
            } else {
                let ids: Vec<&str> = removed.iter().filter_map(Value::as_str).collect();
                println!("  removed  {}", ids.join(" "));
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
        if let Some(relinked) = r.result.get("relinked").and_then(Value::as_array) {
            let field = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
            for x in relinked {
                let (id, path) = (field(x, "media"), field(x, "path"));
                match x["why"].as_str() {
                    Some(why) => println!("  relinked {id}  {path}  (not the same file: {why})"),
                    None => println!("  relinked {id}  {path}"),
                }
            }
            for x in r.result["different"].as_array().into_iter().flatten() {
                let (id, path, why) = (field(x, "media"), field(x, "path"), field(x, "why"));
                println!("  differs  {id}  {path}: {why}");
            }
            for x in r.result["not_found"].as_array().into_iter().flatten() {
                println!("  missing  {}  {}", field(x, "media"), field(x, "name"));
            }
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
        "project.create" => {
            let verb = if r.dry_run { "would create" } else { "created" };
            println!(
                "{verb} {}",
                r.result["project"].as_str().unwrap_or_default()
            );
        }
        "track.list" => {
            for t in r.result.as_array().into_iter().flatten() {
                let mut flags = Vec::new();
                for key in ["locked", "muted", "solo"] {
                    if t[key] == true {
                        flags.push(key.to_string());
                    }
                }
                if t["video_enabled"] == false {
                    flags.push("video off".into());
                }
                if let Some(v) = t["volume_db"].as_f64().filter(|v| *v != 0.0) {
                    flags.push(format!("{v:+} dB"));
                }
                println!(
                    "{:<4} {:>4} clip(s)  {}{}",
                    t["id"].as_str().unwrap_or_default(),
                    t["clips"],
                    t["name"]
                        .as_str()
                        .map(|n| format!("{n:?}  "))
                        .unwrap_or_default(),
                    flags.join(", ")
                );
            }
        }
        "bins.list" => {
            for b in r.result.as_array().into_iter().flatten() {
                println!("{}", b.as_str().unwrap_or_default());
            }
        }
        "markers.list" => {
            for k in r.result.as_array().into_iter().flatten() {
                println!(
                    "{} at {} {} {}",
                    k["id"].as_str().unwrap_or_default(),
                    k["at_tc"].as_str().unwrap_or_default(),
                    k["color"].as_str().unwrap_or_default(),
                    k["name"].as_str().unwrap_or_default()
                );
            }
        }
        "marks.get" => {
            let tc = |k: &str| r.result[k]["tc"].as_str().unwrap_or("-").to_string();
            println!("in {}  out {}", tc("in"), tc("out"));
        }
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
                rate(&r.result["fps"]),
                r.rev
            );
            let b = &r.result["bundle"];
            if let Some(folder) = b["folder"].as_str() {
                let verb = if b["unpacked"] == true {
                    "media in"
                } else {
                    "would unpack the media into"
                };
                println!(
                    "  {verb} {folder} ({} file(s), {})",
                    b["files"],
                    size(b["bytes"].as_u64().unwrap_or(0))
                );
            }
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
                if o["format"] == "otioz" || o["format"] == "otiod" {
                    println!(
                        "wrote {} ({} clips, {} frames, {} media file(s), {})",
                        o["path"].as_str().unwrap_or_default(),
                        o["clips"],
                        o["frames"],
                        o["files"],
                        size(o["bytes"].as_u64().unwrap_or(0))
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
                if let Some(n) = o["gpu_frames"].as_i64().filter(|n| *n > 0) {
                    println!("  {n} of the frames decoded and scaled on the GPU");
                }
                if let Some(why) = o["mix_failed"].as_str() {
                    println!("  the GPU didn't mix the dissolves, so the CPU did: {why}");
                }
                print_copies(&o["copies"]);
                print_color(&o["color"]);
                print_metadata(&o["metadata"]);
            }
            for u in r.result["upscaled"].as_array().into_iter().flatten() {
                let path = std::path::Path::new(u["path"].as_str().unwrap_or_default());
                let name = path.file_name().unwrap_or(path.as_os_str());
                println!(
                    "  Super Scale: {}: {}",
                    name.to_string_lossy(),
                    u["how"].as_str().unwrap_or_default()
                );
            }
            print_notes(&r.result["notes"]);
        }
        "export.plan"
            if matches!(
                r.result["format"].as_str(),
                Some("otio" | "otioz" | "otiod")
            ) =>
        {
            for o in r.result["outputs"].as_array().into_iter().flatten() {
                let media = match o["bytes"].as_u64() {
                    Some(bytes) => format!(", {} media file(s), {}", o["files"], size(bytes)),
                    None => String::new(),
                };
                println!(
                    "{}  {}  frames {}..{} ({} clips{media})",
                    o["format"].as_str().unwrap_or_default(),
                    o["path"].as_str().unwrap_or_default(),
                    o["range"][0],
                    o["range"][1],
                    o["clips"]
                );
            }
            print_notes(&r.result["notes"]);
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
                print_color(&o["color"]);
                print_metadata(&o["metadata"]);
            }
            print_notes(&r.result["notes"]);
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
                "{} integrated, range {:.1} LU, true peak {}, sample peak {} ({} -> {})",
                db("integrated", "LUFS"),
                v["range"].as_f64().unwrap_or(0.0),
                db("true_peak", "dBTP"),
                db("sample_peak", "dBFS"),
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
        "keys.list" => {
            let mut group = "";
            for s in r.result["shortcuts"].as_array().into_iter().flatten() {
                let title = s["group_title"].as_str().unwrap_or_default();
                if title != group {
                    if !group.is_empty() {
                        println!();
                    }
                    println!("{title}");
                    group = title;
                }
                let keys: Vec<&str> = s["keys"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                println!(
                    "  {:<18} {}",
                    keys.join("+"),
                    s["action"].as_str().unwrap_or_default()
                );
            }
        }
        "preferences.get" | "preferences.set" | "preferences.reset" => {
            let mut lines = Vec::new();
            flatten("", &r.result["preferences"], &mut lines);
            for line in lines {
                println!("{line}");
            }
            for key in r.result["ignored"].as_array().into_iter().flatten() {
                println!(
                    "{} is left out: it isn't a preference any more, and goes from the file when \
                     a preference changes",
                    key.as_str().unwrap_or_default()
                );
            }
            if let Some(b) = r.result["backup"].as_str() {
                let verb = if r.dry_run {
                    "would be kept in"
                } else {
                    "are in"
                };
                println!("the old preferences {verb} {b}");
            }
            for t in r.result["themes"].as_array().into_iter().flatten() {
                let from = match (t["error"].as_str(), t["path"].as_str()) {
                    (Some(e), _) => format!(", which doesn't load: {e}"),
                    (None, Some(path)) => format!(" ({path})"),
                    (None, None) => String::new(),
                };
                println!(
                    "appearance.theme={} is {}{from}",
                    t["name"].as_str().unwrap_or_default(),
                    t["title"].as_str().unwrap_or_default()
                );
            }
            for c in r.result["proxy_codecs"].as_array().into_iter().flatten() {
                let plays = match c["plays_on"].as_str() {
                    Some(decoder) => format!("the GPU ({decoder})"),
                    None => "the CPU".into(),
                };
                println!(
                    "proxies.codec={} makes proxies with {}, played on {plays}",
                    c["codec"].as_str().unwrap_or_default(),
                    c["uses"].as_str().unwrap_or("nothing that works here")
                );
            }
            let chosen = r.result["preferences"]["gpu"]["device"]
                .as_str()
                .unwrap_or_default();
            let found = r.result["gpu_chosen"]["id"].as_str();
            let in_use = found.or(r.result["gpu_automatic"]["id"].as_str());
            for g in r.result["gpus"].as_array().into_iter().flatten() {
                let id = g["id"].as_str().unwrap_or_default();
                println!(
                    "gpu.device={id} is {}{}",
                    g["name"].as_str().unwrap_or_default(),
                    if Some(id) == in_use { " (in use)" } else { "" }
                );
            }
            if !chosen.is_empty() && found.is_none() {
                println!("gpu.device={chosen} isn't here: the automatic GPU is in use");
            }
            let c = &r.result["cache"];
            let limit = match c["limit_mb"].as_u64() {
                Some(mb) => format!(", up to {mb} MB"),
                None => ", no limit: the disk's size can't be read".into(),
            };
            println!(
                "cache: {} MB in {} files{limit} ({})",
                c["mb"],
                c["files"],
                c["path"].as_str().unwrap_or_default()
            );
            println!("({})", r.result["path"].as_str().unwrap_or_default());
        }
        "decoders.list" => {
            let rows: Vec<[String; 3]> = r.result["decoders"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| {
                    let text = |k: &str| d[k].as_str().unwrap_or_default().to_string();
                    let ok = if d["ok"] == true {
                        "ok".to_string()
                    } else {
                        format!("no ({})", text("error"))
                    };
                    [text("decoder"), text("codec"), ok]
                })
                .collect();
            let width = |c: usize| rows.iter().map(|r| r[c].len()).max().unwrap_or(0);
            let (w0, w1) = (width(0), width(1));
            for [decoder, codec, ok] in &rows {
                println!("{decoder:<w0$}  {codec:<w1$}  {ok}");
            }
            if rows.is_empty() {
                println!("no GPU decoder here");
            }
        }
        "encoders.list" => {
            let rows: Vec<[String; 4]> = r.result["encoders"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| {
                    let text = |k: &str| e[k].as_str().unwrap_or_default().to_string();
                    let device = match e["device"].as_str() {
                        None => "-".to_string(),
                        Some(d) if e["kind"] == "nvenc" => format!("CUDA device {d}"),
                        Some(d) if d.parse::<u32>().is_ok() => format!("adapter {d}"),
                        Some(d) => d.to_string(),
                    };
                    let ok = if e["ok"] == true {
                        "ok".to_string()
                    } else {
                        format!("no ({})", text("error"))
                    };
                    [text("encoder"), text("pixel"), device, ok]
                })
                .collect();
            let width = |c: usize| rows.iter().map(|r| r[c].len()).max().unwrap_or(0);
            let (w0, w1, w2) = (width(0), width(1), width(2));
            for [encoder, pixel, device, ok] in &rows {
                println!("{encoder:<w0$}  {pixel:<w1$}  {device:<w2$}  {ok}");
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
                if let Some(how) = p["made_with"].as_str() {
                    println!("  {how}");
                }
                print_copies(&p["copies"]);
            }
        }
        "analyze.words" => {
            let v = &r.result;
            for w in v["warnings"].as_array().into_iter().flatten() {
                eprintln!("vidcut: warning: {}", w.as_str().unwrap_or_default());
            }
            let print_words = |words: &Value| {
                for w in words.as_array().into_iter().flatten() {
                    println!(
                        "  {}f–{}f  {}",
                        w["start"],
                        w["end"],
                        w["text"].as_str().unwrap_or_default()
                    );
                }
            };
            match v.as_array() {
                Some(each) => {
                    for e in each {
                        let of = match e["clip"].as_str() {
                            Some(clip) => {
                                format!("{clip} on {}", e["track"].as_str().unwrap_or(""))
                            }
                            None => e["media"].as_str().unwrap_or_default().to_string(),
                        };
                        println!(
                            "{of}: {} word(s), {}, language {}",
                            e["words"].as_array().map_or(0, Vec::len),
                            e["model"].as_str().unwrap_or_default(),
                            e["language"].as_str().unwrap_or("not told")
                        );
                        print_words(&e["words"]);
                    }
                }
                None => {
                    println!(
                        "{} word(s), {} on {}, language {}",
                        v["words"].as_array().map_or(0, Vec::len),
                        v["model"].as_str().unwrap_or_default(),
                        v["device"].as_str().unwrap_or_default(),
                        v["language"].as_str().unwrap_or("not told")
                    );
                    print_words(&v["words"]);
                }
            }
        }
        "media.transcribe" => {
            for m in r.result["media"].as_array().into_iter().flatten() {
                for w in m["warnings"].as_array().into_iter().flatten() {
                    eprintln!("vidcut: warning: {}", w.as_str().unwrap_or_default());
                }
                println!(
                    "{}: {} word(s), {} on {}, language {}{}",
                    m["media"].as_str().unwrap_or_default(),
                    m["words"],
                    m["model"].as_str().unwrap_or_default(),
                    m["device"].as_str().unwrap_or_default(),
                    m["language"].as_str().unwrap_or("not told"),
                    speakers_by(m)
                );
            }
        }
        "edit.ripple_delete_words" => {
            let verb = if r.dry_run { "would cut" } else { "cut" };
            println!(
                "{verb} {} frame(s) in {} span(s) (rev {})",
                r.result["frames"],
                r.result["removed"].as_array().map_or(0, Vec::len),
                r.rev
            );
            for part in r.result["removed"].as_array().into_iter().flatten() {
                println!("  {}f–{}f", part["start"], part["end"]);
            }
        }
        "subtitles.transcribe" => {
            let v = &r.result;
            for w in v["warnings"].as_array().into_iter().flatten() {
                eprintln!("vidcut: warning: {}", w.as_str().unwrap_or_default());
            }
            let cues = v["cues"].as_array().map_or(0, Vec::len);
            let verb = if r.dry_run { "would add" } else { "added" };
            match v["track"].as_str() {
                Some(track) => println!("{verb} {cues} cue(s) on {track} (rev {})", r.rev),
                None => println!("no speech found (rev {})", r.rev),
            }
            let timing = match v["timing"].as_str() {
                Some("words") => "its words",
                Some("phrases") => "its phrases",
                _ => "the pauses",
            };
            println!(
                "  {} on {}, language {}{}, timed by {timing}, {} piece(s) of up to {:.0} s",
                v["model"].as_str().unwrap_or_default(),
                v["device"].as_str().unwrap_or_default(),
                v["language"].as_str().unwrap_or("not told"),
                speakers_by(v),
                v["pieces"],
                v["longest_piece"].as_f64().unwrap_or_default()
            );
        }
        "subtitles.time_words" => {
            let v = &r.result;
            for w in v["warnings"].as_array().into_iter().flatten() {
                eprintln!("vidcut: warning: {}", w.as_str().unwrap_or_default());
            }
            println!(
                "timed {} cue(s) on {}: {} of {} word(s) matched the speech (rev {})",
                v["cues"],
                v["track"].as_str().unwrap_or_default(),
                v["matched"],
                v["words"],
                r.rev
            );
            if let Some(u) = v["unmatched"].as_array().filter(|u| !u.is_empty()) {
                let ids: Vec<&str> = u.iter().filter_map(Value::as_str).collect();
                println!("  no word heard, spread: {}", ids.join(", "));
            }
        }
        "subtitles.retime" => {
            let v = &r.result;
            for w in v["warnings"].as_array().into_iter().flatten() {
                eprintln!("vidcut: warning: {}", w.as_str().unwrap_or_default());
            }
            let verb = if r.dry_run { "would move" } else { "moved" };
            println!(
                "{verb} {} of {} cue(s) on {}: {} (rev {})",
                v["moved"],
                v["cues"],
                v["track"].as_str().unwrap_or_default(),
                retime_correction(&v["correction"]),
                r.rev
            );
            let s = &v["speech"];
            if s.is_object() {
                println!(
                    "  speech: {}, scoring {} against {} at {}: {} times as high, {} needed",
                    retime_secs(&s["offset"]),
                    s["peak"],
                    s["rival"],
                    retime_secs(&s["rival_offset"]),
                    s["clarity"]
                        .as_f64()
                        .map_or("far".into(), |c| format!("{c:.2}")),
                    s["needed"]
                );
            }
            let w = &v["words"];
            if w.is_object() {
                let matched = w["matched"].as_u64().unwrap_or(0);
                println!(
                    "  words: {matched} of {} cue(s) matched, heard by {} on {}, language {}",
                    v["cues"],
                    w["model"].as_str().unwrap_or_default(),
                    w["device"].as_str().unwrap_or_default(),
                    w["language"].as_str().unwrap_or("not told")
                );
                if w["speech_errors"].is_object() {
                    println!(
                        "  after the speech's offset: {}",
                        retime_errors(&w["speech_errors"])
                    );
                }
                if v["correction"]["errors"].is_object() {
                    println!(
                        "  after the correction: {}",
                        retime_errors(&v["correction"]["errors"])
                    );
                }
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
        "media.details" => media_details(&r.result),
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
        "picture.get" => {
            let v = &r.result;
            let pair = |k: &str| {
                let n = |i: usize| v[k][i].as_f64().unwrap_or_default();
                format!("{}, {}", n(0), n(1))
            };
            let crop = &v["crop"];
            let side = |k: &str| crop[k].as_f64().unwrap_or_default();
            println!(
                "{}  position {}  zoom {}  rotation {}°  anchor {}  pitch {}  yaw {}",
                v["clip"].as_str().unwrap_or_default(),
                pair("position"),
                pair("zoom"),
                v["rotation"].as_f64().unwrap_or_default(),
                pair("anchor"),
                v["pitch"].as_f64().unwrap_or_default(),
                v["yaw"].as_f64().unwrap_or_default(),
            );
            let flips: Vec<&str> = [
                ("flip_horizontal", "horizontal"),
                ("flip_vertical", "vertical"),
            ]
            .into_iter()
            .filter(|(k, _)| v[*k] == true)
            .map(|(_, name)| name)
            .collect();
            if !flips.is_empty() {
                println!("  flipped {}", flips.join(" and "));
            }
            println!(
                "  crop left {}  right {}  top {}  bottom {}",
                side("left"),
                side("right"),
                side("top"),
                side("bottom")
            );
            let z = &v["dynamic_zoom"];
            if z.is_object() {
                let end = |k: &str| {
                    let b = &z[k];
                    let n = |i: usize| b["center"][i].as_f64().unwrap_or_default();
                    format!(
                        "{} at {}, {}",
                        b["size"].as_f64().unwrap_or_default(),
                        n(0),
                        n(1)
                    )
                };
                println!(
                    "  dynamic zoom from {} to {}  {}",
                    end("start"),
                    end("end"),
                    z["ease"].as_str().unwrap_or_default()
                );
            }
        }
        "timeline.edit_points" => {
            for p in r.result.as_array().into_iter().flatten() {
                println!("{}  {}", p["tc"].as_str().unwrap_or_default(), p["frame"]);
            }
        }
        "history.get" => {
            let v = &r.result;
            let at = v["at"].as_u64().unwrap_or(0);
            let total =
                v["undo_steps"].as_u64().unwrap_or(0) + v["redo_steps"].as_u64().unwrap_or(0);
            println!("at step {at} of {total}");
            let now = crate::when::now_ms();
            for s in v["steps"].as_array().into_iter().flatten() {
                let n = s["step"].as_u64().unwrap_or(0);
                let by = match (s["by"]["name"].as_str(), s["by"]["kind"].as_str()) {
                    (Some(name), _) => name.to_string(),
                    (None, Some(kind)) => kind.to_uppercase(),
                    _ => "-".into(),
                };
                let when = s["time"]
                    .as_u64()
                    .map_or_else(|| "-".into(), |t| crate::when::ago(t, now));
                println!(
                    "{}{n:>5}  {by:<12}  {when:<16}  {}{}",
                    if n == at { '*' } else { ' ' },
                    s["label"].as_str().unwrap_or_default(),
                    if s["done"] == true { "" } else { "  (undone)" }
                );
            }
        }
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&r.result).unwrap_or_default()
        ),
    }
}

fn live(method: &str, r: &Response) -> Option<()> {
    let v = &r.result;
    match method {
        "player.state"
        | "player.play"
        | "player.pause"
        | "player.toggle"
        | "player.seek"
        | "player.step"
        | "player.mute"
        | "player.audio_scrubbing"
        | "player.loop"
        | "viewer.set"
        | "viewer.load"
        | "viewer.guides"
        | "project.open" => {
            if method == "viewer.guides" {
                println!(
                    "guides: {}",
                    serde_json::to_string(&v["guides"]).unwrap_or_default()
                );
            } else {
                println!("{}", state_line(v));
            }
        }
        "player.stats" => {
            println!("{}", state_line(v));
            let stats =
                serde_json::from_value::<crate::live::stats::PlayerStats>(v["stats"].clone());
            for (heading, rows) in stats.iter().flat_map(|s| s.groups()) {
                println!("  {heading}");
                for (label, fact) in rows {
                    println!("    {label:<12}  {fact}");
                }
            }
        }
        "gui.screenshot" => println!(
            "wrote {} ({}x{})",
            v["path"].as_str().unwrap_or_default(),
            v["width"],
            v["height"]
        ),
        "gui.log" => {
            for e in v.as_array().into_iter().flatten() {
                let status = match e["error"].as_str() {
                    Some(err) => format!("  ERROR {err}"),
                    None => String::new(),
                };
                println!(
                    "#{:<5} {:<7} {}{}",
                    e["seq"],
                    e["origin"].as_str().unwrap_or_default(),
                    e["summary"].as_str().unwrap_or_default(),
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
            println!(
                "clips {}  cues {}  media {}  edit {}",
                ids("clips"),
                ids("cues"),
                ids("media"),
                edit_point(&v["edit"])
            );
        }
        "edit.copy" => println!(
            "copied {} clip(s); paste with `vidcut edit paste` (GUI) or --clips",
            v["clips"].as_array().map_or(0, Vec::len)
        ),
        "gui.quit" | "gui.window" | "gui.trim_editor" | "gui.dynamic_trim" => {
            println!("ok {method}")
        }
        "gui.state" | "gui.show" | "gui.close" | "gui.target" | "gui.timeline" => {
            let t = &v["timeline"];
            let on = |k: &str| if t[k] == true { "on" } else { "off" };
            let windows: Vec<&str> = v["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            println!("edit {}", edit_point(&v["edit"]));
            println!("target {}", v["target"].as_str().unwrap_or("-"));
            println!(
                "timeline zoom {} px/frame, fixed playhead {}, snap {}, safe edit {}",
                t["zoom"],
                on("fixed_playhead"),
                on("snap"),
                on("safe_edit")
            );
            println!(
                "clips show names {}, durations {}, status {}, filmstrips {}, waveforms {}",
                on("names"),
                on("durations"),
                on("status"),
                on("filmstrips"),
                on("waveforms")
            );
            println!(
                "boring detector {} (longer than {} s, shorter than {} frames)",
                on("boring"),
                t["boring_secs"],
                t["jump_frames"]
            );
            println!("subtitle tracks minimized {}", on("minimize_subtitles"));
            println!(
                "windows {}",
                if windows.is_empty() {
                    "-".to_string()
                } else {
                    windows.join(" ")
                }
            );
        }
        "pool.set" => println!(
            "pool: bin {:?}, sorted by {}{}, {} view{}",
            v["bin"].as_str().unwrap_or_default(),
            v["sort"].as_str().unwrap_or_default(),
            if v["descending"] == true {
                " (descending)"
            } else {
                ""
            },
            v["view"].as_str().unwrap_or_default(),
            match v["search"].as_str() {
                Some(s) if !s.is_empty() => format!(", search {s:?}"),
                _ => String::new(),
            },
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

fn edit_point(e: &Value) -> String {
    if e.is_null() {
        return "-".into();
    }
    format!(
        "{} {} {}",
        e["at"],
        e["track"].as_str().unwrap_or_default(),
        e["side"].as_str().unwrap_or_default()
    )
}

fn speakers_by(v: &Value) -> String {
    v["speakers_by"]
        .as_str()
        .map(|by| format!(", speakers by {by}"))
        .unwrap_or_default()
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn retime_secs(v: &Value) -> String {
    format!("{:+.2} s", v.as_f64().unwrap_or_default())
}

fn retime_errors(e: &Value) -> String {
    format!(
        "median {:.2} s, 90% within {:.2} s, {:.1}% within 0.5 s",
        e["median"].as_f64().unwrap_or_default(),
        e["p90"].as_f64().unwrap_or_default(),
        e["within"].as_f64().unwrap_or_default() * 100.0
    )
}

fn retime_correction(c: &Value) -> String {
    let speed = match (c["speed"].as_f64(), c["ratio"].as_str()) {
        (Some(s), Some(r)) if s != 1.0 => format!(" at {r} the speed (×{s:.5})"),
        (Some(s), None) if s != 1.0 => format!(" at ×{s:.5} the speed"),
        _ => String::new(),
    };
    let sections = c["sections"].as_array().cloned().unwrap_or_default();
    let base = match sections.as_slice() {
        [one] => format!("{}{speed}", retime_secs(&one["offset"])),
        many => format!(
            "{} sections{speed}: {}",
            many.len(),
            many.iter()
                .map(|s| format!(
                    "{} from {}",
                    retime_secs(&s["offset"]),
                    clock(s["from"].as_f64().unwrap_or_default())
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    match c["snapped"].as_u64().unwrap_or(0) {
        0 => base,
        n => format!("{n} cue(s) to their words, the others {base}"),
    }
}

fn job_line(j: &Value) -> String {
    let state = j["state"].as_str().unwrap_or_default();
    let time = match (state, j["eta"].as_f64()) {
        ("running", Some(eta)) => format!("ETA {}", clock(eta)),
        ("running", None) => "starting".into(),
        ("queued", _) => "waiting".into(),
        _ => format!("took {}", clock(j["elapsed"].as_f64().unwrap_or(0.0))),
    };
    let error = j["error"]["message"]
        .as_str()
        .map(|e| format!("  ({e})"))
        .unwrap_or_default();
    let speech = &j["details"]["speech"];
    let uses = match speech["model"].as_str() {
        Some(model) => format!(
            "  ({model}{}, language {}{})",
            speech["device"]
                .as_str()
                .map(|d| format!(" on {d}"))
                .unwrap_or_default(),
            speech["language"].as_str().unwrap_or("auto"),
            speakers_by(speech)
        ),
        None => String::new(),
    };
    format!(
        "{:<4} {:<9} {:>3}%  {:<10} {}{uses}{error}",
        j["id"].as_str().unwrap_or_default(),
        state,
        (j["progress"].as_f64().unwrap_or(0.0) * 100.0).floor(),
        time,
        j["title"].as_str().unwrap_or_default()
    )
}

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
    let count = match v["timecode_display"].as_str() {
        Some(d) if d != "auto" => format!(", {d}"),
        _ => String::new(),
    };
    format!(
        "viewer {}{}{} | timeline {} | {} {}",
        v["viewer"].as_str().unwrap_or("-"),
        count,
        if v["loop"].as_bool() == Some(true) {
            ", looping"
        } else {
            ""
        },
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
    let speed = match c["speed"].as_f64() {
        Some(0.0) => ", freeze".to_string(),
        Some(x) => format!(", {}%", (x * 100.0).round()),
        None => String::new(),
    };
    format!(
        "{:<5} {} -> {} {:>6}f  {}  [{} src {}..{}, {}{}{}]",
        s("id"),
        s("start_tc"),
        s("end_tc"),
        c["duration"],
        s("name"),
        s("media"),
        c["source_in"],
        c["source_out"],
        audio,
        speed,
        if c["enabled"] == false {
            ", disabled"
        } else {
            ""
        }
    )
}

fn rate(v: &Value) -> String {
    let text = v.as_str().unwrap_or_default();
    text.parse::<vidcut_core::time::Rational>()
        .map_or_else(|_| text.to_string(), |r| r.decimal())
}

fn timeline(t: &Value) {
    println!(
        "duration {} ({} frames at {} fps)",
        t["duration_tc"].as_str().unwrap_or_default(),
        t["duration"],
        rate(&t["fps"])
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
                rate(&v["fps"]),
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
        let missing = if i["missing"] == true {
            "  MISSING"
        } else {
            ""
        };
        println!(
            "{:<5} {:<30} {} ({} frames)  {}, {} audio{bin}{proxy}{missing}",
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
        rate(&st["fps"]),
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

fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "kB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

fn print_color(c: &Value) {
    let why: Vec<&str> = c["why"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !why.is_empty() {
        println!("    {}", why.join("; "));
    }
}

fn print_metadata(m: &Value) {
    if !m.is_object() || m["mode"].as_str() == Some("none") {
        return;
    }
    println!("    HDR metadata: {}", crate::export::metadata_words(m));
}

fn print_copies(c: &Value) {
    let n = |key: &str| c[key].as_u64().unwrap_or(0);
    if n("down") + n("up") == 0 {
        return;
    }
    let mb = |key: &str| n(key) as f64 / f64::from(1 << 20);
    println!(
        "  frames copied down from the GPU: {} ({:.0} MB), up to it: {} ({:.0} MB)",
        n("down"),
        mb("down_bytes"),
        n("up"),
        mb("up_bytes")
    );
}

fn print_notes(notes: &Value) {
    for n in notes.as_array().into_iter().flatten() {
        let set: Vec<String> = n["set"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(|s| format!("--set {s}")))
            .collect();
        let hint = if set.is_empty() {
            String::new()
        } else {
            format!("; to keep it HDR: {}", set.join(" "))
        };
        println!("note: {}{hint}", n["message"].as_str().unwrap_or_default());
    }
}

fn media_details(v: &Value) {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let rate = |v: &Value| {
        v.as_u64()
            .map_or(String::new(), |b| format!(", {:.1} Mb/s", b as f64 / 1e6))
    };
    println!("{} ({} bytes)", text(&v["path"]), v["size"]);
    println!("  {}{}", text(&v["format"]), rate(&v["bit_rate"]));
    for (k, t) in v["tags"].as_object().into_iter().flatten() {
        println!("  {k}: {}", text(t));
    }
    for s in v["streams"].as_array().into_iter().flatten() {
        let mut parts = vec![text(&s["codec"])];
        for key in [
            "profile",
            "pix_fmt",
            "chroma",
            "layout",
            "sample_format",
            "language",
        ] {
            if let Some(x) = s[key].as_str() {
                parts.push(x.to_string());
            }
        }
        if let Some(b) = s["bits"].as_u64() {
            parts.push(format!("{b} bits"));
        }
        if let Some(r) = s["sample_rate"].as_u64() {
            parts.push(format!("{r} Hz"));
        }
        let color: Vec<String> = ["primaries", "transfer", "matrix", "range"]
            .iter()
            .filter_map(|k| s[*k].as_str().map(String::from))
            .collect();
        if !color.is_empty() {
            parts.push(format!("color {}", color.join("/")));
        }
        println!(
            "  #{} {}: {}{}",
            s["index"],
            text(&s["kind"]),
            parts.join(", "),
            rate(&s["bit_rate"])
        );
        for (k, t) in s["tags"].as_object().into_iter().flatten() {
            println!("    {k}: {}", text(t));
        }
    }
}
