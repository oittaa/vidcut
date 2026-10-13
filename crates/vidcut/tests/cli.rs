//! End-to-end tests of the `vidcut` binary on generated fixtures.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, Picture, Sound, VideoCodec};

/// A scratch directory with its own XDG state (undo history) dir, removed
/// when it drops, also when the test fails.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("vidcut-cli-{name}-"))
            .tempdir_in(vidcut_av::testdata::temp_dir())
            .unwrap();
        Self { dir }
    }

    fn project(&self) -> PathBuf {
        self.dir.path().join("p.vidcut")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.cmd_in(&self.project(), args)
    }

    /// A command on another project.
    fn cmd_in(&self, project: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_vidcut"));
        // Own runtime and config dirs: a GUI running on this machine must
        // not answer, and the user's presets stay out of it.
        c.env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env_remove("VIDCUT_SOCKET")
            .env_remove("VIDCUT_PROJECT")
            .arg("-p")
            .arg(project)
            .args(args);
        c
    }

    /// Runs and expects success; returns the parsed `--json` response.
    fn ok(&self, args: &[&str]) -> Value {
        self.ok_in(&self.project(), args)
    }

    fn ok_in(&self, project: &Path, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.cmd_in(project, &all).output().expect("run vidcut");
        assert!(
            out.status.success(),
            "vidcut {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("vidcut {args:?}: bad JSON ({e})"))
    }

    /// Runs and expects failure with `code`; returns the JSON error.
    fn fails(&self, code: i32, args: &[&str]) -> Value {
        self.fails_in(&self.project(), code, args)
    }

    fn fails_in(&self, project: &Path, code: i32, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.cmd_in(project, &all).output().expect("run vidcut");
        assert_eq!(
            out.status.code(),
            Some(code),
            "vidcut {args:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice::<Value>(&out.stdout).expect("JSON error")["error"].clone()
    }

    /// V1 as (id, start, duration, source_in).
    fn v1(&self) -> Vec<(String, i64, i64, i64)> {
        let t = self.ok(&["timeline", "get"]);
        t["result"]["tracks"][0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["type"] == "clip")
            .map(|i| {
                (
                    i["id"].as_str().unwrap().to_string(),
                    i["start"].as_i64().unwrap(),
                    i["duration"].as_i64().unwrap(),
                    i["source_in"].as_i64().unwrap(),
                )
            })
            .collect()
    }
}

fn fixture(spec: FixtureSpec) -> PathBuf {
    fixture::ensure(&spec).expect("fixture")
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn row(id: &str, start: i64, duration: i64, source_in: i64) -> (String, i64, i64, i64) {
    (id.into(), start, duration, source_in)
}

#[test]
fn builds_and_edits_a_timeline() {
    let sb = Sandbox::new("edit");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let b = fixture(FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ));
    let out = sb
        .cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    // `new` takes the file as an argument; -p is ignored for it.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let r = sb.ok(&["media", "add", s(&a), s(&b)]);
    assert_eq!(r["result"]["media"], serde_json::json!(["m1", "m2"]));
    let info = sb.ok(&["project", "summary"]);
    assert_eq!(
        info["result"]["settings"]["fps"], "30",
        "first video sets the format"
    );

    sb.ok(&["edit", "append", "m1", "--range", "10f..60f"]);
    sb.ok(&["edit", "append", "m2"]);
    sb.ok(&["edit", "split", "--at", "1s,70f"]);
    sb.ok(&["edit", "trim", "c3", "--edge", "tail", "--by", "-10f"]);
    // Splits at 1s (frame 30, inside c1) and 70f (inside c2), then c3 shortened
    // by 10 with ripple.
    assert_eq!(
        sb.v1(),
        vec![
            row("c1", 0, 30, 10),
            row("c3", 30, 10, 40),
            row("c2", 40, 20, 0),
            row("c4", 60, 70, 20)
        ]
    );

    // Clip refs by position and timecodes.
    sb.ok(&["edit", "delete", "V1@00:00:01:05"]);
    assert_eq!(
        sb.v1(),
        vec![
            row("c1", 0, 30, 10),
            row("c2", 30, 20, 0),
            row("c4", 50, 70, 20)
        ]
    );

    // Undo across separate processes.
    sb.ok(&["history", "undo", "2"]);
    assert_eq!(sb.v1().len(), 4);
    sb.ok(&["history", "redo"]);
    let hist = sb.ok(&["history", "get"]);
    // History steps read as words.
    assert_eq!(
        hist["result"]["redo"],
        serde_json::json!(["Delete V1@00:00:01:05"])
    );

    // Dry run changes nothing.
    let before = std::fs::read_to_string(sb.project()).unwrap();
    let r = sb.ok(&["--dry-run", "edit", "delete-range", "5f..+10f"]);
    assert_eq!(r["dry_run"], true);
    assert_eq!(std::fs::read_to_string(sb.project()).unwrap(), before);
}

#[test]
fn proxies_from_the_command_line() {
    let sb = Sandbox::new("proxy");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ));
    assert!(
        sb.cmd(&["project", "create"])
            .arg(sb.project())
            .status()
            .unwrap()
            .success()
    );
    sb.ok(&["media", "add", s(&a)]);
    // Plain and automatic: both send `auto`, false or true.
    sb.ok(&["proxy", "generate"]);
    let m = sb.ok(&["media", "get", "m1"]);
    assert_eq!(m["result"]["proxy"]["stale"], false, "{m}");
    sb.ok(&["proxy", "generate", "--auto"]);
    sb.ok(&["proxy", "remove"]);
    assert!(sb.ok(&["media", "get", "m1"])["result"]["proxy"].is_null());
}

