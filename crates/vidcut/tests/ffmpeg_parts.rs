//! A smaller FFmpeg build, as `VIDCUT_FFMPEG_WITHOUT` makes one: an export
//! that needs a missing part fails as `DEPENDENCY` before it writes anything.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};

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

#[test]
fn an_export_without_its_encoder_fails_before_writing() {
    let sb = Sandbox::new("encoder");
    sb.project(&sdr());
    let out = sb.path("out.mp4");
    let (code, v) = sb.export("libx264,libopenh264", &out);
    assert_eq!(code, Some(10), "{v}");
    assert_eq!(v["error"]["code"], "DEPENDENCY", "{v}");
    assert!(!out.exists());
}
