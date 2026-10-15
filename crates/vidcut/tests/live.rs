//! The live session without a window. The viewer's player is the engine. A
//! harness thread stands in for the GUI's renderer and turns its pictures
//! into RGBA. Its sound goes to a stand-in device. Requests go through
//! `Handle::call` as IPC clients' do.
//! Checks that viewer seeks are frame-exact at every cut, that an edit by
//! another process shows up and undoes, and that proxies show the same
//! frames as the originals.
//!
//! No display, GPU or audio device.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Image, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{PacedOutput, Picture, Player};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

const W: usize = 320;
const H: usize = 180;
const RATE: u32 = 48_000;

/// How long after its buffer is due the stand-in device's sound is heard.
const LATENCY: Duration = Duration::from_millis(30);

type Engine = Arc<Player>;

#[derive(Default)]
struct Port {
    engine: Mutex<Option<Engine>>,
    presents: Mutex<Vec<Presented>>,
    captures: Mutex<Vec<Capture>>,
}

impl live::Renderer for Port {
    fn attach(&self, engine: Option<Engine>) {
        *self.engine.lock().unwrap() = engine;
    }
    fn present(&self, done: Presented) {
        self.presents.lock().unwrap().push(done);
    }
    fn capture_viewer(&self, done: Capture) {
        self.captures.lock().unwrap().push(done);
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

/// What the viewer shows: the picture, as drawn.
#[derive(Default)]
struct Screen {
    picture: Option<Arc<Picture>>,
    image: Option<Image>,
    /// The frame the latest present showed exactly.
    exact: Option<i64>,
}

fn black() -> Image {
    Image {
        width: W as u32,
        height: H as u32,
        rgba: [0, 0, 0, 255].repeat(W * H),
    }
}

struct Harness {
    handle: live::Handle,
    session: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    render: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn start(project: &Path) -> Self {
        let port = Arc::new(Port::default());
        let stop = Arc::new(AtomicBool::new(false));
        // The "render thread": presents the player every 2 ms, as a display
        // would, draws what changed, and answers presents and captures.
        let render = {
            let (port, stop) = (port.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut s = Screen::default();
                while !stop.load(Ordering::Relaxed) {
                    let engine = port.engine.lock().unwrap().clone();
                    let presents: Vec<_> = port.presents.lock().unwrap().drain(..).collect();
                    let now = Instant::now();
                    let shown = engine.as_ref().and_then(|e| e.present(now));
                    let picture = shown.as_ref().map(|p| p.picture.clone());
                    let same = match (&s.picture, &picture) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        (None, None) => s.image.is_some(),
                        _ => false,
                    };
                    if !same {
                        s.image = Some(match &picture {
                            Some(p) => p.to_rgba(W as u32, H as u32).unwrap(),
                            None => black(),
                        });
                        s.picture = picture;
                    }
                    s.exact = shown.filter(|p| p.exact).map(|p| p.frame);
                    for done in presents {
                        done(s.exact);
                    }
                    let captures: Vec<_> = port.captures.lock().unwrap().drain(..).collect();
                    for done in captures {
                        done(Ok(s.image.clone().unwrap_or_else(black)));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        let audio = Arc::new(|| {
            Box::new(PacedOutput::new(Duration::from_millis(10), LATENCY))
                as Box<dyn vidcut_av::player::AudioOutput>
        });
        let (handle, session) = live::start(live::Config {
            project: Some(project.to_path_buf()),
            create: false,
            player: Some(live::PlayerConfig {
                audio,
                budget: vidcut_av::player::DEFAULT_BUDGET,
                gpu_pictures: Vec::new(),
            }),
            preview_error: None,
            renderer: port.clone(),
            notify: Arc::new(|| {}),
            socket: None,
        });
        Self {
            handle,
            session: Some(session),
            stop,
            render: Some(render),
        }
    }

    fn call(&self, method: &str, params: Value) -> vidcut_core::Result<Response> {
        let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
        self.handle.call(e, Origin::Ipc, None, &mut |_| true)
    }

    fn ok(&self, method: &str, params: Value) -> Response {
        self.call(method, params.clone())
            .unwrap_or_else(|e| panic!("{method} {params}: {e}"))
    }

    /// The barcode the viewer shows (via a screenshot PNG).
    fn shown(&self, dir: &Path) -> Option<u32> {
        let out = dir.join("shot.png");
        self.ok("gui.screenshot", json!({ "out": out }));
        let mut r = vidcut_av::decode::VideoReader::open(&out).unwrap();
        let f = r.next_frame().unwrap().unwrap();
        vidcut_av::decode::read_barcode(&f).unwrap()
    }

    /// Waits until `check` holds for the latest snapshot.
    fn wait_for(&self, what: &str, check: impl Fn(&live::Snapshot) -> bool) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if check(&self.handle.snapshot()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for {what}");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(t) = self.session.take() {
            let _ = t.join();
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.render.take() {
            let _ = t.join();
        }
    }
}

fn fixture_file(frames: u32) -> PathBuf {
    fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        W as u32,
        H as u32,
        (30, 1),
        frames,
    ))
    .unwrap()
}

#[test]
fn exact_seeks_external_edit_and_proxies() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-live-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
    }
    // Decoding on the CPU: no GPU needed.
    std::fs::create_dir_all(dir.join("config/vidcut")).unwrap();
    std::fs::write(
        dir.join("config/vidcut/preferences.toml"),
        "[gpu]\nacceleration = false\n",
    )
    .unwrap();
    let a = fixture_file(300);
    let b = fixture::ensure(
        &FixtureSpec::new(
            VideoCodec::Hevc10,
            Container::Mkv,
            W as u32,
            H as u32,
            (30, 1),
            120,
        )
        .no_audio(),
    )
    .unwrap();
    let path = dir.join("p.vidcut");
    let settings = Settings {
        fps: Rational::new(30, 1).unwrap(),
        width: W as u32,
        height: H as u32,
        sample_rate: RATE,
        auto_format: false,
        ..Settings::default()
    };
    Store::create(&path, &Project::new(settings)).unwrap();
    let h = Harness::start(&path);

    // Build a timeline live: c1 = m1 30..90, c2 = m2 0..30, c3 = m1 200..260.
    h.ok("media.add", json!({ "paths": [a, b] }));
    h.ok("edit.append", json!({ "media": "m1", "range": "30f..90f" }));
    h.ok("edit.append", json!({ "media": "m2", "range": "0f..30f" }));
    h.ok(
        "edit.append",
        json!({ "media": "m1", "range": "200f..260f" }),
    );

    // Seeks are frame-exact at every boundary.
    let expected = |t: i64| -> u32 {
        match t {
            0..60 => (30 + t) as u32,
            60..90 => (t - 60) as u32,
            _ => (200 + t - 90) as u32,
        }
    };
    for t in [0, 1, 59, 60, 61, 89, 90, 91, 149] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(h.shown(dir), Some(expected(t)), "timeline frame {t}");
    }