#[test]
fn errors_have_stable_exit_codes() {
    let sb = Sandbox::new("errors");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&a)]);
    sb.ok(&["edit", "append", "m1"]);

    let e = sb.fails(3, &["edit", "delete", "c99"]);
    assert_eq!(e["code"], "NOT_FOUND");
    let e = sb.fails(4, &["edit", "trim", "c1", "--edge", "tail", "--by", "+1f"]);
    assert_eq!(e["code"], "INVALID_EDIT");
    let e = sb.fails(2, &["edit", "split", "--at", "30"]);
    assert!(
        e["message"].as_str().unwrap().contains("without a unit"),
        "{e}"
    );
    let rev = sb.ok(&["project", "summary"])["rev"].as_u64().unwrap();
    let e = sb.fails(
        5,
        &[
            "--if-rev",
            &(rev - 1).to_string(),
            "edit",
            "split",
            "--at",
            "1s",
        ],
    );
    assert_eq!(e["code"], "CONFLICT");
    sb.ok(&["--if-rev", &rev.to_string(), "edit", "split", "--at", "1s"]);
    // What reads the project checks it too: nothing comes of a stale one.
    let stale = rev.to_string();
    let out = sb.dir.path().join("stale.mp4");
    let e = sb.fails(5, &["--if-rev", &stale, "export", "start", s(&out)]);
    assert_eq!(e["code"], "CONFLICT");
    assert!(!out.exists());
    let png = sb.dir.path().join("stale.png");
    let e = sb.fails(
        5,
        &[
            "--if-rev",
            &stale,
            "frame",
            "render",
            "--at",
            "1s",
            "--out",
            s(&png),
        ],
    );
    assert_eq!(e["code"], "CONFLICT");
    assert!(!png.exists());
    let e = sb.fails(5, &["--if-rev", &stale, "timeline", "get"]);
    assert_eq!(e["code"], "CONFLICT");
    let now = (rev + 1).to_string();
    sb.ok(&[
        "--if-rev",
        &now,
        "frame",
        "render",
        "--at",
        "1s",
        "--out",
        s(&png),
    ]);
    sb.ok(&["--if-rev", &now, "timeline", "get"]);
    let e = sb.fails(7, &["media", "add", "/nonexistent.mp4"]);
    assert_eq!(e["code"], "MEDIA");
    // No project at all.
    let out = Command::new(env!("CARGO_BIN_EXE_vidcut"))
        .env_remove("VIDCUT_PROJECT")
        .env("XDG_RUNTIME_DIR", sb.dir.path().join("run"))
        .args(["--json", "project", "summary"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn apply_runs_a_batch_as_one_undo_step() {
    let sb = Sandbox::new("apply");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&a)]);
    let batch = sb.dir.path().join("cuts.jsonl");
    std::fs::write(
        &batch,
        "// a rough cut\n\
         {\"method\": \"edit.append\", \"params\": {\"media\": \"m1\", \"range\": \"0f..60f\"}}\n\
         {\"method\": \"edit.split\", \"params\": {\"at\": [20, 40]}}\n\
         {\"method\": \"edit.delete\", \"params\": {\"clips\": \"V1@25f\"}}\n",
    )
    .unwrap();
    let r = sb.ok(&["apply", s(&batch)]);
    assert_eq!(r["changes"]["created"], serde_json::json!(["c1", "c3"]));
    assert_eq!(sb.v1(), vec![row("c1", 0, 20, 0), row("c3", 20, 20, 40)]);
    sb.ok(&["history", "undo"]);
    assert!(sb.v1().is_empty(), "the batch was one undo step");

    // A failing batch changes nothing.
    std::fs::write(&batch, "[{\"method\": \"edit.append\", \"params\": {\"media\": \"m1\"}}, {\"method\": \"edit.delete\", \"params\": {\"clips\": \"c42\"}}]").unwrap();
    let e = sb.fails(3, &["apply", s(&batch)]);
    assert!(
        e["message"].as_str().unwrap().starts_with("command 1"),
        "{e}"
    );
    assert!(sb.v1().is_empty());
}

#[test]
fn schema_and_rpc() {
    let sb = Sandbox::new("schema");
    let schema = sb.ok(&["schema", "edit.trim"]);
    let props = &schema["result"]["params"]["properties"];
    assert!(
        props.get("clip").is_some() && props.get("edge").is_some(),
        "{schema}"
    );
    let all = sb.ok(&["schema"]);
    assert!(all["result"]["oneOf"].as_array().unwrap().len() > 30);

    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&[
        "rpc",
        "media.add",
        &serde_json::json!({ "paths": [a] }).to_string(),
    ]);
    sb.ok(&["rpc", "edit.append", "{\"media\": \"m1\"}"]);
    let r = sb.ok(&["rpc", "edit.split", "{\"at\": \"1s\"}"]);
    assert_eq!(r["result"]["created"], serde_json::json!(["c2"]));
    let e = sb.fails(2, &["rpc", "edit.split", "{\"at\": \"1s\", \"atx\": 3}"]);
    assert!(e["message"].as_str().unwrap().contains("atx"));
}

