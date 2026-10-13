//! The export's dynamic range and bit depth chosen from the pictures it
//! shows (decision D44), through the `vidcut` binary: plans, notes,
//! per-clip files and explicit values.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Hdr;

/// A scratch directory with its own XDG dirs, removed when it drops.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("vidcut-color-{name}-"))
            .tempdir()
            .unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Runs `vidcut --json ARGS` on the sandbox's project; expects success.
    fn ok(&self, args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_vidcut"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env_remove("VIDCUT_SOCKET")
            .env_remove("VIDCUT_PROJECT")
            .arg("--json")
            .arg("-p")
            .arg(self.path("p.vidcut"))
            .args(args)
            .output()
            .expect("run vidcut");
        assert!(
            out.status.success(),
            "vidcut {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("JSON")
    }

    /// `export plan OUT ARGS…` on the CPU encoders: the result.
    fn plan(&self, out: &str, args: &[&str]) -> Value {
        let out = self.path(out);
        let mut all = vec!["export", "plan", out.to_str().unwrap(), "--encoder", "cpu"];
        all.extend_from_slice(args);
        self.ok(&all)["result"].clone()
    }
}

fn fixture(spec: FixtureSpec) -> String {
    fixture::ensure(&spec)
        .expect("fixture")
        .to_str()
        .unwrap()
        .to_string()
}

