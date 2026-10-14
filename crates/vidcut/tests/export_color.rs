//! The export's dynamic range and bit depth chosen from the pictures it
//! shows (decision D44), through the `vidcut` binary: plans, exports and
//! explicit values.

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
    let sdr = fixture(FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        60,
    ));
    sb.ok(&["project", "create", sb.path("p.vidcut").to_str().unwrap()]);
    sb.ok(&["media", "add", &pq, &sdr]);
    // c1 PQ 2 s, c2 SDR 1 s: mostly HDR.
    sb.ok(&["edit", "append", "m1"]);
    sb.ok(&["edit", "append", "m2", "--range", "0f..30f"]);

    // H.264 can't carry HDR: the plan tone-maps it into SDR in 8 bits.
    let plan = sb.plan("web.mp4", &[]);
    assert_eq!(color(&plan), pair("sdr", 8));
    assert_eq!(plan["outputs"][0]["color"]["tone_mapped"], 60);

    // H.265 Master exports PQ in 10 bits.
    let out = sb.path("master.mkv");
    sb.ok(&[
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
    let info = vidcut_av::probe::probe(&out).unwrap().video.unwrap();
    assert_eq!(info.transfer, "smpte2084");
    assert_eq!(info.pix_fmt, "yuv420p10le");

    // Explicit values win.
    let plan = sb.plan(
        "master.mov",
        &["--preset", "h265-master", "--set", "video.hdr=sdr"],
    );
    assert_eq!(color(&plan), pair("sdr", 10));
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
                hdr10plus_frames: None,
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