#[test]
fn concurrent_cli_runs_never_corrupt_the_project() {
    let sb = Sandbox::new("concurrent");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&a)]);
    let children: Vec<_> = (0..16)
        .map(|_| {
            sb.cmd(&["--json", "edit", "append", "m1", "--range", "0f..10f"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let clips = sb.v1();
    assert_eq!(clips.len(), 16);
    for (i, (_, start, duration, _)) in clips.iter().enumerate() {
        assert_eq!(
            (*start, *duration),
            (i as i64 * 10, 10),
            "gapless and complete"
        );
    }
    assert_eq!(sb.ok(&["project", "summary"])["rev"], 17);
}

/// Barcodes of every frame of a video file.
fn barcodes(path: &Path) -> Vec<Option<u32>> {
    let mut r = vidcut_av::decode::VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        out.push(vidcut_av::decode::read_barcode(&f).unwrap());
    }
    out
}

#[test]
fn exports_frames_ranges_clips_and_audio() {
    let sb = Sandbox::new("export");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let b = fixture(FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&a), s(&b)]);
    sb.ok(&["edit", "append", "m1", "--range", "10f..40f"]);
    sb.ok(&["edit", "append", "m2", "--range", "50f..80f"]);
    let out = sb.dir.path().join("out.mkv");
    let o = s(&out);

    // Dry run: a plan, no file.
    let plan = sb.ok(&[
        "export",
        "plan",
        o,
        "--set",
        "video.quality=0",
        "--encoder",
        "cpu",
    ]);
    assert_eq!(plan["result"]["outputs"][0]["frames"], 60);
    assert!(!out.exists());

    // Whole timeline, frame-exact.
    let r = sb.ok(&[
        "export",
        "start",
        o,
        "--set",
        "video.quality=0",
        "--set",
        "video.speed=ultrafast",
        "--encoder",
        "cpu",
    ]);
    assert_eq!(r["result"]["outputs"][0]["frames"], 60);
    let want: Vec<Option<u32>> = (10..40).chain(50..80).map(Some).collect();
    assert_eq!(barcodes(&out), want);

    // Existing outputs need --overwrite; marks set the range (Quick Export).
    sb.fails(4, &["export", "start", o]);
    sb.ok(&["marks", "set", "--in", "25f", "--out", "35f"]);
    sb.ok(&[
        "export",
        "start",
        o,
        "--overwrite",
        "--set",
        "video.quality=0",
        "--encoder",
        "cpu",
    ]);
    // Frames 25..35 straddle the cut at 30: c1's source 35..40, then c2's 50..55.
    assert_eq!(
        barcodes(&out),
        (35..40).chain(50..55).map(Some).collect::<Vec<_>>()
    );
    sb.ok(&["marks", "clear"]);

    // Per clip, with a name template.
    let dir = sb.dir.path().join("clips");
    let r = sb.ok(&[
        "export",
        "start",
        s(&dir),
        "--per-clip",
        "--preset",
        "prores-422hq",
        "--name",
        "{index:02}-{clip}",
    ]);
    assert_eq!(r["result"]["outputs"].as_array().unwrap().len(), 2);
    assert_eq!(
        barcodes(&dir.join("02-c2.mov")),
        (50..80).map(Some).collect::<Vec<_>>()
    );

    // Audio only, container from the extension.
    let wav = sb.dir.path().join("a.wav");
    sb.ok(&["export", "start", s(&wav)]);
    let info = vidcut_av::probe::probe(&wav).unwrap();
    assert!(info.video.is_none());
    assert_eq!(info.audio[0].sample_rate, 48_000);

    // A still frame of the timeline and of a media item.
    let png = sb.dir.path().join("f.png");
    sb.ok(&["frame", "render", "--at", "35f", "--out", s(&png)]);
    assert_eq!(barcodes(&png), vec![Some(55)]);
    sb.ok(&[
        "frame",
        "render",
        "--media",
        "m1",
        "--at",
        "1s",
        "--out",
        s(&png),
    ]);
    assert_eq!(barcodes(&png), vec![Some(30)]);

    // Unknown presets and impossible settings fail cleanly.
    sb.fails(
        3,
        &[
            "export",
            "start",
            s(&sb.dir.path().join("x.mp4")),
            "--preset",
            "nope",
        ],
    );
    sb.fails(
        2,
        &[
            "export",
            "start",
            s(&sb.dir.path().join("x.mp4")),
            "--preset",
            "prores-422hq",
        ],
    );
}

/// `timeline.get` without ids (they differ between projects).
fn timeline_shape(sb: &Sandbox, project: &Path) -> Value {
    let mut t = sb.ok_in(project, &["timeline", "get"])["result"].clone();
    for item in t["tracks"][0]["items"].as_array_mut().unwrap() {
        item.as_object_mut().unwrap().remove("id");
    }
    for k in t["markers"].as_array_mut().unwrap() {
        k.as_object_mut().unwrap().remove("id");
    }
    t
}

