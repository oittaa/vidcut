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
    let z = v["result"]["ffmpeg_missing"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["names"][0]["name"] == "zscale")
        .cloned()
        .expect("zscale");
    assert_eq!(
        (&z["kind"], &z["names"][0]["library"], &z["used_for"]),
        (
            &"filter".into(),
            &"zimg".into(),
            &"faster HDR conversion".into()
        )
    );
    // H.264 has another encoder where this FFmpeg has one.
    if vidcut_av::parts::has_encoder("libopenh264") {
        assert_eq!(
            v["result"]["ffmpeg_stand_ins"][0],
            serde_json::json!({
                "used_for": "H.264 on the CPU",
                "name": "libopenh264",
                "instead_of": "libx264",
            }),
            "{v}"
        );
    }
}

#[test]
fn an_export_without_its_encoder_fails_before_writing() {
    let sb = Sandbox::new("encoder");
    sb.project(&sdr());
    let out = sb.path("out.mp4");
    let (code, v) = sb.export("libx264,libopenh264", &out);
    let message = dependency(code, &v);
    assert_eq!(
        message,
        format!(
            "{}: this FFmpeg has no encoder for H.264 on the CPU \
             (libx264 from x264 or libopenh264 from OpenH264)",
            out.display()
        )
    );
    assert!(!out.exists());
}

/// Where the usual encoder is missing, another this FFmpeg has encodes;
/// each case runs where this FFmpeg has its stand-in.
#[test]
fn stand_ins_encode_where_the_usual_encoders_are_missing() {
    let sb = Sandbox::new("stand-ins");
    sb.project(&sdr());
    let cases: [(&str, &str, &[&str], &str); 6] = [
        ("libx264", "h264.mp4", &[], "libopenh264"),
        // Kvazaar takes sizes in multiples of 8 only.
        (
            "libx265",
            "hevc.mkv",
            &["video.codec=hevc", "video.width=320", "video.height=176"],
            "libkvazaar",
        ),
        ("libsvtav1", "aom.mkv", &["video.codec=av1"], "libaom-av1"),
        (
            "libsvtav1,libaom-av1",
            "rav1e.mkv",
            &["video.codec=av1"],
            "librav1e",
        ),
        (
            "prores_ks",
            "prores.mov",
            &["video.codec=prores"],
            "prores_aw",
        ),
        ("libopus", "opus.ogg", &[], "opus"),
    ];
    for (without, file, sets, stand_in) in cases {
        if !vidcut_av::parts::has_encoder(stand_in) {
            eprintln!("skipped: this FFmpeg has no {stand_in}");
            continue;
        }
        let out = sb.path(file);
        let mut args = vec!["export", "start", out.to_str().unwrap(), "--encoder", "cpu"];
        for set in sets {
            args.extend(["--set", set]);
        }
        let (code, v) = sb.run(without, &args);
        assert_eq!(code, Some(0), "{without}: {v}");
        let report = &v["result"]["outputs"][0];
        let used = [&report["video_encoder"], &report["audio_encoder"]];
        assert!(used.contains(&&stand_in.into()), "{without}: {v}");
        assert!(out.is_file(), "{without}");
    }
}

/// Without a software H.264 encoder, proxies are ProRes Proxy, which every
/// FFmpeg has.
#[test]
fn proxies_fall_back_to_prores() {
    let sb = Sandbox::new("proxy");
    sb.project(&sdr());
    // Software proxies only: no GPU's AV1 first.
    let (code, v) = sb.run("", &["preferences", "set", "proxies.codec=h264"]);
    assert_eq!(code, Some(0), "{v}");
    let (code, v) = sb.run("libx264,libopenh264", &["proxy", "generate"]);
    assert_eq!(code, Some(0), "{v}");
    let (_, m) = sb.run("", &["media", "get", "m1"]);
    assert_eq!(m["result"]["proxy"]["codec"], "prores", "{m}");
    assert_eq!(m["result"]["proxy"]["stale"], false, "{m}");
}

#[test]
fn hdr_into_sdr_without_zscale_converts_on_the_cpu() {
    let pq = fixture::ensure(
        &FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 320, 180, (30, 1), 30).hdr(Hdr::Pq),
    )
    .unwrap();
    let sb = Sandbox::new("zscale");
    sb.project(&pq);
    // H.264 can't carry HDR: the picture is tone-mapped into SDR.
    let out = sb.path("out.mp4");
    let (code, v) = sb.export("zscale", &out);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["result"]["outputs"][0]["color"]["tone_mapped"], 30, "{v}");
    assert!(out.is_file());
}
