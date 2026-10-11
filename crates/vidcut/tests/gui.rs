//! Checks against the real GUI (needs a Wayland or X11 display and
//! libmpv; run with `--features vidcut/gui-tests`). Starts `vidcut -p FILE`,
//! drives it through the CLI like an agent would, and checks:
//! - a CLI edit is applied by the GUI and shows in its window in < 100 ms;
//! - after `seek`, the viewer screenshot shows exactly the frame `vidcut frame`
//!   renders headless;
//! - live control keeps working while the window is minimized.
//!
//! A failed run keeps its temp dir (it says which): the screenshots, each
//! player's mpv log (`VIDCUT_MPV_LOG_DIR`) and the GUI's stderr with
//! `VIDCUT_DEBUG_PLAYER` traces.
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
    /// Runs a CLI command against this GUI; returns the JSON response and
    /// how long the whole command took.
    fn cli(&self, args: &[&str]) -> (Value, Duration) {
        let started = Instant::now();
        let out = Command::new(BIN)
            .args(["--json", "--instance", &self.child.id().to_string(), "-p"])
            .arg(&self.project)
            .args(args)
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .output()
            .unwrap();
        let took = started.elapsed();
        let v: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
        assert!(out.status.success(), "{args:?}: {v}");
        (v, took)
    }

    fn barcode(&self, png: &Path) -> Option<u32> {
        let mut r = vidcut_av::decode::VideoReader::open(png).unwrap();
        vidcut_av::decode::read_barcode(&r.next_frame().unwrap().unwrap()).unwrap()
    }

    /// Seeks the timeline, screenshots the viewer, and compares with a
    /// headless `vidcut frame`.
    fn check_frame(&self, frame: i64) {
        let at = format!("{frame}f");
        self.cli(&["seek", &at, "--viewer", "timeline"]);
        let shot = self.dir.path().join(format!("shot-{frame}.png"));
        self.cli(&["screenshot", "-o", shot.to_str().unwrap()]);
        let reference = self.dir.path().join(format!("frame-{frame}.png"));
        let out = Command::new(BIN)
            .args(["--headless", "-p"])
            .arg(&self.project)
            .args(["frame", "--at", &at, "-o", reference.to_str().unwrap()])
            .env("XDG_STATE_HOME", self.dir.path().join("state"))
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            self.barcode(&shot),
            self.barcode(&reference),
            "timeline frame {frame}: viewer vs vidcut frame"
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
                "kept {}: screenshots, mpv logs, the GUI's stderr",
                self.dir.path().display()
            );
        }
    }
}

#[test]
fn gui_acceptance() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        eprintln!("skipping: no display");
        return;
    }
    if let Err(e) = vidcut_mpv::available() {
        eprintln!("skipping: {e}");
        return;
    }
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
    let out = Command::new(BIN).arg("new").arg(&project).output().unwrap();
    assert!(out.status.success());
    // A phone-style clip, stored sideways with a rotation: the preview
    // turns it upright on the GPU, like export. (mpv's software renderer,
    // which tests/live.rs uses, aborts on rotated video.)
    let rotated = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 60)
            .rotation(90)
            .no_audio(),
    )
    .unwrap();
    headless(&["media", "add", media.to_str().unwrap()]);
    headless(&["media", "add", rotated.to_str().unwrap()]);
    headless(&["edit", "append", "m1", "--range", "0f..90f"]);
    headless(&["edit", "append", "m1", "--range", "200f..260f"]);
    headless(&["edit", "append", "m2", "--range", "10f..30f"]);

    // Evidence for a failed run, kept in `dir`.
    let stderr = std::fs::File::create(dir.join("stderr.log")).unwrap();
    let child = Command::new(BIN)
        .arg("-p")
        .arg(&project)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("VIDCUT_MPV_LOG_DIR", &dir)
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
    // mpv opens its logs when the players are made, before the session starts.
    for player in ["source", "timeline"] {
        let log = dir.join(format!("{pid}-{player}.log"));
        assert!(log.exists(), "no mpv log {}", log.display());
    }
    // A fixed window size keeps screenshots comparable.
    gui.cli(&["window", "--size", "1280x800"]);
    std::thread::sleep(Duration::from_millis(500));

    // 1. Seek + screenshot shows exactly what `vidcut frame` renders.
    // Frames 150..170 are the rotated clip.
    for f in [0, 1, 45, 89, 90, 91, 149, 150, 155, 169] {
        gui.check_frame(f);
    }

    // 2. A CLI edit is applied by the GUI (the command returns after the GUI
    // saved it) and the window shows it in under 100 ms.
    let before = dir.join("before.png");
    gui.cli(&["screenshot", "--window", "-o", before.to_str().unwrap()]);
    let (r, took) = gui.cli(&["edit", "split", "--at", "45f"]);
    assert!(r["via"].as_str().unwrap().starts_with("gui:"), "{r}");
    let after = dir.join("after.png");
    let (_, shot_took) = gui.cli(&["screenshot", "--window", "-o", after.to_str().unwrap()]);
    eprintln!("edit round trip {took:?}; window screenshot {shot_took:?}");
    assert!(took < Duration::from_millis(100), "edit took {took:?}");
    assert!(
        gui.window_diff(&before, &after) > 0,
        "the window doesn't show the split"
    );
    let (tl, _) = gui.cli(&["timeline"]);
    let clips = tl["result"]["tracks"][0]["items"].as_array().unwrap().len();
    assert_eq!(clips, 4, "{tl}");

    // 3. Minimized, the GUI still answers, seeks and renders frame-exactly.
    gui.cli(&["window", "--minimize"]);
    std::thread::sleep(Duration::from_millis(500));
    gui.check_frame(100);
    let (r, took) = gui.cli(&["edit", "split", "--at", "120f"]);
    assert!(
        r["changes"]["created"]
            .as_array()
            .is_some_and(|c| c.len() == 1)
    );
    eprintln!("edit while minimized {took:?}");
    gui.check_frame(121);

    // Quit cleanly: the instance unregisters.
    gui.cli(&["quit"]);
    let started = Instant::now();
    while vidcut::ipc::registry::list().iter().any(|i| i.pid == pid) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "GUI did not quit"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