#[test]
fn otio_round_trips_frame_exactly() {
    // Export → import reproduces the timeline frame-exactly.
    let sb = Sandbox::new("otio");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let b = fixture(FixtureSpec::new(
        VideoCodec::Hevc10,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&a), s(&b)]);
    sb.ok(&["edit", "append", "m1", "--range", "10f..40f"]);
    sb.ok(&["edit", "append", "m2", "--range", "50f..80f"]);
    sb.ok(&["edit", "append", "m1", "--range", "60f..90f"]);
    sb.ok(&["edit", "append", "m2", "--range", "0f..20f"]);
    sb.ok(&["edit", "audio-roll", "c1", "--edge", "tail", "--by", "4f"]);
    sb.ok(&["edit", "delete", "c3", "--lift"]);
    sb.ok(&["clip", "set", "c4", "--gain-db", "-3"]);
    sb.ok(&[
        "markers", "add", "--at", "15f", "--name", "look", "--color", "rose",
    ]);
    sb.ok(&["marks", "set", "--in", "5f", "--out", "80f"]);
    let cut = sb.dir.path().join("cut.otio");
    let plan = sb.ok(&["export", "plan", s(&cut)]);
    assert_eq!(plan["result"]["outputs"][0]["clips"], 3);
    assert!(!cut.exists());
    // The whole timeline, not In–Out: the marks travel inside.
    let r = sb.ok(&["export", "start", s(&cut)]);
    assert_eq!(r["result"]["outputs"][0]["frames"], 110);
    sb.fails(4, &["export", "start", s(&cut)]);
    sb.fails(
        2,
        &[
            "export",
            "start",
            s(&cut),
            "--preset",
            "h264-master",
            "--overwrite",
        ],
    );

    let other = sb.dir.path().join("q.vidcut");
    sb.ok(&["project", "create", s(&other)]);
    let r = sb.ok_in(&other, &["timeline", "import", s(&cut)]);
    assert_eq!(r["result"]["warnings"], serde_json::json!([]));
    assert_eq!(
        timeline_shape(&sb, &other),
        timeline_shape(&sb, &sb.project())
    );
    // Same pictures at the cuts, in the gap and at the end.
    for at in ["0f", "29f", "30f", "59f", "60f", "89f", "90f", "109f"] {
        let (x, y) = (sb.dir.path().join("x.png"), sb.dir.path().join("y.png"));
        sb.ok(&["frame", "render", "--at", at, "--out", s(&x)]);
        sb.ok_in(&other, &["frame", "render", "--at", at, "--out", s(&y)]);
        assert_eq!(
            std::fs::read(&x).unwrap(),
            std::fs::read(&y).unwrap(),
            "frame {at}"
        );
    }

    // Moved media: an error naming the files, unless found by name.
    let fixtures = a.parent().unwrap();
    let moved = sb.dir.path().join("moved.otio");
    // (Media are named by plain path, as JSON writes it: on Windows with
    // its backslashes escaped.)
    let quoted = serde_json::to_string(fixtures).unwrap();
    let separator = if cfg!(windows) { r"\\" } else { "/" };
    let text = std::fs::read_to_string(&cut).unwrap().replace(
        &format!("{}{separator}", &quoted[..quoted.len() - 1]),
        "\"/nonexistent/footage/",
    );
    std::fs::write(&moved, text).unwrap();
    let third = sb.dir.path().join("r.vidcut");
    sb.ok(&["project", "create", s(&third)]);
    let e = sb.fails_in(&third, 7, &["timeline", "import", s(&moved)]);
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("/nonexistent/footage/"),
        "{e}"
    );
    // Found by name in a subfolder.
    let above = fixtures.parent().unwrap();
    sb.ok_in(
        &third,
        &["timeline", "import", s(&moved), "--search", s(above)],
    );
    assert_eq!(
        timeline_shape(&sb, &third),
        timeline_shape(&sb, &sb.project())
    );
}

/// A Python with OpenTimelineIO (`VIDCUT_OTIO_PYTHON`, else `python3`), to
/// check files against the reference implementation. One named must work:
/// CI names one, and a skip there would hide a broken setup.
fn otio_python() -> Option<String> {
    let named = std::env::var("VIDCUT_OTIO_PYTHON").ok();
    let python = named.clone().unwrap_or_else(|| "python3".into());
    let ok = Command::new(&python)
        .args(["-c", "import opentimelineio"])
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        ok || named.is_none(),
        "VIDCUT_OTIO_PYTHON={python} can't import opentimelineio"
    );
    ok.then_some(python)
}

