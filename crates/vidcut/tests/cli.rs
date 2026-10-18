use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, Picture, Sound, VideoCodec};

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

    fn cmd_in(&self, project: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_vidcut"));
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
    assert_eq!(
        sb.v1(),
        vec![
            row("c1", 0, 30, 10),
            row("c3", 30, 10, 40),
            row("c2", 40, 20, 0),
            row("c4", 60, 70, 20)
        ]
    );

    sb.ok(&["edit", "delete", "V1@00:00:01:05"]);
    assert_eq!(
        sb.v1(),
        vec![
            row("c1", 0, 30, 10),
            row("c2", 30, 20, 0),
            row("c4", 50, 70, 20)
        ]
    );

    sb.ok(&["history", "undo", "2"]);
    assert_eq!(sb.v1().len(), 4);
    sb.ok(&["history", "redo"]);

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
    sb.fails(2, &["edit", "split", "--at", "30"]);
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

    std::fs::write(&batch, "[{\"method\": \"edit.append\", \"params\": {\"media\": \"m1\"}}, {\"method\": \"edit.delete\", \"params\": {\"clips\": \"c42\"}}]").unwrap();
    sb.fails(3, &["apply", s(&batch)]);
    assert!(sb.v1().is_empty());
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
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for c in children {
        let out = c.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}: {}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
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

fn barcodes(path: &Path) -> Vec<Option<u32>> {
    let mut r = vidcut_av::decode::VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        out.push(vidcut_av::decode::read_barcode(&f).unwrap());
    }
    out
}

#[test]
fn a_proxy_made_from_another_file_is_made_again() {
    let sb = Sandbox::new("proxy-origin");
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 30);
    let numbered = fixture(spec.clone());
    let black = fixture(spec.picture(Picture::Black));
    let out = sb
        .cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    sb.ok(&["media", "add", s(&numbered), s(&black)]);
    let proxy = |media: &str| {
        let r = sb.ok(&["proxy", "generate", media]);
        PathBuf::from(r["result"]["proxies"][0]["path"].as_str().unwrap())
    };
    let numbered_proxy = proxy("m1");
    std::fs::copy(proxy("m2"), &numbered_proxy).unwrap();
    assert_eq!(proxy("m1"), numbered_proxy);
    assert_eq!(
        barcodes(&numbered_proxy),
        (0..30).map(Some).collect::<Vec<_>>()
    );
}

#[test]
fn a_project_names_its_proxy_by_file_in_each_computers_folder() {
    let sb = Sandbox::new("proxy-file");
    let media = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ));
    let (here, there) = (sb.dir.path().join("here"), sb.dir.path().join("there"));
    sb.ok(&[
        "preferences",
        "set",
        &format!("proxies.folder={}", s(&here)),
    ]);
    sb.ok(&["project", "create", s(&sb.project())]);
    sb.ok(&["media", "add", s(&media)]);
    let r = sb.ok(&["proxy", "generate", "m1"]);
    let made = PathBuf::from(r["result"]["proxies"][0]["path"].as_str().unwrap());
    let name = made.file_name().unwrap().to_str().unwrap();
    std::fs::create_dir_all(&there).unwrap();
    std::fs::rename(&made, there.join(name)).unwrap();
    sb.ok(&[
        "preferences",
        "set",
        &format!("proxies.folder={}", s(&there)),
    ]);

    let proxy = sb.ok(&["media", "get", "m1"])["result"]["proxy"].clone();
    assert_eq!(proxy["path"].as_str(), Some(s(&there.join(name))));
    assert_eq!(proxy["missing"], Value::Null);
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(sb.project()).unwrap()).unwrap();
    assert_eq!(
        saved["media"][0]["proxy"]["file"].as_str(),
        Some(name),
        "{}",
        saved["media"][0]["proxy"]
    );
}

