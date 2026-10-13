//! A smaller FFmpeg build, as `VIDCUT_FFMPEG_WITHOUT` makes one: `doctor`
//! names what it lacks, and an export that needs a missing part fails as
//! `DEPENDENCY` before it writes anything; one that doesn't runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Hdr;

/// A scratch directory with its own XDG dirs and a project, removed when it
/// drops.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("vidcut-parts-{name}-"))
            .tempdir()
            .unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `vidcut --json -p p.vidcut ARGS` with the parts in `without` missing:
    /// its exit code and JSON.
    fn run(&self, without: &str, args: &[&str]) -> (Option<i32>, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_vidcut"))
            .env("VIDCUT_FFMPEG_WITHOUT", without)
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
        let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
            panic!(
                "vidcut {args:?}: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code(), json)
    }

    /// A project of `media`, all of it on the timeline.
    fn project(&self, media: &Path) {
        for args in [
            &["project", "create", self.path("p.vidcut").to_str().unwrap()][..],
            &["media", "add", media.to_str().unwrap()],
            &["edit", "append", "m1"],
        ] {
            let (code, v) = self.run("", args);
            assert_eq!(code, Some(0), "{args:?}: {v}");
        }
    }

    /// `export start OUT` on the CPU, quickly: its exit code and JSON.
    fn export(&self, without: &str, out: &Path) -> (Option<i32>, Value) {
        let args = [
            "export",
            "start",
            out.to_str().unwrap(),
            "--encoder",
            "cpu",
            "--set",
            "video.speed=ultrafast",
        ];
        self.run(without, &args)
    }
}

fn sdr() -> PathBuf {
    fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ))
    .unwrap()
}

/// A `DEPENDENCY` failure's message.
fn dependency(code: Option<i32>, v: &Value) -> String {
    assert_eq!(code, Some(10), "{v}");
    assert_eq!(v["error"]["code"], "DEPENDENCY", "{v}");
    v["error"]["message"].as_str().unwrap().to_string()
}

#[test]
fn doctor_names_what_the_build_lacks() {
    let sb = Sandbox::new("doctor");
    let (code, v) = sb.run("zscale, libx264", &["doctor"]);
    assert_eq!(code, Some(0), "{v}");
    let missing = v["result"]["ffmpeg_missing"].as_array().unwrap();
    let part = |name: &str| missing.iter().find(|p| p["name"] == name).cloned();
    let z = part("zscale").expect("zscale");
    assert_eq!(
        (&z["kind"], &z["library"], &z["used_for"]),
        (&"filter".into(), &"zimg".into(), &"HDR conversion".into())
    );
    assert_eq!(part("libx264").expect("libx264")["library"], "x264");
}

#[test]
fn an_export_without_its_encoder_fails_before_writing() {
    let sb = Sandbox::new("encoder");
    sb.project(&sdr());
    let out = sb.path("out.mp4");
    let (code, v) = sb.export("libx264", &out);
    let message = dependency(code, &v);
    assert!(
        message.contains("no libx264 encoder") && message.contains("x264"),
        "{message}"
    );
    assert!(!out.exists());
}

#[test]
fn hdr_into_sdr_without_zscale_fails_before_writing_and_sdr_still_exports() {
    let pq = fixture::ensure(
        &FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 30).hdr(Hdr::Pq),
    )
    .unwrap();
    let sb = Sandbox::new("zscale");
    sb.project(&pq);
    // H.264 can't carry HDR: the picture is tone-mapped into SDR.
    let out = sb.path("out.mp4");
    let (code, v) = sb.export("zscale", &out);
    let message = dependency(code, &v);
    assert!(
        message.contains("30 frames change between HDR and SDR")
            && message.contains("no zscale filter")
            && message.contains("zimg"),
        "{message}"
    );
    assert!(!out.exists());

    // SDR into SDR needs no zscale.
    let sb = Sandbox::new("sdr");
    sb.project(&sdr());
    let out = sb.path("out.mp4");
    let (code, v) = sb.export("zscale", &out);
    assert_eq!(code, Some(0), "{v}");
    assert!(out.is_file());
}