/// Runs `script` with OpenTimelineIO's Python library; its stdout.
fn python(python: &str, dir: &Path, script: &str) -> String {
    let out = Command::new(python)
        .current_dir(dir)
        .args(["-c", script])
        .output()
        .expect("run python");
    assert!(
        out.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// The same pictures at `frames` in two projects.
fn same_frames(sb: &Sandbox, a: &Path, b: &Path, frames: &[&str]) {
    for at in frames {
        let (x, y) = (sb.dir.path().join("x.png"), sb.dir.path().join("y.png"));
        sb.ok_in(a, &["frame", "render", "--at", at, "--out", s(&x)]);
        sb.ok_in(b, &["frame", "render", "--at", at, "--out", s(&y)]);
        assert_eq!(
            std::fs::read(&x).unwrap(),
            std::fs::read(&y).unwrap(),
            "frame {at}"
        );
    }
}

#[test]
fn otio_bundles_and_timecode() {
    // Deviation X9: `.otioz` and `.otiod` bundles carry the media along;
    // source ranges count from the media's timecode.
    let sb = Sandbox::new("otioz");
    let tc = fixture(
        FixtureSpec::new(VideoCodec::H264, Container::Mov, 320, 180, (25, 1), 75)
            .timecode("10:00:00:00"),
    );
    let plain = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (25, 1),
        75,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    let r = sb.ok(&["media", "add", s(&tc), s(&plain)]);
    assert_eq!(r["result"]["media"], serde_json::json!(["m1", "m2"]));
    let m1 = sb.ok(&["media", "get", "m1"]);
    assert_eq!(
        m1["result"]["info"]["timecode"],
        serde_json::json!({"count": 900_000, "rate": "25"}),
        "{m1}"
    );
    assert_eq!(m1["result"]["timecode"], "10:00:00:00");
    sb.ok(&["edit", "append", "m1", "--range", "10f..40f"]);
    sb.ok(&["edit", "append", "m2", "--range", "5f..35f"]);
    sb.ok(&["edit", "append", "m1", "--range", "50f..70f"]);
    sb.ok(&["edit", "audio-roll", "c1", "--edge", "tail", "--by", "3f"]);
    sb.ok(&["clip", "set", "c2", "--gain-db", "-4"]);

    // .otio: plain paths, sources from 10:00:00:00.
    let cut = sb.dir.path().join("cut.otio");
    sb.ok(&["export", "start", s(&cut)]);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&cut).unwrap()).unwrap();
    let first = &doc["tracks"]["children"][0]["children"][0];
    let reference = &first["media_references"]["DEFAULT_MEDIA"];
    assert_eq!(reference["target_url"], s(&tc));
    assert_eq!(
        reference["available_range"]["start_time"]["value"],
        900_000.0
    );
    assert_eq!(first["source_range"]["start_time"]["value"], 900_010.0);

    // .otioz: planned, then written as OpenTimelineIO's adapter lays it out.
    let bundle = sb.dir.path().join("cut.otioz");
    let plan = sb.ok(&["export", "plan", s(&bundle)]);
    let o = &plan["result"]["outputs"][0];
    assert_eq!(
        (o["format"].as_str(), o["files"].as_u64()),
        (Some("otioz"), Some(2))
    );
    let bytes = std::fs::metadata(&tc).unwrap().len() + std::fs::metadata(&plain).unwrap().len();
    assert_eq!(o["bytes"].as_u64(), Some(bytes));
    assert!(!bundle.exists());
    sb.ok(&["export", "start", s(&bundle)]);
    sb.fails(4, &["export", "start", s(&bundle)]);
    sb.ok(&["export", "start", s(&bundle), "--overwrite"]);
    let names = |zip: &Path| -> Vec<String> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
        (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().into_owned())
            .collect()
    };
    let file_name = |p: &Path| p.file_name().unwrap().to_str().unwrap().to_string();
    assert_eq!(
        names(&bundle),
        [
            "version.txt".to_string(),
            "content.otio".into(),
            format!("media/{}", file_name(&tc)),
            format!("media/{}", file_name(&plain)),
        ]
    );

    // Imported elsewhere: the media unpack next to the bundle.
    let away = sb.dir.path().join("away");
    std::fs::create_dir_all(&away).unwrap();
    let moved = away.join("cut.otioz");
    std::fs::copy(&bundle, &moved).unwrap();
    let other = away.join("q.vidcut");
    sb.ok(&["project", "create", s(&other)]);
    let dry = sb.ok_in(&other, &["--dry-run", "timeline", "import", s(&moved)]);
    assert_eq!(dry["result"]["bundle"]["unpacked"], false);
    assert!(!away.join("cut").exists(), "a dry run unpacks nothing");
    let r = sb.ok_in(&other, &["timeline", "import", s(&moved)]);
    assert_eq!(r["result"]["warnings"], serde_json::json!([]), "{r}");
    assert_eq!(r["result"]["bundle"]["folder"], s(&away.join("cut")));
    let pool = sb.ok_in(&other, &["media", "list"]);
    assert!(
        pool["result"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| Path::new(m["path"].as_str().unwrap())
                .starts_with(away.join("cut").join("media"))),
        "{pool}"
    );
    assert_eq!(
        timeline_shape(&sb, &other),
        timeline_shape(&sb, &sb.project())
    );
    same_frames(
        &sb,
        &sb.project(),
        &other,
        &["0f", "29f", "30f", "59f", "60f", "79f"],
    );
    // Again: the unpacked media are the bundle's, and stay.
    sb.ok_in(&other, &["timeline", "import", s(&moved), "--replace"]);
    // Or unpacked where asked.
    let third = sb.dir.path().join("r.vidcut");
    sb.ok(&["project", "create", s(&third)]);
    let fast = sb.dir.path().join("fast");
    sb.ok_in(
        &third,
        &["timeline", "import", s(&moved), "--unpack", s(&fast)],
    );
    assert!(fast.join("media").join(file_name(&plain)).is_file());

    // .otiod: the same as a folder.
    let folder = sb.dir.path().join("cut.otiod");
    sb.ok(&["export", "start", s(&folder)]);
    assert!(folder.join("content.otio").is_file());
    let fourth = sb.dir.path().join("s.vidcut");
    sb.ok(&["project", "create", s(&fourth)]);
    sb.ok_in(&fourth, &["timeline", "import", s(&folder)]);
    assert_eq!(
        timeline_shape(&sb, &fourth),
        timeline_shape(&sb, &sb.project())
    );

    // Not a timeline nor a container vidcut knows: an error, not an MP4
    // under that name.
    sb.fails(2, &["export", "start", s(&sb.dir.path().join("cut.xyz"))]);
    assert!(!sb.dir.path().join("cut.xyz").exists());

    // The reference implementation reads all three, and its own bundles of
    // our timeline import frame-exactly.
    let Some(py) = otio_python() else {
        eprintln!(
            "skipping the OpenTimelineIO interop part: no Python with opentimelineio \
             (pip install opentimelineio; or set VIDCUT_OTIO_PYTHON)"
        );
        return;
    };
    let out = python(
        &py,
        sb.dir.path(),
        r#"
import opentimelineio as otio
for f in ["cut.otio", "cut.otioz", "cut.otiod"]:
    tl = otio.adapters.read_from_file(f)
    c = tl.find_clips()[0]
    print(f, len(tl.find_clips()), c.source_range.start_time.value,
          c.available_range().start_time.value, c.range_in_parent().start_time.value)
tl = otio.adapters.read_from_file("cut.otio")
# Its bundler needs Windows paths as file: URLs (deviation X9).
import pathlib, re
for c in tl.find_clips():
    r = c.media_reference
    if isinstance(r, otio.schema.ExternalReference) and re.match(r"[A-Za-z]:[\\/]", r.target_url):
        r.target_url = pathlib.Path(r.target_url).as_uri()
otio.adapters.write_to_file(tl, "py.otioz")
"#,
    );
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "cut.otio 6 900010.0 900000.0 0.0",
            "cut.otioz 6 900010.0 900000.0 0.0",
            "cut.otiod 6 900010.0 900000.0 0.0"
        ]
    );
    let fifth = sb.dir.path().join("t.vidcut");
    sb.ok(&["project", "create", s(&fifth)]);
    let r = sb.ok_in(
        &fifth,
        &["timeline", "import", s(&sb.dir.path().join("py.otioz"))],
    );
    assert_eq!(r["result"]["warnings"], serde_json::json!([]), "{r}");
    assert_eq!(
        timeline_shape(&sb, &fifth),
        timeline_shape(&sb, &sb.project())
    );
}

