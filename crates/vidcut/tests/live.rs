//! The live session without a window: both players render through mpv's
//! software renderer on a harness thread (standing in for the GUI), and
//! requests go through `Handle::call` exactly as IPC clients' do. Checks that
//! what the viewer shows after seeks and edits is frame-exact, that edits from
//! other processes appear, and the clipboard and selection behavior.
//!
//! Needs libmpv (skips without it), no display.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Image, Origin, PlayerId, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};
use vidcut_mpv::{Mpv, SwRenderer, ffi::MPV_RENDER_UPDATE_FRAME};

const W: usize = 320;
const H: usize = 180;

#[derive(Default)]
struct Port {
    captures: Mutex<Vec<(PlayerId, Capture)>>,
}

impl live::Renderer for Port {
    fn capture_viewer(&self, player: PlayerId, done: Capture) {
        self.captures.lock().unwrap().push((player, done));
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

struct Harness {
    handle: live::Handle,
    session: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    render: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn start(project: &Path) -> Option<Self> {
        if let Err(e) = vidcut_mpv::available() {
            eprintln!("skipping: {e}");
            return None;
        }
        let mut opts = live::mpv_options(false);
        opts.push(("hwdec".into(), "no".into()));
        let opts: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let source = Mpv::new(&opts).unwrap();
        let timeline = Mpv::new(&opts).unwrap();
        let port = Arc::new(Port::default());
        let handle_cell: Arc<OnceLock<live::Handle>> = Arc::new(OnceLock::new());
        let stop = Arc::new(AtomicBool::new(false));
        // The "render thread": renders both players into buffers whenever
        // mpv has a frame, reports renders, and answers captures.
        let render = {
            let (port, cell, stop) = (port.clone(), handle_cell.clone(), stop.clone());
            let players = [source.clone(), timeline.clone()];
            std::thread::spawn(move || {
                let mut renderers: Vec<SwRenderer> = players
                    .iter()
                    .map(|m| SwRenderer::new(m).unwrap())
                    .collect();
                let mut bufs = vec![vec![0u8; W * H * 4]; 2];
                while !stop.load(Ordering::Relaxed) {
                    for (i, r) in renderers.iter_mut().enumerate() {
                        if r.update() & MPV_RENDER_UPDATE_FRAME != 0 {
                            let new = r.next_frame_is_new();
                            r.render_rgb0(W, H, W * 4, &mut bufs[i]).unwrap();
                            if let Some(h) = cell.get() {
                                h.rendered(PlayerId::ALL[i], new);
                            }
                        }
                    }
                    let captures: Vec<_> = port.captures.lock().unwrap().drain(..).collect();
                    for (id, done) in captures {
                        // RGB0 → opaque RGBA.
                        let mut rgba = bufs[id.index()].clone();
                        rgba.chunks_exact_mut(4).for_each(|px| px[3] = 255);
                        done(Ok(Image {
                            width: W as u32,
                            height: H as u32,
                            rgba,
                        }));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        let (handle, session) = live::start(live::Config {
            project: Some(project.to_path_buf()),
            create: false,
            players: Some((source, timeline)),
            preview_error: None,
            renderer: port,
            notify: Arc::new(|| {}),
            socket: None,
        });
        let _ = handle_cell.set(handle.clone());
        Some(Self {
            handle,
            session: Some(session),
            stop,
            render: Some(render),
        })
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
    fn wait_for(&self, what: &str, check: impl Fn(&live::Snapshot) -> bool) -> Duration {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if check(&self.handle.snapshot()) {
                return started.elapsed();
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
fn live_session() {
    let dir = std::env::temp_dir().join(format!("vidcut-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe { std::env::set_var("XDG_STATE_HOME", dir.join("state")) };
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
        auto_format: false,
        ..Settings::default()
    };
    Store::create(&path, &Project::new(settings)).unwrap();
    let Some(h) = Harness::start(&path) else {
        return;
    };
    let t0 = Instant::now();
    let step = |name: &str| eprintln!("{:>8.1} ms  {name}", t0.elapsed().as_secs_f64() * 1e3);

    // Build a timeline live: c1 = m1 30..90, c2 = m2 0..30, c3 = m1 200..260.
    h.ok("media.add", json!({ "paths": [a, b] }));
    h.ok("edit.append", json!({ "media": "m1", "range": "30f..90f" }));
    h.ok("edit.append", json!({ "media": "m2", "range": "0f..30f" }));
    let r = h.ok(
        "edit.append",
        json!({ "media": "m1", "range": "200f..260f" }),
    );
    assert!(r.via.starts_with("gui:"), "{}", r.via);
    // Decision D15: the playhead moved past the appended clip.
    assert_eq!(
        h.ok("player.state", json!({})).result["timeline"]["position"],
        150
    );

    step("Seeks are frame-exact at every bounda");
    // Seeks are frame-exact at every boundary, and agree with frame.render.
    let expected = |t: i64| -> u32 {
        match t {
            0..60 => (30 + t) as u32,
            60..90 => (t - 60) as u32,
            _ => (200 + t - 90) as u32,
        }
    };
    for t in [0, 1, 59, 60, 61, 89, 90, 91, 149] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(h.shown(&dir), Some(expected(t)), "timeline frame {t}");
    }
    let out = dir.join("f.png");
    h.ok("frame.render", json!({ "at": "playhead", "out": out }));
    let mut r = vidcut_av::decode::VideoReader::open(&out).unwrap();
    let f = r.next_frame().unwrap().unwrap();
    assert_eq!(
        vidcut_av::decode::read_barcode(&f).unwrap(),
        Some(expected(149))
    );
    // Adjacent segments of one file: c3's last frame before a pasted m1 piece.
    let r = h.ok(
        "edit.append",
        json!({ "media": "m1", "range": "100f..130f" }),
    );
    let c4 = &r.changes.created[0];
    for t in [148, 149, 150, 151] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        let want = if t < 150 {
            expected(t)
        } else {
            (100 + t - 150) as u32
        };
        assert_eq!(h.shown(&dir), Some(want), "same-file boundary, frame {t}");
    }
    h.ok("edit.delete", json!({ "clips": c4 }));
    h.ok("player.seek", json!({ "at": 149 }));
    // Relative steps and seeks.
    h.ok("player.step", json!({ "frames": -2 }));
    assert_eq!(h.shown(&dir), Some(expected(147)));
    h.ok("player.seek", json!({ "at": "-1s" }));
    assert_eq!(h.shown(&dir), Some(expected(117)));

    step("An edit reloads the preview");
    // An edit reloads the preview: split c1 and delete its first half.
    h.ok("edit.split", json!({ "at": 20 }));
    h.ok("edit.delete", json!({ "clips": "c1" }));
    h.ok("player.seek", json!({ "at": 0 }));
    assert_eq!(
        h.shown(&dir),
        Some(50),
        "the timeline now starts at m1 frame 50"
    );

    step("Source Clip: source times");
    // Source Clip: source times, marks drive edits without a range.
    h.ok("viewer.load", json!({ "media": "m1", "at": "100f" }));
    assert_eq!(h.shown(&dir), Some(100));
    h.ok(
        "media.marks.set",
        json!({ "media": "m1", "in": "playhead", "out": "+10f" }),
    );
    let m1 = h.ok("media.get", json!({ "media": "m1" }));
    assert_eq!(m1.result["marks"], json!({ "in": 100, "out": 110 }));
    let state = h.ok("player.state", json!({}));
    assert_eq!(state.result["viewer"], "source_clip");
    assert_eq!(state.result["source"]["source_frame"], 100);

    step("Source Tape: m1");
    // Source Tape: m1 (300 frames) then m2; the clip under the playhead is
    // selected in the pool.
    h.ok("player.seek", json!({ "at": 310, "viewer": "source_tape" }));
    assert_eq!(h.shown(&dir), Some(10), "tape frame 310 = m2 frame 10");
    let sel = h.ok("selection.get", json!({}));
    assert_eq!(sel.result["media"], json!(["m2"]));
    // Q-style switching keeps each player where it was.
    h.ok("viewer.set", json!({ "mode": "timeline" }));
    h.ok("viewer.set", json!({ "mode": "source_tape" }));
    assert_eq!(h.shown(&dir), Some(10));

    step("Clipboard: copy two clips");
    // Clipboard: copy two clips, paste them at the end of the timeline.
    h.ok("viewer.set", json!({ "mode": "timeline" }));
    let tl = h.ok("timeline.get", json!({}));
    let ids: Vec<String> = tl.result["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "clip")
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    let end = h.ok("project.summary", json!({})).result["duration"]
        .as_i64()
        .unwrap();
    h.ok(
        "edit.copy",
        json!({ "clips": [ids[0].clone(), ids[1].clone()] }),
    );
    h.ok("player.seek", json!({ "at": "end" }));
    let pasted = h.ok("edit.paste", json!({}));
    assert_eq!(pasted.changes.created.len(), 2, "{pasted:?}");
    let after = h.ok("project.summary", json!({})).result["duration"]
        .as_i64()
        .unwrap();
    assert_eq!(after, end + 40 + 30);
    h.ok("player.seek", json!({ "at": end }));
    assert_eq!(h.shown(&dir), Some(50), "the pasted copy of the first clip");

    step("Selection takes clip refs.");
    // Selection takes clip refs.
    let sel = h.ok("selection.set", json!({ "clips": "V1@0f" }));
    assert_eq!(sel.result["clips"], json!([ids[0]]));

    step("A headless edit by another process");
    // A headless edit by another process (bypassing the GUI) shows up.
    let rev = h.ok("project.summary", json!({})).rev;
    let store = Store::open(&path).unwrap();
    store
        .transact(|ed| {
            ed.apply(
                Envelope::from_json(json!({"method": "marks.set", "params": {"in": 5, "out": 15}}))
                    .unwrap(),
                &vidcut::env::AvEnv,
                None,
            )
        })
        .unwrap();
    let took = h.wait_for("the external edit", |s| {
        s.project()
            .is_some_and(|p| p.rev > rev && p.marks.mark_in == Some(5))
    });
    eprintln!("external edit visible after {took:?}");
    assert!(took < Duration::from_millis(500), "{took:?}");
    // ...and undoes like any other step, from the GUI's side.
    let undone = h.ok("history.undo", json!({}));
    assert_eq!(undone.result["undone"], json!(["marks.set"]));

    step("Errors are reported, not hangs.");
    // Errors are reported, not hangs.
    let e = h.call("viewer.load", json!({ "media": "m9" })).unwrap_err();
    assert_eq!(e.code, vidcut_core::ErrorCode::NotFound);
    let e = h
        .call("edit.paste", json!({ "clips": [], "at": 0 }))
        .unwrap_err();
    assert_eq!(e.code, vidcut_core::ErrorCode::Usage);
    let log = h.ok("gui.log", json!({ "limit": 3 }));
    assert_eq!(log.result.as_array().unwrap().len(), 3);

    // A clip from a file's very first frame: its EDL start is half a frame
    // before the file's first timestamp.
    let fresh = dir.join("first.vidcut");
    h.ok("project.open", json!({ "path": fresh, "create": true }));
    let big = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        640,
        360,
        (30, 1),
        300,
    ))
    .unwrap();
    h.ok("media.add", json!({ "paths": [big] }));
    h.ok("edit.append", json!({ "media": "m1", "range": "0f..90f" }));
    h.ok(
        "edit.append",
        json!({ "media": "m1", "range": "200f..260f" }),
    );
    for (t, want) in [
        (0, 0),
        (1, 1),
        (87, 87),
        (88, 88),
        (89, 89),
        (90, 200),
        (91, 201),
    ] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(
            h.shown(&dir),
            Some(want),
            "first-frame clip, timeline frame {t}"
        );
    }

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