    // Stats for nerds tell of the picture shown, and the panel toggles.
    let stats = h.ok("player.state", json!({})).result["stats"].clone();
    assert_eq!(stats["picture"]["frame"], json!(149), "{stats}");
    assert_eq!(stats["picture"]["size"], json!([W, H]), "{stats}");
    assert!(stats["kept"]["pictures"].as_u64() > Some(0), "{stats}");
    let toggled = h.ok("player.stats", json!({})).result;
    assert_eq!(toggled["stats_shown"], json!(true));

    // A headless edit by another process (bypassing the GUI) shows up.
    let rev = h.ok("project.summary", json!({})).rev;
    let store = Store::open(&path).unwrap();
    store
        .transact(|ed| {
            ed.apply(
                Envelope::from_json(json!({"method": "marks.set", "params": {"in": 5, "out": 15}}))
                    .unwrap(),
                &vidcut::env::AvEnv::new(),
                None,
            )
        })
        .unwrap();
    h.wait_for("the external edit", |s| {
        s.project()
            .is_some_and(|p| p.rev > rev && p.marks.mark_in == Some(5))
    });
    // ...and undoes like any other step, from the GUI's side.
    let undone = h.ok("history.undo", json!({}));
    assert_eq!(
        undone.result["undone"],
        json!(["Timeline marks: In 00:00:00:05, Out 00:00:00:15"])
    );

    // A proxy of m1 (decision D41) shows the frames `frame render` shows
    // from the original once the project plays proxies.
    let rendered = |t: i64| {
        let out = dir.join("render.png");
        h.ok("frame.render", json!({ "at": t, "out": out }));
        let mut r = vidcut_av::decode::VideoReader::open(&out).unwrap();
        vidcut_av::decode::read_barcode(&r.next_frame().unwrap().unwrap()).unwrap()
    };
    h.ok("proxy.generate", json!({ "media": "m1" }));
    let m1 = h.ok("media.get", json!({ "media": "m1" })).result;
    assert!(m1["proxy"].is_object(), "{m1}");
    h.ok("project.set", json!({ "use_proxies": true }));
    for t in [0, 9, 10, 25, 26, 30] {
        let want = rendered(t);
        assert!(want.is_some(), "frame {t} shows m1");
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        let state = h.ok("player.state", json!({})).result;
        assert_eq!(state["timeline"]["error"], Value::Null, "{state}");
        assert_eq!(h.shown(dir), want, "proxy, timeline frame {t}");
    }

    // An OTIO file opens as a project file made next to it (deviation X9).
    let cut = dir.join("cut.otio");
    h.ok("export.start", json!({ "output": cut }));
    let tracks = h.ok("timeline.get", json!({})).result["tracks"].clone();
    let opened = h.ok("project.open", json!({ "path": cut })).result;
    assert_eq!(opened["warnings"], json!([]));
    let project = PathBuf::from(opened["project"].as_str().unwrap());
    assert_eq!(project.file_name().unwrap(), "cut.vidcut");
    assert_eq!(h.ok("timeline.get", json!({})).result["tracks"], tracks);
    // The name is taken now.
    assert!(h.call("project.open", json!({ "path": cut })).is_err());

    // A new project over a file already there: without force the file
    // opens, and this one isn't a project; with force it is replaced.
    let taken = dir.join("taken.vidcut");
    std::fs::write(&taken, "not a project").unwrap();
    let create = json!({ "path": taken, "create": true, "fps": "25", "width": 640, "height": 360 });
    assert!(h.call("project.open", create.clone()).is_err());
    let mut force = create;
    force["force"] = json!(true);
    h.ok("project.open", force);
    let made = Store::open(&taken).unwrap().load().unwrap();
    let p = made.project();
    assert_eq!((p.settings.width, p.media.len()), (640, 0));

    drop(h);
}