#[test]
fn presets_save_and_delete() {
    let sb = Sandbox::new("presets");
    let r = sb.ok(&[
        "presets",
        "save",
        "web",
        "--from",
        "youtube-1080p",
        "--set",
        "video.quality=23",
        "--title",
        "Web",
        "--quick-export",
    ]);
    let path = r["result"]["path"].as_str().unwrap().to_string();
    assert!(path.starts_with(s(&sb.dir.path().join("config"))), "{path}");
    let p = sb.ok(&["presets", "get", "web"])["result"].clone();
    assert_eq!(
        (p["title"].as_str(), p["quick_export"].as_bool()),
        (Some("Web"), Some(true))
    );
    assert_eq!(p["settings"]["video"]["quality"], 23);
    let list = sb.ok(&["presets", "list"])["result"].clone();
    assert!(list.as_array().unwrap().iter().any(|p| p["name"] == "web"));
    // Existing names need --overwrite; built-in presets can't be deleted.
    sb.fails(4, &["presets", "save", "web", "--from", "h264-master"]);
    sb.fails(4, &["presets", "save", "h264-master", "--from", "av1"]);
    sb.fails(2, &["presets", "save", "../evil", "--from", "av1"]);
    sb.fails(
        2,
        &[
            "presets",
            "save",
            "x",
            "--from",
            "av1",
            "--set",
            "video.nope=1",
        ],
    );
    let settings = serde_json::json!({ "container": "mkv", "audio": { "codec": "flac" } });
    sb.ok(&[
        "presets",
        "save",
        "lossless-audio",
        "--settings",
        &settings.to_string(),
    ]);
    assert_eq!(
        sb.ok(&["presets", "get", "lossless-audio"])["result"]["settings"]["container"],
        "mkv"
    );
    sb.ok(&["presets", "delete", "web"]);
    sb.fails(3, &["presets", "get", "web"]);
    sb.fails(4, &["presets", "delete", "h264-master"]);
    sb.fails(3, &["presets", "delete", "web"]);
}

/// The left channel of a 16-bit stereo WAV, as `f32`.
fn wav_left(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
    bytes[data..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[lo, hi, ..]| f32::from(i16::from_le_bytes([lo, hi])) / 32768.0)
        .collect()
}

fn peak(samples: &[f32], at: usize) -> f32 {
    samples[at..at + 960]
        .iter()
        .fold(0f32, |m, s| m.max(s.abs()))
}

#[test]
fn b_roll_and_music_export_correctly() {
    // B-roll on V2 covers Track 1, music on A1 mixes
    // with Track 1's audio, and the mixer's fader, solo and mute apply to
    // export.
    let sb = Sandbox::new("multitrack");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    let b = fixture(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300).no_audio(),
    );
    let music = fixture(
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 120).audio_only(),
    );
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&a), s(&b), s(&music)]);
    sb.ok(&["edit", "append", "m1"]);
    sb.ok(&[
        "edit",
        "overwrite",
        "m2",
        "--range",
        "150f..180f",
        "--at",
        "30f",
        "--track",
        "V2",
    ]);
    let r = sb.ok(&[
        "edit",
        "overwrite",
        "m3",
        "--range",
        "0f..60f",
        "--at",
        "15f",
    ]);
    assert_eq!(r["result"]["clip"], "c3");
    let tracks = sb.ok(&["track", "list"])["result"].clone();
    assert_eq!(tracks.as_array().unwrap().len(), 3);

    let video = sb.dir.path().join("v.mkv");
    sb.ok(&[
        "export",
        "start",
        s(&video),
        "--encoder",
        "cpu",
        "--set",
        "video.quality=0",
        "--set",
        "video.speed=ultrafast",
    ]);
    let want: Vec<Option<u32>> = (0..30).chain(150..180).chain(60..90).map(Some).collect();
    assert_eq!(barcodes(&video), want, "the B-roll covers frames 30..60");

    let mix = |name: &str| {
        let out = sb.dir.path().join(name);
        sb.ok(&["export", "start", s(&out), "--set", "audio.codec=pcm_s16"]);
        wav_left(&out)
    };
    // Track 1 beeps at 0, 1 and 2 s; the music (from 0.5 s) at 0.5 and 1.5 s.
    let l = mix("mix.wav");
    let found = vidcut_av::beep::onsets(&l, 0.1, 4800);
    assert_eq!(found.len(), 5, "{found:?}");
    for (got, want) in found.iter().zip([0, 24_000, 48_000, 72_000, 96_000]) {
        assert!(
            (*got as i64 - want).abs() <= 48,
            "beep at {got}, want {want}"
        );
    }
    let level = peak(&l, 24_000) / peak(&l, 48_000);
    // The fader takes the music 6 dB down.
    sb.ok(&["track", "set", "A1", "--volume-db", "-6"]);
    let l = mix("fader.wav");
    let ratio = peak(&l, 24_000) / peak(&l, 48_000) / level;
    assert!((ratio - 0.501).abs() < 0.03, "fader: {ratio}");

    sb.ok(&["track", "set", "A1", "--solo", "true"]);
    let l = mix("solo.wav");
    let found = vidcut_av::beep::onsets(&l, 0.1, 4800);
    assert_eq!(found.len(), 2, "only the music: {found:?}");
    sb.ok(&["track", "set", "A1", "--muted", "true"]);
    let l = mix("muted.wav");
    assert!(
        l.iter().all(|s| s.abs() < 1e-3),
        "soloed but muted: silence"
    );
}

