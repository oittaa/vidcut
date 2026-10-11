//! End-to-end tests of the `vidcut` binary on generated fixtures.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, Picture, Sound, VideoCodec};

/// A scratch directory with its own XDG state (undo history) dir.
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("vidcut-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn project(&self) -> PathBuf {
        self.dir.join("p.vidcut")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.cmd_in(&self.project(), args)
    }

    /// A command on another project.
    fn cmd_in(&self, project: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_vidcut"));
        // Own runtime and config dirs: a GUI running on this machine must
        // not answer, and the user's presets stay out of it.
        c.env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_RUNTIME_DIR", self.dir.join("run"))
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
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
        let t = self.ok(&["timeline"]);
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

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
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
    let out = sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    // `new` takes the file as an argument; -p is ignored for it.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let r = sb.ok(&["media", "add", s(&a), s(&b)]);
    assert_eq!(r["result"]["media"], serde_json::json!(["m1", "m2"]));
    let info = sb.ok(&["info"]);
    assert_eq!(
        info["result"]["settings"]["fps"], "30",
        "first video sets the format"
    );

    sb.ok(&["edit", "append", "m1", "--range", "10f..60f"]);
    sb.ok(&["edit", "append", "m2"]);
    sb.ok(&["edit", "split", "--at", "1s,70f"]);
    sb.ok(&["edit", "trim", "c3", "--tail", "--by", "-10f"]);
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
    sb.ok(&["undo", "2"]);
    assert_eq!(sb.v1().len(), 4);
    sb.ok(&["redo"]);
    let hist = sb.ok(&["history"]);
    assert_eq!(hist["result"]["redo"], serde_json::json!(["edit.delete"]));

    // Dry run changes nothing.
    let before = std::fs::read_to_string(sb.project()).unwrap();
    let r = sb.ok(&["--dry-run", "edit", "delete-range", "5f..+10f"]);
    assert_eq!(r["dry_run"], true);
    assert_eq!(std::fs::read_to_string(sb.project()).unwrap(), before);
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    sb.ok(&["media", "add", s(&a)]);
    sb.ok(&["edit", "append", "m1"]);

    let e = sb.fails(3, &["edit", "delete", "c99"]);
    assert_eq!(e["code"], "NOT_FOUND");
    let e = sb.fails(4, &["edit", "trim", "c1", "--tail", "--by", "+1f"]);
    assert_eq!(e["code"], "INVALID_EDIT");
    let e = sb.fails(2, &["edit", "split", "--at", "30"]);
    assert!(
        e["message"].as_str().unwrap().contains("without a unit"),
        "{e}"
    );
    let rev = sb.ok(&["info"])["rev"].as_u64().unwrap();
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
    let out = sb.dir.join("stale.mp4");
    let e = sb.fails(5, &["--if-rev", &stale, "export", s(&out)]);
    assert_eq!(e["code"], "CONFLICT");
    assert!(!out.exists());
    let png = sb.dir.join("stale.png");
    let e = sb.fails(
        5,
        &["--if-rev", &stale, "frame", "--at", "1s", "-o", s(&png)],
    );
    assert_eq!(e["code"], "CONFLICT");
    assert!(!png.exists());
    let e = sb.fails(5, &["--if-rev", &stale, "timeline"]);
    assert_eq!(e["code"], "CONFLICT");
    let now = (rev + 1).to_string();
    sb.ok(&["--if-rev", &now, "frame", "--at", "1s", "-o", s(&png)]);
    sb.ok(&["--if-rev", &now, "timeline"]);
    let e = sb.fails(7, &["media", "add", "/nonexistent.mp4"]);
    assert_eq!(e["code"], "MEDIA");
    // No project at all.
    let out = Command::new(env!("CARGO_BIN_EXE_vidcut"))
        .env_remove("VIDCUT_PROJECT")
        .env("XDG_RUNTIME_DIR", sb.dir.join("run"))
        .args(["--json", "info"])
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    sb.ok(&["media", "add", s(&a)]);
    let batch = sb.dir.join("cuts.jsonl");
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
    sb.ok(&["undo"]);
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    sb.ok(&[
        "rpc",
        "media.add",
        &format!("{{\"paths\": [\"{}\"]}}", s(&a)),
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
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
    assert_eq!(sb.ok(&["info"])["rev"], 17);
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    sb.ok(&["media", "add", s(&a), s(&b)]);
    sb.ok(&["edit", "append", "m1", "--range", "10f..40f"]);
    sb.ok(&["edit", "append", "m2", "--range", "50f..80f"]);
    let out = sb.dir.join("out.mkv");
    let o = s(&out);

    // Dry run: a plan, no file.
    let plan = sb.ok(&[
        "--dry-run",
        "export",
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
    sb.fails(4, &["export", o]);
    sb.ok(&["marks", "set", "--in", "25f", "--out", "35f"]);
    sb.ok(&[
        "export",
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
    let dir = sb.dir.join("clips");
    let r = sb.ok(&[
        "export",
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
    let wav = sb.dir.join("a.wav");
    sb.ok(&["export", s(&wav)]);
    let info = vidcut_av::probe::probe(&wav).unwrap();
    assert!(info.video.is_none());
    assert_eq!(info.audio[0].sample_rate, 48_000);

    // A still frame of the timeline and of a media item.
    let png = sb.dir.join("f.png");
    sb.ok(&["frame", "--at", "35f", "-o", s(&png)]);
    assert_eq!(barcodes(&png), vec![Some(55)]);
    sb.ok(&["frame", "--media", "m1", "--at", "1s", "-o", s(&png)]);
    assert_eq!(barcodes(&png), vec![Some(30)]);

    // Unknown presets and impossible settings fail cleanly.
    sb.fails(3, &["export", s(&sb.dir.join("x.mp4")), "--preset", "nope"]);
    sb.fails(
        2,
        &[
            "export",
            s(&sb.dir.join("x.mp4")),
            "--preset",
            "prores-422hq",
        ],
    );
}

/// `timeline.get` without ids (they differ between projects).
fn timeline_shape(sb: &Sandbox, project: &Path) -> Value {
    let mut t = sb.ok_in(project, &["timeline"])["result"].clone();
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
    // M5 acceptance: export → import reproduces the timeline frame-exactly.
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    sb.ok(&["media", "add", s(&a), s(&b)]);
    sb.ok(&["edit", "append", "m1", "--range", "10f..40f"]);
    sb.ok(&["edit", "append", "m2", "--range", "50f..80f"]);
    sb.ok(&["edit", "append", "m1", "--range", "60f..90f"]);
    sb.ok(&["edit", "append", "m2", "--range", "0f..20f"]);
    sb.ok(&["edit", "audio-roll", "c1", "--tail", "--by", "4f"]);
    sb.ok(&["edit", "delete", "c3", "--lift"]);
    sb.ok(&["clip", "set", "c4", "--gain", "-3"]);
    sb.ok(&[
        "markers", "add", "--at", "15f", "--name", "look", "--color", "rose",
    ]);
    sb.ok(&["marks", "set", "--in", "5f", "--out", "80f"]);
    let cut = sb.dir.join("cut.otio");
    let plan = sb.ok(&["--dry-run", "export", s(&cut)]);
    assert_eq!(plan["result"]["outputs"][0]["clips"], 3);
    assert!(!cut.exists());
    // The whole timeline, not In–Out: the marks travel inside.
    let r = sb.ok(&["export", s(&cut)]);
    assert_eq!(r["result"]["outputs"][0]["frames"], 110);
    sb.fails(4, &["export", s(&cut)]);
    sb.fails(
        2,
        &["export", s(&cut), "--preset", "h264-master", "--overwrite"],
    );

    let other = sb.dir.join("q.vidcut");
    sb.ok(&["new", s(&other)]);
    let r = sb.ok_in(&other, &["import", s(&cut)]);
    assert_eq!(r["result"]["warnings"], serde_json::json!([]));
    assert_eq!(
        timeline_shape(&sb, &other),
        timeline_shape(&sb, &sb.project())
    );
    // Same pictures at the cuts, in the gap and at the end.
    for at in ["0f", "29f", "30f", "59f", "60f", "89f", "90f", "109f"] {
        let (x, y) = (sb.dir.join("x.png"), sb.dir.join("y.png"));
        sb.ok(&["frame", "--at", at, "-o", s(&x)]);
        sb.ok_in(&other, &["frame", "--at", at, "-o", s(&y)]);
        assert_eq!(
            std::fs::read(&x).unwrap(),
            std::fs::read(&y).unwrap(),
            "frame {at}"
        );
    }

    // Moved media: an error naming the files, unless found by name.
    let fixtures = a.parent().unwrap();
    let moved = sb.dir.join("moved.otio");
    let text = std::fs::read_to_string(&cut).unwrap().replace(
        &format!("file://{}/", s(fixtures)),
        "file:///nonexistent/footage/",
    );
    std::fs::write(&moved, text).unwrap();
    let third = sb.dir.join("r.vidcut");
    sb.ok(&["new", s(&third)]);
    let e = sb.fails_in(&third, 7, &["import", s(&moved)]);
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("/nonexistent/footage/"),
        "{e}"
    );
    // Found by name in a subfolder (as Resolve relinks, p. 530).
    let above = fixtures.parent().unwrap();
    sb.ok_in(&third, &["import", s(&moved), "--search", s(above)]);
    assert_eq!(
        timeline_shape(&sb, &third),
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
    assert!(path.starts_with(s(&sb.dir.join("config"))), "{path}");
    let p = sb.ok(&["presets", "web"])["result"].clone();
    assert_eq!(
        (p["title"].as_str(), p["quick_export"].as_bool()),
        (Some("Web"), Some(true))
    );
    assert_eq!(p["settings"]["video"]["quality"], 23);
    let list = sb.ok(&["presets"])["result"].clone();
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
        sb.ok(&["presets", "lossless-audio"])["result"]["settings"]["container"],
        "mkv"
    );
    sb.ok(&["presets", "rm", "web"]);
    sb.fails(3, &["presets", "web"]);
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
    // M6 acceptance: B-roll on V2 covers Track 1 (p. 647), music on A1 mixes
    // with Track 1's audio, and the mixer's fader, solo and mute apply to
    // export (p. 650).
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
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

    let video = sb.dir.join("v.mkv");
    sb.ok(&[
        "export",
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
        let out = sb.dir.join(name);
        sb.ok(&["export", s(&out), "--set", "audio.codec=pcm_s16"]);
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
    sb.ok(&["track", "set", "A1", "--volume", "-6"]);
    let l = mix("fader.wav");
    let ratio = peak(&l, 24_000) / peak(&l, 48_000) / level;
    assert!((ratio - 0.501).abs() < 0.03, "fader: {ratio}");

    sb.ok(&["track", "set", "A1", "--solo"]);
    let l = mix("solo.wav");
    let found = vidcut_av::beep::onsets(&l, 0.1, 4800);
    assert_eq!(found.len(), 2, "only the music: {found:?}");
    sb.ok(&["track", "set", "A1", "--mute"]);
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
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    sb.ok(&["media", "add", s(&tone)]);
    sb.ok(&["edit", "append", "m1"]);
    let before = sb.ok(&["analyze", "loudness"]);
    assert!((before["result"]["integrated"].as_f64().unwrap() + 20.0).abs() < 0.3);
    let out = sb.dir.join("tone.wav");
    let r = sb.ok(&["export", s(&out), "--set", "audio.loudness=-16"]);
    let applied = &r["result"]["outputs"][0]["loudness"];
    assert!(
        (applied["gain_db"].as_f64().unwrap() - 4.0).abs() < 0.3,
        "{applied}"
    );
    let check = sb.dir.join("check.vidcut");
    sb.cmd_in(&check, &["new"]).arg(&check).output().unwrap();
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
    let loud = sb.dir.join("loud.vidcut");
    sb.cmd_in(&loud, &["new"]).arg(&loud).output().unwrap();
    sb.ok_in(&loud, &["media", "add", s(&beeps)]);
    sb.ok_in(&loud, &["edit", "append", "m1"]);
    let measured = sb.ok_in(&loud, &["analyze", "loudness"])["result"].clone();
    let r = sb.ok_in(
        &loud,
        &[
            "export",
            s(&sb.dir.join("beeps.wav")),
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
    // p. 690: a clip of five 30-frame shots splits at each shot change;
    // the fourth shot is black.
    let sb = Sandbox::new("scenes");
    let shots = fixture(
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 150)
            .picture(Picture::Shots { frames: 30 })
            .no_audio(),
    );
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
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
    let tl = sb.ok(&["timeline"]);
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
    // M9: a still and a numbered image sequence, added, edited and
    // exported (pp. 114, 366, 1194; decisions D38, D39).
    let sb = Sandbox::new("images");
    let still = fixture::ensure_images(&ImageSpec::still(77, 320, 180, "png")).unwrap();
    let seq = fixture::ensure_images(&ImageSpec::sequence(1, 60, 6, 320, 180)).unwrap();
    sb.cmd(&["new"]).arg(sb.project()).output().unwrap();
    // The still leaves the format to the sequence, which brings its rate.
    sb.ok(&["media", "add", s(&still[0])]);
    sb.ok(&["media", "add", s(&seq[10]), "--sequence", "--fps", "25"]);
    let info = sb.ok(&["info"]);
    assert_eq!(info["result"]["settings"]["fps"], "25", "{info}");
    let m = sb.ok(&["media", "show", "m2"])["result"].clone();
    assert_eq!(m["name"], "img_[000001-000060].png");
    assert_eq!(
        (&m["frames"], &m["sequence"]["count"]),
        (&60.into(), &60.into())
    );
    let m = sb.ok(&["media", "show", "m1"])["result"].clone();
    assert_eq!(
        (&m["still"], &m["frames"]),
        (&true.into(), &125.into()),
        "5 s"
    );
    sb.ok(&["set", "--still-duration", "2s"]);
    sb.ok(&["edit", "append", "m1"]);
    sb.ok(&["edit", "append", "m2", "--range", "10f..+20f"]);
    sb.ok(&["edit", "append", "m1", "--range", "1h..+5f"]);
    let out = sb.dir.join("out.mkv");
    sb.ok(&[
        "export",
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