#[test]
fn projects_share_an_absolute_proxy_folder() {
    let sb = Sandbox::new("proxy-folder");
    let media = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ));
    let shared = sb.dir.path().join("shared");
    let folder = format!("proxies.folder={}", s(&shared));
    sb.ok(&["preferences", "set", &folder]);
    let proxy = |name: &str| {
        let project = sb.dir.path().join(name).join("p.vidcut");
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        sb.ok_in(&project, &["project", "create", s(&project)]);
        sb.ok_in(&project, &["media", "add", s(&media)]);
        let r = sb.ok_in(&project, &["proxy", "generate", "m1"]);
        PathBuf::from(r["result"]["proxies"][0]["path"].as_str().unwrap())
    };
    let first = proxy("a");
    let made = std::fs::metadata(&first).unwrap().modified().unwrap();
    let second = proxy("b");
    assert_eq!(second, first);
    assert_eq!(first.parent(), Some(shared.as_path()));
    assert_eq!(
        std::fs::metadata(&second).unwrap().modified().unwrap(),
        made
    );
}

#[test]
fn exports_frames_ranges_and_clips() {
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
    assert_eq!(
        barcodes(&out),
        (35..40).chain(50..55).map(Some).collect::<Vec<_>>()
    );
    sb.ok(&["marks", "clear"]);

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

    let png = sb.dir.path().join("f.png");
    sb.ok(&["frame", "render", "--at", "35f", "--out", s(&png)]);
    assert_eq!(barcodes(&png), vec![Some(55)]);
}

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
    let r = sb.ok(&["export", "start", s(&cut)]);
    assert_eq!(r["result"]["outputs"][0]["frames"], 110);

    let other = sb.dir.path().join("q.vidcut");
    sb.ok(&["project", "create", s(&other)]);
    let r = sb.ok_in(&other, &["timeline", "import", s(&cut)]);
    assert_eq!(r["result"]["warnings"], serde_json::json!([]));
    assert_eq!(
        timeline_shape(&sb, &other),
        timeline_shape(&sb, &sb.project())
    );
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

    let fixtures = a.parent().unwrap();
    let moved = sb.dir.path().join("moved.otio");
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&cut).unwrap()).unwrap();
    move_media(&mut doc);
    std::fs::write(&moved, doc.to_string()).unwrap();
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

fn clips_in(sb: &Sandbox, project: &Path) -> Vec<(i64, i64)> {
    let t = sb.ok_in(project, &["timeline", "get"]);
    t["result"]["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "clip")
        .map(|i| {
            (
                i["start"].as_i64().unwrap(),
                i["duration"].as_i64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn projects_come_from_otio_files() {
    let sb = Sandbox::new("from-otio");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (24, 1),
        96,
    ));
    sb.ok(&["project", "create", s(&sb.project())]);
    sb.ok(&["media", "add", s(&a)]);
    sb.ok(&["edit", "append", "m1", "--range", "0f..48f"]);
    sb.ok(&["edit", "append", "m1", "--range", "60f..84f"]);
    sb.ok(&["markers", "add", "--at", "10f", "--name", "look"]);
    let cut = sb.dir.path().join("cut.otio");
    sb.ok(&["export", "start", s(&cut)]);
    let path = |name: &str| sb.dir.path().join(name);

    let own = path("own.vidcut");
    let r = sb.ok(&["project", "create", s(&own), "--from", s(&cut)]);
    assert_eq!(r["result"]["warnings"], serde_json::json!([]));
    assert_eq!(
        sb.ok_in(&own, &["timeline", "get"])["result"],
        sb.ok(&["timeline", "get"])["result"]
    );
    let other = path("other.vidcut");
    sb.fails(
        2,
        &[
            "project",
            "create",
            s(&other),
            "--from",
            s(&cut),
            "--fps",
            "25",
        ],
    );
    sb.fails(4, &["project", "create", s(&own), "--from", s(&cut)]);

    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&cut).unwrap()).unwrap();
    doc["metadata"]["vidcut"]
        .as_object_mut()
        .unwrap()
        .remove("project");
    let theirs = path("theirs.otio");
    std::fs::write(&theirs, doc.to_string()).unwrap();
    let made = path("made.vidcut");
    let r = sb.ok(&["project", "create", s(&made), "--from", s(&theirs)]);
    let settings = &r["result"]["settings"];
    let rate = sb.ok(&["project", "summary"])["result"]["settings"]["fps"].clone();
    assert_eq!(
        (&settings["fps"], &settings["width"], &settings["height"]),
        (&rate, &serde_json::json!(320), &serde_json::json!(180))
    );
    assert_eq!(
        timeline_shape(&sb, &made),
        timeline_shape(&sb, &sb.project())
    );
    let at25 = path("at25.vidcut");
    let r = sb.ok(&[
        "project",
        "create",
        s(&at25),
        "--from",
        s(&theirs),
        "--fps",
        "25",
        "--width",
        "640",
        "--height",
        "360",
        "--start-tc",
        "01:00:00:00",
    ]);
    assert_eq!(r["result"]["settings"]["start_tc"], 90_000);
    assert_eq!(clips_in(&sb, &at25), [(0, 50), (50, 25)]);
}

#[test]
fn a_timeline_imported_as_a_compound_clip_plays_the_same() {
    let sb = Sandbox::new("compound-import");
    let a = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ));
    sb.ok(&["project", "create", s(&sb.project())]);
    sb.ok(&["media", "add", s(&a)]);
    sb.ok(&["edit", "append", "m1", "--range", "10f..40f"]);
    sb.ok(&["edit", "append", "m1", "--range", "60f..90f"]);
    let cut = sb.dir.path().join("cut.otio");
    sb.ok(&["export", "start", s(&cut)]);
    let other = sb.dir.path().join("q.vidcut");
    sb.ok(&["project", "create", s(&other)]);
    sb.ok_in(
        &other,
        &["timeline", "import", s(&cut), "--mode", "compound"],
    );
    let t = sb.ok_in(&other, &["timeline", "get"]);
    let items = t["result"]["tracks"][0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["duration"], 60);
    same_frames(&sb, &sb.project(), &other, &["0f", "29f", "30f", "59f"]);
}