#[test]
fn loudness_normalized_exports_meet_their_target() {
    // A -20 LUFS tone exported at -16 LUFS measures -16 LUFS afterwards.
    let sb = Sandbox::new("loudness");
    let tone = fixture(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 300)
            .sound(Sound::Tone),
    );
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&tone)]);
    sb.ok(&["edit", "append", "m1"]);
    let before = sb.ok(&["analyze", "loudness"]);
    assert!((before["result"]["integrated"].as_f64().unwrap() + 20.0).abs() < 0.3);
    let out = sb.dir.path().join("tone.wav");
    let r = sb.ok(&["export", "start", s(&out), "--set", "audio.loudness=-16"]);
    let applied = &r["result"]["outputs"][0]["loudness"];
    assert!(
        (applied["gain_db"].as_f64().unwrap() - 4.0).abs() < 0.3,
        "{applied}"
    );
    let check = sb.dir.path().join("check.vidcut");
    sb.cmd_in(&check, &["project", "create"])
        .arg(&check)
        .output()
        .unwrap();
    sb.ok_in(&check, &["media", "add", s(&out)]);
    sb.ok_in(&check, &["edit", "append", "m1"]);
    let after = sb.ok_in(&check, &["analyze", "loudness"]);
    let lufs = after["result"]["integrated"].as_f64().unwrap();
    assert!((lufs + 16.0).abs() < 0.3, "{after}");

    // Beeps peak at -6 dBFS: raising them 8 LU to -14 would clip, so the
    // gain stops where true peaks reach -1 dBTP.
    let beeps = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ));
    let loud = sb.dir.path().join("loud.vidcut");
    sb.cmd_in(&loud, &["project", "create"])
        .arg(&loud)
        .output()
        .unwrap();
    sb.ok_in(&loud, &["media", "add", s(&beeps)]);
    sb.ok_in(&loud, &["edit", "append", "m1"]);
    let measured = sb.ok_in(&loud, &["analyze", "loudness"])["result"].clone();
    let r = sb.ok_in(
        &loud,
        &[
            "export",
            "start",
            s(&sb.dir.path().join("beeps.wav")),
            "--set",
            "audio.loudness=-14",
        ],
    );
    let applied = &r["result"]["outputs"][0]["loudness"];
    let peak = applied["true_peak"].as_f64().unwrap();
    assert!((peak + 1.0).abs() < 0.05, "held at -1 dBTP: {applied}");
    let gain = applied["gain_db"].as_f64().unwrap();
    let wanted = -14.0 - measured["integrated"].as_f64().unwrap();
    assert!(
        gain < wanted - 1.0,
        "less than the {wanted} dB the target wanted: {applied}"
    );
}