/// `hdr` and `bit_depth` of each output's color.
fn colors(plan: &Value) -> Vec<(String, u64)> {
    plan["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            (
                o["color"]["hdr"].as_str().unwrap().to_string(),
                o["color"]["bit_depth"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn color(plan: &Value) -> (String, u64) {
    colors(plan).remove(0)
}

fn pair(hdr: &str, depth: u64) -> (String, u64) {
    (hdr.to_string(), depth)
}

#[test]
fn auto_color_follows_the_shown_picture() {
    let sb = Sandbox::new("auto");
    let hevc10 = || FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 60);
    let pq = fixture(hevc10().hdr(Hdr::Pq));
    let deep = fixture(hevc10());
    let sdr = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        60,
    ));
    sb.ok(&["project", "create", sb.path("p.vidcut").to_str().unwrap()]);
    sb.ok(&["media", "add", &pq, &sdr, &deep]);
    // c1 PQ 2 s, c2 SDR 1 s: mostly HDR.
    sb.ok(&["edit", "append", "m1"]);
    sb.ok(&["edit", "append", "m2", "--range", "0f..30f"]);

    // HEVC carries it: PQ in 10 bits, nothing to note.
    let plan = sb.plan("master.mov", &["--preset", "h265-master"]);
    assert_eq!(color(&plan), pair("pq", 10));
    assert_eq!(
        plan["outputs"][0]["color"]["picture"],
        json!({"frames": 90, "pq": 60, "hlg": 0, "deep": 60})
    );
    assert_eq!(plan["notes"], json!([]));
    // H.264 can't: SDR in 8 bits, and a note that says how to keep it.
    let plan = sb.plan("web.mp4", &[]);
    assert_eq!(color(&plan), pair("sdr", 8));
    assert_eq!(plan["outputs"][0]["color"]["tone_mapped"], 60);
    assert_eq!(plan["notes"][0]["code"], "hdr_tone_mapped");
    assert_eq!(
        plan["notes"][0]["set"],
        json!(["video.codec=hevc", "video.hdr=pq"])
    );
    assert!(
        plan["notes"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("2.0 s of HDR picture"),
        "{}",
        plan["notes"][0]
    );
    // TikTok asks for SDR: the facts, but no note.
    let plan = sb.plan("phone.mp4", &["--preset", "tiktok"]);
    assert_eq!(color(&plan), pair("sdr", 8));
    assert_eq!(plan["notes"], json!([]));

    // The export does what the plan says.
    let out = sb.path("master.mkv");
    let r = sb.ok(&[
        "export",
        "start",
        out.to_str().unwrap(),
        "--preset",
        "h265-master",
        "--encoder",
        "cpu",
        "--set",
        "video.speed=ultrafast",
    ]);
    assert_eq!(r["result"]["outputs"][0]["color"]["hdr"], "pq");
    let info = vidcut_av::probe::probe(&out).unwrap().video.unwrap();
    assert_eq!(info.transfer, "smpte2084");
    assert_eq!(info.pix_fmt, "yuv420p10le");

    // c3 SDR 2 s more: HDR is now 60 of 150 frames, under half. SDR, and
    // as most of the picture is 8-bit, 8 bits.
    sb.ok(&["edit", "append", "m2"]);
    let plan = sb.plan("master.mov", &["--preset", "h265-master"]);
    assert_eq!(color(&plan), pair("sdr", 10), "H.265 Master is Main10");
    assert_eq!(
        sb.plan("v.webm", &[])["outputs"][0]["color"]["bit_depth"],
        8
    );
    assert_eq!(plan["notes"][0]["set"], json!(["video.hdr=pq"]));
    // Explicit values win.
    let plan = sb.plan(
        "master.mov",
        &["--preset", "h265-master", "--set", "video.hdr=sdr"],
    );
    assert_eq!(color(&plan), pair("sdr", 10));
    assert_eq!(plan["notes"], json!([]));
    let plan = sb.plan(
        "master.mov",
        &["--preset", "h265-master", "--set", "video.hdr=hlg"],
    );
    assert_eq!(color(&plan), pair("hlg", 10));

    // Per clip, each file decides from its own clip.
    let plan = sb.plan("clips", &["--per-clip", "--preset", "av1"]);
    assert_eq!(
        colors(&plan),
        [pair("pq", 10), pair("sdr", 10), pair("sdr", 10)]
    );
    let plan = sb.plan("clips", &["--per-clip", "--preset", "vp9-webm"]);
    assert_eq!(
        colors(&plan),
        [pair("pq", 10), pair("sdr", 8), pair("sdr", 8)]
    );
    assert_eq!(plan["notes"], json!([]));
    let plan = sb.plan("clips", &["--per-clip"]);
    assert_eq!(
        colors(&plan),
        [pair("sdr", 8), pair("sdr", 8), pair("sdr", 8)]
    );
    assert!(
        plan["notes"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("HDR picture in 1 of 3 files"),
        "{}",
        plan["notes"][0]
    );

    // 10-bit SDR: 10 bits where they play widely, 8 in H.264.
    sb.ok(&["edit", "append", "m3"]);
    let range = ["--range", "150f..210f"];
    let plan = sb.plan("deep.webm", &range);
    assert_eq!(color(&plan), pair("sdr", 10));
    let plan = sb.plan("deep.mp4", &range);
    assert_eq!(color(&plan), pair("sdr", 8));
    let mut args = range.to_vec();
    args.extend(["--preset", "dnxhr-hq"]);
    let plan = sb.plan("deep.mov", &args);
    assert_eq!(color(&plan), pair("sdr", 10));
    args.extend(["--set", "video.profile=sq"]);
    assert_eq!(color(&sb.plan("deep.mov", &args)), pair("sdr", 8));
    let out = sb.path("deep.mkv");
    sb.ok(&[
        "export",
        "start",
        out.to_str().unwrap(),
        "--range",
        "150f..210f",
        "--set",
        "video.codec=hevc",
        "--set",
        "video.speed=ultrafast",
        "--encoder",
        "cpu",
    ]);
    let info = vidcut_av::probe::probe(&out).unwrap().video.unwrap();
    assert_eq!(info.pix_fmt, "yuv420p10le");
    assert_ne!(info.transfer, "smpte2084");
}

/// `presets get` shows `auto` for the masters and `sdr` for TikTok.
#[test]
fn presets_show_their_dynamic_range() {
    let sb = Sandbox::new("presets");
    let presets = sb.ok(&["presets", "list"]);
    let hdr = |name: &str| {
        presets["result"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap()["settings"]["video"]["hdr"]
            .clone()
    };
    assert_eq!(hdr("h265-master"), "auto");
    assert_eq!(hdr("tiktok"), "sdr");
}

#[test]
fn hdr_metadata_follows_the_sources() {
    // Decision D46, through the binary: the plan says what an export writes,
    // the export writes it, and media probed before vidcut kept the values
    // are probed again.
    use vidcut_av::fixture::HdrMeta;
    let sb = Sandbox::new("metadata");
    let plus = fixture(
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 30)
            .hdr(Hdr::Pq)
            .hdr_meta(HdrMeta {
                max_nits: 1000,
                light: Some((1000, 400)),
                hdr10plus: true,
            })
            .no_audio(),
    );
    let project = sb.path("p.vidcut");
    sb.ok(&["project", "create", project.to_str().unwrap()]);
    sb.ok(&["media", "add", &plus]);
    sb.ok(&["edit", "append", "m1"]);
    let media = sb.ok(&["media", "get", "m1"])["result"].clone();
    assert_eq!(media["info"]["video"]["hdr10plus"], true);
    assert_eq!(
        media["info"]["video"]["light"],
        json!({"max_cll": 1000, "max_fall": 400})
    );

    // As a project from before: the values gone from the file.
    let mut file: Value =
        serde_json::from_str(&std::fs::read_to_string(&project).unwrap()).unwrap();
    let v = &mut file["media"][0]["info"]["video"];
    for k in ["mastering", "light", "hdr10plus"] {
        v.as_object_mut().unwrap().remove(k);
    }
    std::fs::write(&project, file.to_string()).unwrap();
    assert!(sb.ok(&["media", "get", "m1"])["result"]["info"]["video"]["light"].is_null());

    let plan = sb.plan("master.mkv", &["--preset", "h265-master"]);
    let m = &plan["outputs"][0]["metadata"];
    assert_eq!(m["mode"], "hdr10plus", "{m}");
    assert_eq!(m["light"], json!({"max_cll": 1000, "max_fall": 400}));
    // The settings the encoder runs with, and none asked for.
    let plan = sb.plan(
        "plain.mkv",
        &[
            "--preset",
            "h265-master",
            "--set",
            "video.hdr_metadata=none",
        ],
    );
    assert_eq!(plan["outputs"][0]["metadata"]["mode"], "none");

    let out = sb.path("master.mkv");
    let r = sb.ok(&[
        "export",
        "start",
        out.to_str().unwrap(),
        "--preset",
        "h265-master",
        "--encoder",
        "cpu",
        "--set",
        "video.speed=ultrafast",
    ]);
    assert_eq!(
        r["result"]["outputs"][0]["metadata"]["mode"], "hdr10plus",
        "{r}"
    );
    let frames = vidcut_av::hdr::read_frames(&out, false, usize::MAX).unwrap();
    assert_eq!(frames.len(), 30);
    assert!(frames.iter().enumerate().all(|(k, f)| {
        f.hdr10plus.as_deref() == Some(fixture::hdr10plus_t35(k as u32).as_slice())
            && f.metadata.light.is_some_and(|l| l.max_cll == 1000)
    }));
}