fn move_media(doc: &mut Value) {
    match doc {
        Value::Object(fields) => {
            for (key, value) in fields.iter_mut() {
                if key == "target_url"
                    && let Some(path) = value.as_str()
                {
                    let name = Path::new(path).file_name().unwrap().to_string_lossy();
                    *value = format!("/nonexistent/footage/{name}").into();
                } else {
                    move_media(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(move_media),
        _ => {}
    }
}

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
    sb.ok(&["edit", "append", "m1", "--range", "10f..40f"]);
    sb.ok(&["edit", "append", "m2", "--range", "5f..35f"]);
    sb.ok(&["edit", "append", "m1", "--range", "50f..70f"]);
    sb.ok(&["edit", "audio-roll", "c1", "--edge", "tail", "--by", "3f"]);
    sb.ok(&["clip", "set", "c2", "--gain-db", "-4"]);

    let cut = sb.dir.path().join("cut.otio");
    sb.ok(&["export", "start", s(&cut)]);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&cut).unwrap()).unwrap();
    let first = &doc["tracks"]["children"][0]["children"][0];
    let reference = &first["media_references"]["DEFAULT_MEDIA"];
    assert_eq!(
        reference["available_range"]["start_time"]["value"],
        900_000.0
    );
    assert_eq!(first["source_range"]["start_time"]["value"], 900_010.0);

    let bundle = sb.dir.path().join("cut.otioz");
    sb.ok(&["export", "start", s(&bundle)]);
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

    let away = sb.dir.path().join("away");
    std::fs::create_dir_all(&away).unwrap();
    let moved = away.join("cut.otioz");
    std::fs::copy(&bundle, &moved).unwrap();
    let other = away.join("q.vidcut");
    sb.ok(&["project", "create", s(&other)]);
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
}

#[test]
fn scene_cuts_split_encoded_shots_exactly() {
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
    let sb = Sandbox::new("images");
    let still = fixture::ensure_images(&ImageSpec::still(77, 320, 180, "png")).unwrap();
    let seq = fixture::ensure_images(&ImageSpec::sequence(1, 60, 6, 320, 180)).unwrap();
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();
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
    let r = sb.ok(&["media", "add", s(&seq[0])]);
    assert_eq!(r["changes"]["created"], serde_json::json!(["m3"]));
}

#[test]
fn a_folder_comes_in_as_a_bin_skipping_what_isnt_media() {
    let sb = Sandbox::new("folder");
    let shoot = sb.dir.path().join("Shoot");
    for dir in ["Day 1", "Empty", "Frames", "Docs"] {
        std::fs::create_dir_all(shoot.join(dir)).unwrap();
    }
    let wide = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (25, 1),
        50,
    ));
    let take = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mkv,
        320,
        180,
        (30, 1),
        30,
    ));
    std::fs::copy(&wide, shoot.join("a-wide.mp4")).unwrap();
    std::fs::copy(&take, shoot.join("Day 1").join("take.mkv")).unwrap();
    for frame in fixture::ensure_images(&ImageSpec::sequence(1, 8, 4, 320, 180)).unwrap() {
        std::fs::copy(
            &frame,
            shoot.join("Frames").join(frame.file_name().unwrap()),
        )
        .unwrap();
    }
    std::fs::write(shoot.join("broken.mp4"), "not a video").unwrap();
    std::fs::write(shoot.join("Docs").join("readme.txt"), "readme").unwrap();
    sb.cmd(&["project", "create"])
        .arg(sb.project())
        .output()
        .unwrap();

    let r = sb.ok(&["media", "add-folder", s(&shoot), "--sequence"]);
    assert_eq!(r["result"]["bin"], "Shoot");
    assert_eq!(r["result"]["media"], serde_json::json!(["m1", "m2", "m3"]));
    let skipped: Vec<&str> = r["result"]["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["path"].as_str().unwrap())
        .collect();
    let canonical = |p: PathBuf| vidcut_core::dirs::canonical(&p).unwrap();
    assert_eq!(
        skipped,
        [
            s(&canonical(shoot.join("broken.mp4"))),
            s(&canonical(shoot.join("Docs").join("readme.txt"))),
        ]
    );
    assert_eq!(
        sb.ok(&["bins", "list"])["result"],
        serde_json::json!(["Shoot", "Shoot/Day 1", "Shoot/Frames"])
    );
    let m3 = sb.ok(&["media", "get", "m3"])["result"].clone();
    assert_eq!(
        (&m3["name"], &m3["bin"]),
        (&"img_[0001-0008].png".into(), &"Shoot/Frames".into())
    );
    let info = sb.ok(&["project", "summary"]);
    assert_eq!(info["result"]["settings"]["fps"], "25", "{info}");

    sb.ok(&["history", "undo"]);
    assert_eq!(sb.ok(&["media", "list"])["result"], serde_json::json!([]));
    assert_eq!(sb.ok(&["bins", "list"])["result"], serde_json::json!([]));
}

