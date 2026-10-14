//! Checks against the real GUI (needs a Wayland or X11 display and a GPU
//! wgpu draws on, Vulkan on Linux; run with `--features vidcut/gui-tests`).
//! Starts `vidcut -p FILE`, drives it through the CLI like an agent would,
//! and checks:
//! - after `seek`, the viewer screenshot (the GPU shader's picture) shows
//!   exactly the frame `vidcut frame render` renders headless, turned
//!   pictures and HEVC on NVDEC among them;
//! - a CLI edit is applied by the GUI and shows in its window;
//! - live control keeps working while the window is minimized;
//! - GPU acceleration, and GPU decoding alone, switch the viewer's decoder
//!   at once.
//!
//! The session must be unlocked: the window screenshots need the window
//! shown (a hidden window doesn't paint on every system); the viewer's work
//! without it.
//!
//! A failed run keeps its temp dir (it says which): the screenshots and the
//! GUI's stderr with `VIDCUT_DEBUG_PLAYER` traces.
#![cfg(feature = "gui-tests")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};

const BIN: &str = env!("CARGO_BIN_EXE_vidcut");

struct Gui {
    child: Child,
    /// Removed when the run passes; kept as evidence when it fails.
    dir: tempfile::TempDir,
    project: PathBuf,
}

impl Gui {
    /// Runs a CLI command against this GUI; returns the JSON response.
    fn cli(&self, args: &[&str]) -> Value {
        let out = Command::new(BIN)
            .args(["--json", "--instance", &self.child.id().to_string(), "-p"])
            .arg(&self.project)
            .args(args)
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
        assert!(out.status.success(), "{args:?}: {v}");
        v
    }

    fn barcode(&self, png: &Path) -> Option<u32> {
        let mut r = vidcut_av::decode::VideoReader::open(png).unwrap();
        vidcut_av::decode::read_barcode(&r.next_frame().unwrap().unwrap()).unwrap()
    }

    /// Seeks the timeline, screenshots the viewer, and compares with a
    /// headless `vidcut frame render`.
    fn check_frame(&self, frame: i64) {
        let at = format!("{frame}f");
        self.cli(&["player", "seek", &at, "--viewer", "timeline"]);
        let shot = self.dir.path().join(format!("shot-{frame}.png"));
        self.cli(&["gui", "screenshot", "--out", shot.to_str().unwrap()]);
        let reference = self.dir.path().join(format!("frame-{frame}.png"));
        let out = Command::new(BIN)
            .args(["--headless", "-p"])
            .arg(&self.project)
            .args([
                "frame",
                "render",
                "--at",
                &at,
                "--out",
                reference.to_str().unwrap(),
            ])
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            self.barcode(&shot),
            self.barcode(&reference),
            "timeline frame {frame}: viewer vs vidcut frame render"
        );
    }

    /// Differing pixels between two window screenshots (RGBA via PNG).
    fn window_diff(&self, a: &Path, b: &Path) -> usize {
        let read = |p: &Path| {
            let mut r = vidcut_av::decode::VideoReader::open(p).unwrap();
            let f = r.next_frame().unwrap().unwrap();
            (f.width(), f.height(), f.stride(0), f.data(0).to_vec())
        };
        let (wa, ha, sa, da) = read(a);
        let (wb, hb, sb, db) = read(b);
        assert_eq!((wa, ha), (wb, hb));
        let mut n = 0;
        for y in 0..ha as usize {
            for x in 0..wa as usize * 4 {
                if da[y * sa + x] != db[y * sb + x] {
                    n += 1;
                }
            }
        }
        n
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            self.dir.disable_cleanup(true);
            eprintln!(
                "kept {}: screenshots, the GUI's stderr",
                self.dir.path().display()
            );
        }
    }
}

