//! End-to-end tests of the `vidcut` binary on generated fixtures.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};

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
        let mut c = Command::new(env!("CARGO_BIN_EXE_vidcut"));
        c.env("XDG_STATE_HOME", self.dir.join("state"))
            .env_remove("VIDCUT_PROJECT")
            .arg("-p")
            .arg(self.project())
            .args(args);
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("run vidcut")
    }

    /// Runs and expects success; returns the parsed `--json` response.
    fn ok(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.run(&all);
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
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.run(&all);
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
    let e = sb.fails(7, &["media", "add", "/nonexistent.mp4"]);
    assert_eq!(e["code"], "MEDIA");
    // No project at all.
    let out = Command::new(env!("CARGO_BIN_EXE_vidcut"))
        .env_remove("VIDCUT_PROJECT")
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