#[test]
fn reset_deletes_settings_caches_and_state_but_keeps_presets() {
    let s = Sandbox::new("reset");
    let d = s.dir.path();
    let files = [
        "config/vidcut/preferences.toml",
        "config/vidcut/presets/mine.toml",
        "cache/vidcut/thumbs/a.jpg",
        "state/vidcut/recent.json",
    ];
    for f in files {
        std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
        std::fs::write(d.join(f), "x").unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_vidcut"))
        .env("XDG_STATE_HOME", d.join("state"))
        .env("XDG_RUNTIME_DIR", d.join("run"))
        .env("XDG_CONFIG_HOME", d.join("config"))
        .env("XDG_CACHE_HOME", d.join("cache"))
        .env_remove("VIDCUT_SOCKET")
        .env_remove("VIDCUT_PROJECT")
        .args(["--json", "reset"])
        .output()
        .expect("run vidcut");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        files.map(|f| d.join(f).exists()),
        [false, true, false, false]
    );
}

#[cfg(unix)]
#[test]
fn reset_refuses_while_another_gui_runs() {
    let s = Sandbox::new("reset-gui");
    let d = s.dir.path();
    let instances = d.join("run/vidcut/instances");
    std::fs::create_dir_all(&instances).unwrap();
    let socket = instances.join("gui.sock");
    std::fs::write(&socket, "").unwrap();
    let me = std::process::id();
    std::fs::write(
        instances.join(format!("{me}.json")),
        serde_json::json!({ "pid": me, "socket": socket, "project": null, "version": "test" })
            .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(d.join("config/vidcut")).unwrap();
    std::fs::write(d.join("config/vidcut/preferences.toml"), "x").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vidcut"))
        .env("XDG_STATE_HOME", d.join("state"))
        .env("XDG_RUNTIME_DIR", d.join("run"))
        .env("XDG_CONFIG_HOME", d.join("config"))
        .env("XDG_CACHE_HOME", d.join("cache"))
        .env_remove("VIDCUT_SOCKET")
        .env_remove("VIDCUT_PROJECT")
        .args(["--json", "reset"])
        .output()
        .expect("run vidcut");
    assert_eq!(
        (
            out.status.code(),
            d.join("config/vidcut/preferences.toml").exists()
        ),
        (Some(5), true)
    );
}