#[test]
fn scene_cuts_split_encoded_shots_exactly() {
    // A clip of five 30-frame shots splits at each shot change;
    // the fourth shot is black.
    let sb = Sandbox::new("scenes");
    let shots = fixture(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 150)
            .picture(Picture::Shots { frames: 30 })
            .no_audio(),
    );
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&shots)]);
    let scenes = sb.ok(&["analyze", "scenes", "--media", "m1"]);
    let cuts: Vec<i64> = scenes["result"][0]["cuts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["frame"].as_i64().unwrap())
        .collect();
    assert_eq!(cuts, vec![30, 60, 90, 120]);
    let black = sb.ok(&["analyze", "black", "--media", "m1"]);
    let r = &black["result"][0]["ranges"][0];
    assert_eq!(
        (r["start"].as_i64(), r["end"].as_i64()),
        (Some(90), Some(120))
    );
    sb.ok(&["edit", "append", "m1", "--range", "10f..150f"]);
    sb.ok(&["edit", "detect-scene-cuts", "c1"]);
    let tl = sb.ok(&["timeline", "get"]);
    let starts: Vec<(i64, i64)> = tl["result"]["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "clip")
        .map(|i| {
            (
                i["start"].as_i64().unwrap(),
                i["source_in"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        starts,
        vec![(0, 10), (20, 30), (50, 60), (80, 90), (110, 120)]
    );
}

#[test]
fn stills_and_sequences_edit_and_export() {
    // A still and a numbered image sequence, added, edited and
    // exported (decisions D38, D39).
    let sb = Sandbox::new("images");
    let still = fixture::ensure_images(&ImageSpec::still(77, 320, 180, "png")).unwrap();
    let seq = fixture::ensure_images(&ImageSpec::sequence(1, 60, 6, 320, 180)).unwrap();
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    // The still leaves the format to the sequence, which brings its rate.
    sb.ok(&["media", "add", s(&still[0])]);
    sb.ok(&["media", "add", s(&seq[10]), "--sequence", "--fps", "25"]);
    let info = sb.ok(&["project", "summary"]);
    assert_eq!(info["result"]["settings"]["fps"], "25", "{info}");
    let m = sb.ok(&["media", "get", "m2"])["result"].clone();
    assert_eq!(m["name"], "img_[000001-000060].png");
    assert_eq!(
        (&m["frames"], &m["sequence"]["count"]),
        (&60.into(), &60.into())
    );
    let m = sb.ok(&["media", "get", "m1"])["result"].clone();
    assert_eq!(
        (&m["still"], &m["frames"]),
        (&true.into(), &125.into()),
        "5 s"
    );
    sb.ok(&["project", "set", "--still-duration", "2s"]);
    sb.ok(&["edit", "append", "m1"]);
    sb.ok(&["edit", "append", "m2", "--range", "10f..+20f"]);
    sb.ok(&["edit", "append", "m1", "--range", "1h..+5f"]);
    let out = sb.dir.path().join("out.mkv");
    sb.ok(&[
        "export",
        "start",
        s(&out),
        "--set",
        "video.quality=0",
        "--set",
        "video.speed=ultrafast",
        "--encoder",
        "cpu",
    ]);
    let mut want = vec![Some(77); 50];
    want.extend((10..30).map(Some));
    want.extend([Some(77); 5]);
    assert_eq!(barcodes(&out), want);
    // Through an .otioz bundle (deviation X9): the still, and the sequence's
    // pictures in a folder of their own; back, the same pictures.
    let bundle = sb.dir.path().join("pictures.otioz");
    sb.ok(&["export", "start", s(&bundle)]);
    let other = sb.dir.path().join("q.vidcut");
    sb.ok(&["project", "create", s(&other)]);
    let r = sb.ok_in(&other, &["timeline", "import", s(&bundle)]);
    assert_eq!(r["result"]["warnings"], serde_json::json!([]), "{r}");
    let m = sb.ok_in(&other, &["media", "get", "m2"])["result"].clone();
    assert_eq!(m["sequence"]["count"], 60, "{m}");
    assert!(
        Path::new(m["path"].as_str().unwrap())
            .starts_with(sb.dir.path().join("pictures").join("media")),
        "{m}"
    );
    let back = sb.dir.path().join("back.mkv");
    sb.ok_in(
        &other,
        &[
            "export",
            "start",
            s(&back),
            "--set",
            "video.quality=0",
            "--set",
            "video.speed=ultrafast",
            "--encoder",
            "cpu",
        ],
    );
    assert_eq!(barcodes(&back), want);
    if let Some(py) = otio_python() {
        let out = python(
            &py,
            sb.dir.path(),
            r#"
import opentimelineio as otio
tl = otio.adapters.read_from_file("pictures.otioz")
for c in tl.find_clips():
    r = c.media_reference
    print(type(r).__name__, getattr(r, "target_url_base", None) or r.target_url)
print(tl.find_clips()[1].media_reference.target_url_for_image_number(0))
"#,
        );
        let name = |p: &Path| p.file_name().unwrap().to_str().unwrap().to_string();
        let (picture, folder) = (name(&still[0]), name(seq[0].parent().unwrap()));
        assert_eq!(
            out.lines().collect::<Vec<_>>(),
            [
                format!("ExternalReference media/{picture}"),
                format!("ImageSequenceReference media/{folder}/"),
                format!("ExternalReference media/{picture}"),
                format!("media/{folder}/{}", name(&seq[0])),
            ],
            "{out}"
        );
    }
    // A file of the sequence without --sequence is a still of its own.
    let r = sb.ok(&["media", "add", s(&seq[0])]);
    assert_eq!(r["changes"]["created"], serde_json::json!(["m3"]));
    // --fps is for sequences (a usage error, before any request).
    let out = sb
        .cmd(&["media", "add", s(&seq[0]), "--fps", "30"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// Decision D45 from the command line: the plan names the passes, a
/// bitrate makes two on the CPU encoders (one with constant quality), and
/// a per-clip export makes two for every file, leaving no statistics.
#[test]
fn two_pass_exports_from_the_command_line() {
    let sb = Sandbox::new("passes");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    sb.ok(&["media", "add", s(&a)]);
    sb.ok(&["edit", "append", "m1", "--range", "0f..45f"]);
    sb.ok(&["edit", "append", "m1", "--range", "45f..90f"]);
    let out = sb.dir.path().join("out.mp4");
    let bitrate = [
        "--set",
        "video.bitrate=300000",
        "--set",
        "video.quality=null",
    ];
    let plan = |extra: &[&str]| {
        let mut args = vec!["export", "plan", s(&out), "--encoder", "cpu"];
        args.extend_from_slice(extra);
        sb.ok(&args)["result"]["video_encoder"]["passes"].clone()
    };
    assert_eq!(
        plan(&bitrate),
        serde_json::json!({"kind": "two_runs", "fast_first": true})
    );
    assert_eq!(plan(&[]), serde_json::json!({"kind": "one"}));
    let mut full = bitrate.to_vec();
    full.extend(["--set", "video.first_pass=full"]);
    assert_eq!(plan(&full)["fast_first"], false);
    let mut one = bitrate.to_vec();
    one.extend(["--set", "video.passes=1"]);
    assert_eq!(plan(&one)["kind"], "one");
    // Two passes need a bitrate.
    let e = sb.fails(2, &["export", "plan", s(&out), "--set", "video.passes=2"]);
    assert!(e["message"].as_str().unwrap().contains("bitrate"), "{e}");

    // Per clip: two passes for each file, every frame where it belongs.
    let clips = sb.dir.path().join("clips");
    let mut args = vec![
        "export",
        "start",
        s(&clips),
        "--per-clip",
        "--encoder",
        "cpu",
        "--set",
        "video.speed=ultrafast",
    ];
    args.extend_from_slice(&bitrate);
    let r = sb.ok(&args);
    let outputs = r["result"]["outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 2);
    for (i, o) in outputs.iter().enumerate() {
        assert_eq!(o["passes"], 2);
        let path = PathBuf::from(o["output"].as_str().unwrap());
        let start = 45 * i as u32;
        let want: Vec<Option<u32>> = (start..start + 45).map(Some).collect();
        assert_eq!(barcodes(&path), want, "{}", path.display());
    }
    let left: Vec<String> = std::fs::read_dir(&clips)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), 2, "only the files: {left:?}");
}