#[test]
fn gui_acceptance() {
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        640,
        360,
        (30, 1),
        300,
    ))
    .unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-")
        .tempdir()
        .unwrap();
    let dir = tmp.path().to_path_buf();
    let project = dir.join("accept.vidcut");
    let headless = |args: &[&str]| {
        let out = Command::new(BIN)
            .args(["--headless", "-p"])
            .arg(&project)
            .args(args)
            .env("XDG_STATE_HOME", dir.join("state"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let out = Command::new(BIN)
        .args(["project", "create"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success());
    // A phone-style clip, stored sideways with a rotation: the shader turns
    // it upright, like export.
    let rotated = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 60)
            .rotation(90)
            .no_audio(),
    )
    .unwrap();
    // 10-bit HEVC: P010 pictures, decoded on NVDEC where there is one.
    let hevc = fixture::ensure(
        &FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, 640, 360, (30, 1), 60).no_audio(),
    )
    .unwrap();
    headless(&["media", "add", media.to_str().unwrap()]);
    headless(&["media", "add", rotated.to_str().unwrap()]);
    headless(&["media", "add", hevc.to_str().unwrap()]);
    headless(&["edit", "append", "m1", "--range", "0f..90f"]);
    headless(&["edit", "append", "m1", "--range", "200f..260f"]);
    headless(&["edit", "append", "m2", "--range", "10f..30f"]);
    headless(&["edit", "append", "m3", "--range", "5f..35f"]);

    // Evidence for a failed run, kept in `dir`.
    let stderr = std::fs::File::create(dir.join("stderr.log")).unwrap();
    let child = Command::new(BIN)
        .arg("-p")
        .arg(&project)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("VIDCUT_DEBUG_PLAYER", "1")
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let pid = child.id();
    let gui = Gui {
        child,
        dir: tmp,
        project: project.clone(),
    };
    let started = Instant::now();
    while !vidcut::ipc::registry::list()
        .iter()
        .any(|i| i.pid == pid && i.project.is_some())
    {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "GUI did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // A fixed window size keeps screenshots comparable.
    gui.cli(&["gui", "window", "--size", "1280,800"]);
    std::thread::sleep(Duration::from_millis(500));

    // 1. Seek + screenshot shows exactly what `vidcut frame render` renders.
    // Frames 150..170 are the rotated clip, 170..200 the HEVC one.
    for f in [0, 1, 45, 89, 90, 91, 149, 150, 155, 169, 170, 185, 199] {
        gui.check_frame(f);
    }

    // 2. A CLI edit is applied by the GUI (the command returns after the GUI
    // saved it) and the window shows it.
    let before = dir.join("before.png");
    gui.cli(&[
        "gui",
        "screenshot",
        "--target",
        "window",
        "--out",
        before.to_str().unwrap(),
    ]);
    let r = gui.cli(&["edit", "split", "--at", "45f"]);
    assert!(r["via"].as_str().unwrap().starts_with("gui:"), "{r}");
    let after = dir.join("after.png");
    gui.cli(&[
        "gui",
        "screenshot",
        "--target",
        "window",
        "--out",
        after.to_str().unwrap(),
    ]);
    assert!(
        gui.window_diff(&before, &after) > 0,
        "the window doesn't show the split"
    );
    let tl = gui.cli(&["timeline", "get"]);
    let clips = tl["result"]["tracks"][0]["items"].as_array().unwrap().len();
    assert_eq!(clips, 5, "{tl}");

    // 3. Minimized, the GUI still answers, seeks and renders frame-exactly.
    gui.cli(&["gui", "window", "--minimized", "true"]);
    std::thread::sleep(Duration::from_millis(500));
    gui.check_frame(100);
    let r = gui.cli(&["edit", "split", "--at", "120f"]);
    assert!(
        r["changes"]["created"]
            .as_array()
            .is_some_and(|c| c.len() == 1)
    );
    gui.check_frame(121);

    gui.cli(&["gui", "window", "--minimized", "false"]);
    std::thread::sleep(Duration::from_millis(300));

    // 4. Turning GPU acceleration off, from anywhere (here the CLI, which
    // writes the preferences file), makes the preview decode in software at
    // once, still frame-exactly; back on, as it did. The HEVC clip decodes
    // on NVDEC where there is one.
    gui.check_frame(180);
    let hwdec = |gui: &Gui| {
        let st = gui.cli(&["player", "state"]);
        st["result"]["timeline"]["hwdec"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let wait_for = |gui: &Gui, want: &dyn Fn(&str) -> bool| {
        let started = Instant::now();
        loop {
            let now = hwdec(gui);
            if want(&now) {
                return now;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "hwdec stays {now:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let before = hwdec(&gui);
    eprintln!("the HEVC clip decodes on {before:?}");
    if vidcut_av::cuda::CudaDevice::open().is_ok() {
        assert_eq!(before, "nvdec");
    }
    gui.cli(&["preferences", "set", "gpu.acceleration=false"]);
    wait_for(&gui, &|h| h == "no");
    gui.check_frame(180);
    gui.check_frame(95);
    gui.cli(&["preferences", "set", "gpu.acceleration=true"]);
    gui.check_frame(181);
    wait_for(&gui, &|h| h == before);
    gui.check_frame(182);
    // Decoding alone off: the CPU decodes, the GPU on for the rest.
    gui.cli(&["preferences", "set", "gpu.decoding=false"]);
    wait_for(&gui, &|h| h == "no");
    gui.check_frame(183);
    gui.cli(&["preferences", "set", "gpu.decoding=true"]);
    wait_for(&gui, &|h| h == before);
    gui.check_frame(184);
    // Each GPU chosen from anywhere: the player opens on its decoder where
    // it stood, still frame-exact; cleared, the automatic one again.
    let prefs = gui.cli(&["preferences", "get"]);
    let gpus = prefs["result"]["gpus"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (i, g) in gpus.iter().enumerate() {
        let kind = match g["vendor"].as_str() {
            Some("NVIDIA") => "nvdec",
            _ if cfg!(windows) => "d3d11va",
            _ => "vaapi",
        };
        let id = g["id"].as_str().unwrap_or_default();
        gui.cli(&["preferences", "set", &format!("gpu.device={id}")]);
        wait_for(&gui, &|h| h == kind);
        gui.check_frame(188 + i as i64);
    }
    if !gpus.is_empty() {
        gui.cli(&["preferences", "set", "gpu.device="]);
        wait_for(&gui, &|h| h == before);
        gui.check_frame(184);
    }

    // An audio device chosen from anywhere: the player opens on it where it
    // stood, still frame-exact; one that isn't connected plays on the
    // default, saying so; cleared, the system's default again.
    let default = vidcut_av::player::output_devices()
        .ok()
        .and_then(|d| d.into_iter().find(|d| d.default));
    if let Some(d) = default {
        let audio = |gui: &Gui| {
            let st = gui.cli(&["player", "state"]);
            let r = &st["result"];
            (
                r["audio_device"].clone(),
                r["audio_error"].as_str().map(str::to_string),
            )
        };
        let wait_audio = |gui: &Gui, want: serde_json::Value, error: Option<&str>| {
            let started = Instant::now();
            loop {
                let (device, e) = audio(gui);
                let told = match (error, e.as_deref()) {
                    (None, None) => true,
                    (Some(want), Some(e)) => e.contains(want),
                    _ => false,
                };
                if device == want && told {
                    return;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "the player stays on {device:?} ({e:?})"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        gui.cli(&[
            "preferences",
            "set",
            &format!("player.audio_device={}", d.id),
        ]);
        wait_audio(&gui, serde_json::json!(d.id), None);
        gui.check_frame(185);
        gui.cli(&["preferences", "set", "player.audio_device=wasapi:{gone}"]);
        wait_audio(&gui, serde_json::Value::Null, Some("isn't connected"));
        gui.check_frame(186);
        gui.cli(&["preferences", "set", "player.audio_device="]);
        wait_audio(&gui, serde_json::Value::Null, None);
        gui.check_frame(187);
    }

    // Quit cleanly: the instance unregisters.
    gui.cli(&["gui", "quit"]);
    let started = Instant::now();
    while vidcut::ipc::registry::list().iter().any(|i| i.pid == pid) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "GUI did not quit"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
