//! The live session without a window. Playback is the session's own player
//! (FFmpeg, in process). A harness thread stands in for the GUI: it acks
//! presents, and can hold one back so a later seek overtakes it. Requests go
//! through `Handle::call` exactly as IPC clients' do. Checks that what the
//! viewer shows after seeks and edits is frame-exact, that edits from other
//! processes appear, and the clipboard and selection behavior.
//!
//! No display, and libmpv is not loaded.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, Playback, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, ImageSpec, Sound, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, ErrorCode, Project, Response, Store};

const W: usize = 320;
const H: usize = 180;

#[derive(Default)]
struct Port {
    presents: Mutex<Vec<(PlayerId, Presented)>>,
    captures: Mutex<Vec<(PlayerId, Capture)>>,
    /// Milliseconds the render thread looks away once it next finds a new
    /// picture waiting, like a UI thread busy elsewhere.
    stall: AtomicU64,
}

impl live::Renderer for Port {
    fn present(&self, player: PlayerId, done: Presented) {
        self.presents.lock().unwrap().push((player, done));
    }
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
    port: Arc<Port>,
    session: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    render: Option<std::thread::JoinHandle<()>>,
    /// Samples the timeline player wrote to its sink, in order.
    heard: Arc<Mutex<Vec<f32>>>,
}

impl Harness {
    fn start(project: &Path) -> Self {
        let port = Arc::new(Port::default());
        let stop = Arc::new(AtomicBool::new(false));
        // The "render thread" only acks presents. A stall holds the ack
        // (cleared as the wait starts, so a second seek lands first).
        let render = {
            let (port, stop) = (port.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let stalled = {
                        let pending = port.presents.lock().unwrap();
                        if port.stall.load(Ordering::Relaxed) > 0 && !pending.is_empty() {
                            Some(port.stall.swap(0, Ordering::Relaxed))
                        } else {
                            None
                        }
                    };
                    if let Some(ms) = stalled {
                        std::thread::sleep(Duration::from_millis(ms));
                    }
                    let presents: Vec<_> = port.presents.lock().unwrap().drain(..).collect();
                    for (_, done) in presents {
                        done();
                    }
                    // Captures are answered by the session. Drop any the
                    // renderer was asked for so a waiter cannot hang.
                    let captures: Vec<_> = port.captures.lock().unwrap().drain(..).collect();
                    for (_, done) in captures {
                        done(Err(vidcut_core::Error::usage("no render context")));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        let heard = Arc::new(Mutex::new(Vec::new()));
        let record = heard.clone();
        let (handle, session) = live::start(live::Config {
            project: Some(project.to_path_buf()),
            create: false,
            preview_error: None,
            renderer: port.clone(),
            notify: Arc::new(|| {}),
            socket: None,
            // The device boundary: the session writes the mix here instead of
            // opening PulseAudio. Source playback is not what these tests hear.
            playback: Arc::new(move |id| {
                if id == PlayerId::Timeline {
                    Box::new(live::Recorder {
                        samples: record.clone(),
                    })
                } else {
                    Box::new(live::Discard)
                }
            }),
        });
        Self {
            handle,
            port,
            session: Some(session),
            stop,
            render: Some(render),
            heard,
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

    /// Makes the render thread look away for `d` once it next finds a new
    /// picture waiting.
    fn stall_next(&self, d: Duration) {
        self.port
            .stall
            .store(d.as_millis() as u64, Ordering::Relaxed);
    }

    /// Waits until the render thread has begun the stall [`stall_next`]
    /// asked for: the picture it found is held back from now on, so a
    /// request sent now lands while it is.
    ///
    /// [`stall_next`]: Self::stall_next
    fn wait_stalled(&self) {
        let started = Instant::now();
        while self.port.stall.load(Ordering::Relaxed) != 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the render thread never stalled"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The barcode the viewer shows (via a screenshot PNG).
    fn shown(&self, dir: &Path) -> Option<u32> {
        let out = dir.join("shot.png");
        self.ok("gui.screenshot", json!({ "out": out }));
        let mut r = vidcut_av::decode::VideoReader::open(&out).unwrap();
        let f = r.next_frame().unwrap().unwrap();
        vidcut_av::decode::read_barcode(&f).unwrap()
    }

    fn timeline(&self) -> Value {
        self.ok("player.state", json!({})).result["timeline"].clone()
    }

    fn position(&self) -> i64 {
        self.timeline()["position"].as_i64().unwrap()
    }

    fn monitor(&self) -> Value {
        self.timeline()["monitor"].clone()
    }

    fn playing(&self) -> bool {
        self.timeline()["playing"].as_bool().unwrap()
    }

    fn frames(&self) -> i64 {
        self.timeline()["frames"].as_i64().unwrap()
    }

    /// Samples written since the last call.
    fn take_heard(&self) -> Vec<f32> {
        std::mem::take(&mut self.heard.lock().unwrap())
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

/// The luma (green) of a PNG screenshot.
fn image(png: &Path) -> (usize, usize, Vec<u8>) {
    let mut r = vidcut_av::decode::VideoReader::open(png).unwrap();
    let f = r.next_frame().unwrap().unwrap();
    let rgb = f.data(0);
    let (w, h, stride) = (f.width() as usize, f.height() as usize, f.stride(0));
    let bpp = rgb.len() / (stride * h).max(1) * stride / w.max(1);
    let bpp = bpp.clamp(1, 4);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(rgb[y * stride + x * bpp + bpp.min(2) - 1]);
        }
    }
    (w, h, out)
}

/// Mean difference between each row of the top eighth and the average of
/// its neighbors: large where woven fields differ.
fn combing((w, h, y): &(usize, usize, Vec<u8>)) -> f64 {
    let rows = h / 8;
    let mut sum = 0.0;
    for r in 1..rows - 1 {
        for x in 0..*w {
            let (a, b, c) = (
                f64::from(y[(r - 1) * w + x]),
                f64::from(y[r * w + x]),
                f64::from(y[(r + 1) * w + x]),
            );
            sum += (b - (a + c) / 2.0).abs();
        }
    }
    sum / ((rows - 2) * w) as f64
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
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-live-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
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
    let h = Harness::start(&path);
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
        assert_eq!(h.shown(dir), Some(expected(t)), "timeline frame {t}");
    }
    step("A late renderer");
    // A renderer that gets to a seek's frame only after libmpv stopped
    // waiting for it (200 ms) still has that frame on screen when the seek
    // returns (docs/decisions/gui.md §4).
    for t in [61, 149] {
        h.stall_next(Duration::from_millis(500));
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(h.shown(dir), Some(expected(t)), "late renderer, frame {t}");
    }
    step("An overtaken seek fails");
    // Seek A's frame is late (the renderer looks away), and seek B moves the
    // viewer elsewhere first: A's frame never shows, so A says so instead of
    // reporting B's frame as its own.
    h.stall_next(Duration::from_millis(400));
    let handle = h.handle.clone();
    let a = std::thread::spawn(move || {
        let e = Envelope::from_json(json!({ "method": "player.seek", "params": { "at": 30 } }))
            .unwrap();
        handle.call(e, Origin::Ipc, None, &mut |_| true)
    });
    h.wait_stalled();
    h.ok("player.seek", json!({ "at": 120 }));
    let e = a.join().unwrap().unwrap_err();
    assert_eq!(e.code, vidcut_core::ErrorCode::Conflict, "{e}");
    assert!(
        e.message.contains("frame 120"),
        "names the frame it moved to: {e}"
    );
    assert_eq!(h.shown(dir), Some(expected(120)));
    // A fast seek to the same frame doesn't stand in for an exact one: it
    // may show a keyframe instead.
    h.stall_next(Duration::from_millis(400));
    let handle = h.handle.clone();
    let a = std::thread::spawn(move || {
        let e = Envelope::from_json(json!({ "method": "player.seek", "params": { "at": 100 } }))
            .unwrap();
        handle.call(e, Origin::Ipc, None, &mut |_| true)
    });
    h.wait_stalled();
    h.ok("player.seek", json!({ "at": 100, "fast": true }));
    let e = a.join().unwrap().unwrap_err();
    assert_eq!(e.code, vidcut_core::ErrorCode::Conflict, "{e}");
    assert!(e.message.contains("keyframe near frame 100"), "{e}");

    // `if_rev` holds for live requests too.
    let rev = h.ok("project.summary", json!({})).rev;
    let e = h
        .call("player.seek", json!({ "at": 10, "if_rev": rev - 1 }))
        .unwrap_err();
    assert_eq!(e.code, vidcut_core::ErrorCode::Conflict, "{e}");

    step("A seek while playing returns");
    // Playback moves the playhead on from the sought frame; that isn't
    // another seek. The renderer looking away lets playback move on before
    // the frame is counted as shown.
    h.ok("player.seek", json!({ "at": 0 }));
    h.ok("player.play", json!({}));
    std::thread::sleep(Duration::from_millis(150));
    for at in [40, 90] {
        h.stall_next(Duration::from_millis(400));
        h.ok("player.seek", json!({ "at": at }));
    }
    // A preview reload while that seek is in flight samples the playhead.
    // Playback has moved it off the asked frame; adopting the sample would
    // fail the seek and name the playback frame as another seek.
    h.stall_next(Duration::from_millis(800));
    let handle = h.handle.clone();
    let a = std::thread::spawn(move || {
        let e = Envelope::from_json(json!({
            "method": "player.seek",
            "params": { "at": 20 }
        }))
        .unwrap();
        handle.call(e, Origin::Ipc, None, &mut |_| true)
    });
    h.wait_stalled();
    h.ok("track.set", json!({ "track": "V1", "volume_db": -6.0 }));
    a.join()
        .unwrap()
        .unwrap_or_else(|e| panic!("reload while playing must not retarget the seek: {e}"));
    h.ok("player.pause", json!({}));
    // Back to 149 for what follows.
    h.ok("player.seek", json!({ "at": 149 }));
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
        assert_eq!(h.shown(dir), Some(want), "same-file boundary, frame {t}");
    }
    h.ok("edit.delete", json!({ "clips": c4 }));
    h.ok("player.seek", json!({ "at": 149 }));
    // Relative steps and seeks.
    h.ok("player.step", json!({ "frames": -2 }));
    assert_eq!(h.shown(dir), Some(expected(147)));
    h.ok("player.seek", json!({ "at": "-1s" }));
    assert_eq!(h.shown(dir), Some(expected(117)));
    step("Quick steps");
    // Steps quicker than frames decode, as a held arrow key makes them: a
    // seek waits for the one still decoding instead of cancelling it, and
    // the last lands exactly. The GUI's overtaken steps aren't errors.
    let steps: Vec<_> = (0..8)
        .map(|_| {
            let handle = h.handle.clone();
            std::thread::spawn(move || {
                let e = Envelope::from_json(json!({
                    "method": "player.step",
                    "params": { "frames": 1 }
                }))
                .unwrap();
                handle.call(e, Origin::Gui { drag: false }, None, &mut |_| true)
            })
        })
        .collect();
    for s in steps {
        s.join()
            .unwrap()
            .expect("an overtaken GUI step is no error");
    }
    assert_eq!(
        h.ok("player.state", json!({})).result["timeline"]["position"],
        125
    );
    assert_eq!(h.shown(dir), Some(expected(125)));

    step("An edit reloads the preview");
    // An edit reloads the preview: split c1 and delete its first half.
    h.ok("edit.split", json!({ "at": 20 }));
    h.ok("edit.delete", json!({ "clips": "c1" }));
    h.ok("player.seek", json!({ "at": 0 }));
    assert_eq!(
        h.shown(dir),
        Some(50),
        "the timeline now starts at m1 frame 50"
    );

    step("Source Clip: source times");
    // Source Clip: source times, marks drive edits without a range.
    h.ok("viewer.load", json!({ "media": "m1", "at": "100f" }));
    assert_eq!(h.shown(dir), Some(100));
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
    assert_eq!(h.shown(dir), Some(10), "tape frame 310 = m2 frame 10");
    let sel = h.ok("selection.get", json!({}));
    assert_eq!(sel.result["media"], json!(["m2"]));
    // Q-style switching keeps each player where it was.
    h.ok("viewer.set", json!({ "mode": "timeline" }));
    h.ok("viewer.set", json!({ "mode": "source_tape" }));
    assert_eq!(h.shown(dir), Some(10));

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
    assert_eq!(h.shown(dir), Some(50), "the pasted copy of the first clip");

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
                &vidcut::env::AvEnv::new(),
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
    assert_eq!(
        undone.result["undone"],
        json!(["Timeline marks: In 00:00:00:05, Out 00:00:00:15"])
    );

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
            h.shown(dir),
            Some(want),
            "first-frame clip, timeline frame {t}"
        );
    }

    step("Several tracks: B-roll on top, music mixed");
    // B-roll on V2 covers Track 1 in the preview, frame-exactly.
    h.ok(
        "edit.overwrite",
        json!({ "media": "m1", "range": "250f..270f", "at": 30, "track": "V2" }),
    );
    for (t, want) in [(29, 29), (30, 250), (49, 269), (50, 50)] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(h.shown(dir), Some(want), "with B-roll, timeline frame {t}");
    }
    // Music on A1 plays as a second audio stream, mixed with Track 1's
    // audio and turned down by its fader; the picture stays exact.
    let music = fixture::ensure(
        &FixtureSpec::new(
            VideoCodec::H264,
            Container::Mkv,
            W as u32,
            H as u32,
            (30, 1),
            120,
        )
        .audio_only(),
    )
    .unwrap();
    h.ok("media.add", json!({ "paths": [music] }));
    h.ok(
        "edit.overwrite",
        json!({ "media": "m2", "range": "0f..60f", "at": 10 }),
    );
    h.ok("track.set", json!({ "track": "A1", "volume_db": -6.0 }));
    h.ok("player.seek", json!({ "at": 31, "viewer": "timeline" }));
    assert_eq!(h.shown(dir), Some(251));
    assert_eq!(
        h.ok("player.state", json!({})).result["timeline"]["error"],
        Value::Null
    );
    // The Mixer's fader while dragged replaces the saved level in the mix.
    // The project does not change. A track that is not in the mix is an error.
    let rev = h.ok("project.summary", json!({})).rev;
    h.ok("player.play", json!({}));
    let preview = h.ok(
        "mixer.preview",
        json!({ "track": "A1", "volume_db": -20.0 }),
    );
    assert_eq!(preview.result["live"], true, "{preview:?}");
    let e = h
        .call("mixer.preview", json!({ "track": "V7", "volume_db": 0.0 }))
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Media, "{e}");
    h.ok("player.pause", json!({}));
    assert_eq!(h.ok("project.summary", json!({})).rev, rev);
    // Track 1 ripples: the B-roll and the music follow (decision D21).
    h.ok("edit.delete", json!({ "clips": "c1" }));
    h.ok("player.seek", json!({ "at": 0, "viewer": "timeline" }));
    assert_eq!(h.shown(dir), Some(250), "the B-roll moved with Track 1");
    h.ok("player.seek", json!({ "at": 20, "viewer": "timeline" }));
    assert_eq!(h.shown(dir), Some(220));
    // The monitoring mute changes nothing in the project.
    let rev = h.ok("project.summary", json!({})).rev;
    let s = h.ok("player.mute", json!({}));
    assert_eq!(s.result["muted"], true);
    assert_eq!(h.ok("project.summary", json!({})).rev, rev);

    step("Interlaced sources are deinterlaced");
    // Both fields show the frame's barcode; in the top eighth a block moves
    // each field, so a frame shown woven combs there.
    let interlaced = fixture::ensure(
        &FixtureSpec::new(
            VideoCodec::H264,
            Container::Mkv,
            W as u32,
            H as u32,
            (30, 1),
            60,
        )
        .interlaced(true)
        .no_audio(),
    )
    .unwrap();
    let added = h.ok("media.add", json!({ "paths": [interlaced] }));
    let media = added.changes.created[0].clone();
    let end = h.ok("project.summary", json!({})).result["duration"]
        .as_i64()
        .unwrap();
    h.ok(
        "edit.append",
        json!({ "media": media, "range": "10f..+20f" }),
    );
    for (t, want) in [(end, 10), (end + 7, 17), (end + 19, 29)] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(
            h.shown(dir),
            Some(want),
            "interlaced clip, timeline frame {t}"
        );
        let shot = image(&dir.join("shot.png"));
        assert!(combing(&shot) < 10.0, "frame {t} shows woven fields");
    }

    step("HDR sources are tone-mapped");
    // The barcode's white is SDR reference white in PQ: shown without tone
    // mapping it is gray and doesn't read.
    let pq = fixture::ensure(
        &FixtureSpec::new(
            VideoCodec::Hevc10,
            Container::Mkv,
            W as u32,
            H as u32,
            (30, 1),
            30,
        )
        .hdr(vidcut_core::model::Hdr::Pq)
        .no_audio(),
    )
    .unwrap();
    let added = h.ok("media.add", json!({ "paths": [pq] }));
    let media = added.changes.created[0].clone();
    let end = h.ok("project.summary", json!({})).result["duration"]
        .as_i64()
        .unwrap();
    h.ok(
        "edit.append",
        json!({ "media": media, "range": "5f..+10f" }),
    );
    h.ok(
        "player.seek",
        json!({ "at": end + 3, "viewer": "timeline" }),
    );
    assert_eq!(h.shown(dir), Some(8), "PQ clip in the preview");

    step("Many cuts: a seek waits only for the checks near its frame");
    // 120 clips of 4 frames from all over a long fixture, appended at once:
    // 120 tail checks, those nearest the playhead (the end, decision D15)
    // first. The seek there doesn't wait for the rest (gui.md §4).
    let long = fixture_file(1600);
    let added = h.ok("media.add", json!({ "paths": [long] }));
    let media = added.changes.created[0].clone();
    let commands: Vec<Value> = (0..120)
        .map(|k| {
            json!({ "method": "edit.append",
                    "params": { "media": media, "range": format!("{}f..+4f", 13 * k + 5) } })
        })
        .collect();
    h.ok("apply", json!({ "commands": commands }));
    let total = h.ok("project.summary", json!({})).result["duration"]
        .as_i64()
        .unwrap();
    // Live requests see the timeline as edited at once, though its reload
    // waits a moment to coalesce edits (gui.md §7).
    let state = h.ok("player.state", json!({})).result;
    assert_eq!(state["timeline"]["frames"].as_i64(), Some(total));
    let first = total - 4 * 120;
    let started = Instant::now();
    let seek = h.ok(
        "player.seek",
        json!({ "at": total - 3, "viewer": "timeline" }),
    );
    eprintln!(
        "first seek after 120 cuts: {:?}, {} tail checks still running",
        started.elapsed(),
        seek.result["preview_pending"]
    );
    assert_eq!(h.shown(dir), Some(13 * 119 + 5 + 1));
    let all = h.wait_for("every tail checked", |_| {
        h.ok("player.state", json!({})).result["preview_pending"] == 0
    });
    eprintln!("all tail checks done {all:?} later");
    for k in [0, 1, 37, 118, 119] {
        let at = first + 4 * k;
        for (t, want) in [(at, 13 * k + 5), (at + 3, 13 * k + 8)] {
            h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
            assert_eq!(
                h.shown(dir),
                Some(want as u32),
                "clip {k}, timeline frame {t}"
            );
        }
    }
    // Deleting the clip under the playhead shows the next one there at once,
    // without a seek. The second delete's reload is still waiting when the
    // screenshot asks.
    let at = first + 4 * 60 + 1;
    h.ok("player.seek", json!({ "at": at, "viewer": "timeline" }));
    assert_eq!(h.shown(dir), Some(13 * 60 + 6));
    h.ok("edit.delete", json!({ "clips": format!("V1@{at}f") }));
    h.ok("edit.delete", json!({ "clips": format!("V1@{at}f") }));
    assert_eq!(
        h.shown(dir),
        Some(13 * 62 + 6),
        "clip 62 moved under the playhead"
    );
    let state = h.ok("player.state", json!({})).result;
    assert_eq!(state["timeline"]["frames"].as_i64(), Some(total - 8));

    step("Stills and image sequences show frame for frame");
    // A still longer than one image list (chained), then a 24 fps sequence
    // on the 30 fps timeline, its pictures picked as export picks them.
    let still = fixture::ensure_images(&ImageSpec::still(77, W as u32, H as u32, "png")).unwrap();
    let seq = fixture::ensure_images(&ImageSpec::sequence(1, 60, 6, W as u32, H as u32)).unwrap();
    let r = h.ok("media.add", json!({ "paths": [still[0]] }));
    let still_id = r.changes.created[0].clone();
    let r = h.ok(
        "media.add",
        json!({ "paths": [seq[0]], "sequence": true, "fps": "24" }),
    );
    let seq_id = r.changes.created[0].clone();
    let end = h.ok("project.summary", json!({})).result["duration"]
        .as_i64()
        .unwrap();
    h.ok(
        "edit.append",
        json!({ "media": still_id, "range": "0f..+400f" }),
    );
    h.ok(
        "edit.append",
        json!({ "media": seq_id, "range": "5f..+30f" }),
    );
    let pick = |m: i64| {
        let t = (m as f64 - 0.5) / 30.0;
        (t * 24.0 - 1e-6).ceil() as u32
    };
    for (t, want) in [
        (end, 77),
        (end + 299, 77),
        (end + 350, 77),
        (end + 399, 77),
        (end + 400, pick(5)),
        (end + 411, pick(16)),
        (end + 429, pick(34)),
    ] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(h.shown(dir), Some(want), "timeline frame {t}");
    }
    // The source viewer shows a still as long as the still duration.
    h.ok("viewer.load", json!({ "media": still_id }));
    let state = h.ok("player.state", json!({})).result;
    assert_eq!(state["source"]["frames"], 150, "{state}");
    assert_eq!(h.shown(dir), Some(77));

    step("A failed tail check is tried again, then reported");
    // The source viewer's cut, which the timeline doesn't have: its retry
    // doesn't wait for the timeline. The file can't be read any more, so the
    // check fails twice.
    let gone = dir.join("unreadable.mkv");
    std::fs::copy(fixture_file(300), &gone).unwrap();
    let added = h.ok("media.add", json!({ "paths": [gone] }));
    let media = added.changes.created[0].clone();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gone, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _ = h.call("viewer.load", json!({ "media": media }));
    let took = h.wait_for("the notice", |s| {
        s.notices
            .iter()
            .any(|n| n.text.contains("checking a preview cut"))
    });
    assert!(
        took >= Duration::from_secs(1),
        "tried again first: {took:?}"
    );
    std::fs::set_permissions(&gone, std::fs::Permissions::from_mode(0o644)).unwrap();

    drop(h);
}

fn maps_contain_libmpv() -> bool {
    std::fs::read_to_string("/proc/self/maps")
        .unwrap()
        .lines()
        .any(|l| l.contains("libmpv"))
}

/// Playback through the live session with libmpv not mapped: barcodes across
/// a cut and a gap, a step back, forward and reverse play, the fixture beep,
/// and a lane fader that changes the next buffer.
#[test]
fn playback_without_libmpv() {
    assert!(
        !maps_contain_libmpv(),
        "libmpv is mapped before the session starts"
    );
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-nompv-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: this test runs in its own process when invoked by name. The
    // other test in this binary sets the same variable the same way.
    unsafe { std::env::set_var("XDG_STATE_HOME", dir.join("state")) };
    let settings = Settings {
        fps: Rational::new(30, 1).unwrap(),
        width: W as u32,
        height: H as u32,
        auto_format: false,
        ..Settings::default()
    };
    let clip = fixture::ensure(
        &FixtureSpec::new(
            VideoCodec::H264,
            Container::Mov,
            W as u32,
            H as u32,
            (30, 1),
            90,
        )
        .with_audio(AudioCodec::PcmS16, 48_000, 2),
    )
    .unwrap();
    let path = dir.join("p.vidcut");
    Store::create(&path, &Project::new(settings.clone())).unwrap();
    let h = Harness::start(&path);
    h.ok("media.add", json!({ "paths": [clip] }));
    h.ok("edit.append", json!({ "media": "m1", "range": "0f..60f" }));
    // Lift 10..20: a gap between source frame 9 and source frame 20.
    h.ok(
        "edit.delete_range",
        json!({ "range": "10f..20f", "lift": true }),
    );

    let at = |frame: i64| {
        h.ok("player.seek", json!({ "at": frame, "viewer": "timeline" }));
        h.shown(dir)
    };
    assert_eq!(at(0), Some(0), "frame 0");
    assert_eq!(at(9), Some(9), "the frame before the gap");
    assert_eq!(at(15), None, "the gap is black");
    assert_eq!(at(20), Some(20), "the frame after the gap");
    h.ok("player.seek", json!({ "at": 24, "viewer": "timeline" }));
    h.ok("player.step", json!({ "frames": -2, "viewer": "timeline" }));
    assert_eq!(h.position(), 22);
    assert_eq!(h.shown(dir), Some(22), "a step back");

    // Forward play crosses the cut into the gap, then the next clip.
    // Slow enough that a poll lands inside the ten-frame gap; pause before
    // reading the picture so the playhead cannot move on.
    h.ok("player.seek", json!({ "at": 8, "viewer": "timeline" }));
    h.ok("player.play", json!({ "speed": 0.1 }));
    let started = Instant::now();
    let mut saw_gap = false;
    let mut played = false;
    while started.elapsed() < Duration::from_secs(12) {
        let pos = h.position();
        if (10..20).contains(&pos) && !saw_gap {
            h.ok("player.pause", json!({}));
            let pos = h.position();
            assert!(
                (10..20).contains(&pos),
                "left the gap before pausing ({pos})"
            );
            assert_eq!(h.shown(dir), None, "playing through the gap at {pos}");
            saw_gap = true;
            h.ok("player.play", json!({ "speed": 0.1 }));
        }
        if pos >= 22 {
            h.ok("player.pause", json!({}));
            let pos = h.position();
            assert!(pos >= 20, "paused before the cut ({pos})");
            assert_eq!(
                h.shown(dir),
                Some(pos as u32),
                "playing after the cut at {pos}"
            );
            played = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.ok("player.pause", json!({}));
    assert!(saw_gap, "playback never showed the gap");
    assert!(played, "playback never passed the cut");

    // Reverse play steps back onto that same clip.
    h.ok("player.seek", json!({ "at": 25, "viewer": "timeline" }));
    h.ok("player.play", json!({ "speed": -1.0 }));
    let started = Instant::now();
    let mut reversed = None;
    while started.elapsed() < Duration::from_secs(4) {
        let pos = h.position();
        if (20..25).contains(&pos) {
            let code = h.shown(dir);
            assert_eq!(code, Some(pos as u32), "playing backwards at {pos}");
            reversed = code;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.ok("player.pause", json!({}));
    assert!(reversed.is_some(), "reverse play did not step back");

    // The beep is the first 10 ms of every source second. Frame 0 and
    // frame 30 (one second into the source, past the gap) each start on it;
    // a later buffer in that second is quiet.
    let beep_at = |frame: i64| {
        h.ok("player.seek", json!({ "at": frame, "viewer": "timeline" }));
        let seq = h.monitor()["seq"].as_u64().unwrap();
        h.ok("player.play", json!({ "speed": 1.0 }));
        let started = Instant::now();
        let mut heard = false;
        let mut quiet = false;
        while started.elapsed() < Duration::from_secs(2) {
            let m = h.monitor();
            if m["seq"].as_u64().unwrap() > seq && m["since_seek_peak"].as_f64().unwrap() > 0.2 {
                heard = true;
            }
            if heard && m["peak"].as_f64().unwrap() < 0.05 && h.position() > frame {
                quiet = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        h.ok("player.pause", json!({}));
        assert!(heard, "no beep at frame {frame}");
        assert!(quiet, "the beep at frame {frame} never went quiet");
    };
    beep_at(0);
    beep_at(30);

    // A steady tone. The track fader is already −6 dB in the plan; dragging
    // it to −12 must be heard as −12, not −18. Mute silences the sink. The
    // project does not change, and the picture keeps moving.
    let tone = fixture::ensure(
        &FixtureSpec::new(
            VideoCodec::H264,
            Container::Mov,
            W as u32,
            H as u32,
            (30, 1),
            90,
        )
        .with_audio(vidcut_av::fixture::AudioCodec::PcmS16, 48_000, 2)
        .sound(Sound::Tone),
    )
    .unwrap();
    let tone_path = dir.join("tone.vidcut");
    Store::create(&tone_path, &Project::new(settings)).unwrap();
    h.ok("project.open", json!({ "path": tone_path }));
    h.ok("media.add", json!({ "paths": [tone] }));
    h.ok("edit.append", json!({ "media": "m1", "range": "0f..90f" }));
    h.ok("track.set", json!({ "track": "V1", "volume_db": -6.0 }));
    h.ok("player.seek", json!({ "at": 0, "viewer": "timeline" }));
    assert!(
        (track_volume(&h, "V1") + 6.0).abs() < 1e-6,
        "saved fader {}",
        track_volume(&h, "V1")
    );
    h.take_heard();
    h.ok("player.play", json!({ "speed": 1.0 }));
    let started = Instant::now();
    let mut loud = 0.0;
    let mut loud_sink = 0.0;
    while started.elapsed() < Duration::from_secs(2) {
        loud = h.monitor()["rms"].as_f64().unwrap();
        let heard = h.heard.lock().unwrap();
        // Half a second, so the rms is the tone and not one short buffer.
        loud_sink = if heard.len() >= 48_000 {
            rms(&heard)
        } else {
            0.0
        };
        drop(heard);
        // −6 dB under the tone (rms about 0.035), not the unfaded 0.07.
        if (0.025..0.050).contains(&loud) && h.position() > 0 && (0.025..0.050).contains(&loud_sink)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        (0.025..0.050).contains(&loud),
        "saved −6 dB was not what played ({loud})"
    );
    assert!(
        (0.025..0.050).contains(&loud_sink),
        "the sink did not get the −6 dB tone ({loud_sink})"
    );

    // Mute silences the device, not only the meter. Playback continues.
    // Measure after the call returns: samples written while the request was
    // in flight are still the old level.
    h.ok("player.mute", json!({ "mute": true }));
    let at_mute = h.heard.lock().unwrap().len();
    let pos_mute = h.position();
    let started = Instant::now();
    let mut silenced = false;
    while started.elapsed() < Duration::from_secs(2) {
        // A thousand samples is less than one frame, so wait until the
        // playhead moves as well. The lock is not held across the call.
        let pos = h.position();
        let buf = h.heard.lock().unwrap();
        if pos > pos_mute && buf.len() > at_mute + 1000 {
            silenced = rms(&buf[at_mute..]) < 1e-4;
            break;
        }
        drop(buf);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(silenced, "mute did not silence the sink");
    assert!(h.position() > pos_mute, "mute stopped playback");
    h.ok("player.mute", json!({ "mute": false }));
    let at_open = h.heard.lock().unwrap().len();
    let started = Instant::now();
    let mut restored = false;
    while started.elapsed() < Duration::from_secs(2) {
        let buf = h.heard.lock().unwrap();
        if buf.len() > at_open + 1000 && (0.025..0.050).contains(&rms(&buf[at_open..])) {
            restored = true;
            break;
        }
        drop(buf);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(restored, "unmute did not restore the sink");

    let rev = h.ok("project.summary", json!({})).rev;
    let preview = h.ok(
        "mixer.preview",
        json!({ "track": "V1", "volume_db": -12.0 }),
    );
    assert_eq!(preview.result["live"], true);
    // Drop samples from before the fader moved. What arrives now is the
    // next buffer, at the fader's own level.
    h.take_heard();
    let started = Instant::now();
    let mut quiet = loud;
    let mut quiet_sink = 0.0;
    while started.elapsed() < Duration::from_secs(2) {
        quiet = h.monitor()["rms"].as_f64().unwrap();
        let heard = h.heard.lock().unwrap();
        quiet_sink = if heard.len() >= 48_000 {
            rms(&heard)
        } else {
            0.0
        };
        drop(heard);
        // −12 dB is about 0.018. The meter is one buffer, so its band is
        // wide; the sink ratio over half a second is the precise check.
        // Stacking −12 on the baked −6 would be about 0.009 (ratio 0.25),
        // and ignoring the drag would stay near the −6 level.
        let sink_ratio = if loud_sink > 0.0 {
            quiet_sink / loud_sink
        } else {
            0.0
        };
        if (0.012..0.025).contains(&quiet) && (0.45..0.56).contains(&sink_ratio) && h.position() > 0
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let ratio = quiet / loud;
    let sink_ratio = quiet_sink / loud_sink;
    let pos = h.position();
    let code = h.shown(dir);
    h.ok("player.pause", json!({}));
    assert!(
        (0.012..0.025).contains(&quiet),
        "the fader did not replace the saved −6 dB ({quiet} from {loud}, ratio {ratio})"
    );
    assert!(
        (0.45..0.56).contains(&sink_ratio),
        "the sink did not follow the fader ({quiet_sink} from {loud_sink}, ratio {sink_ratio})"
    );
    assert!(pos > 0, "playback stopped for the fader");
    assert_eq!(code, Some(pos as u32), "the picture kept moving");
    assert_eq!(h.ok("project.summary", json!({})).rev, rev);
    assert!(
        (track_volume(&h, "V1") + 6.0).abs() < 1e-6,
        "the preview fader was saved"
    );
    let missing = h
        .call("mixer.preview", json!({ "track": "V7", "volume_db": 0.0 }))
        .unwrap_err();
    assert_eq!(missing.code, ErrorCode::Media, "{missing}");

    // Forward play pauses itself at the last frame.
    let end = h.frames();
    h.ok(
        "player.seek",
        json!({ "at": end - 4, "viewer": "timeline" }),
    );
    h.ok("player.play", json!({ "speed": 1.0 }));
    let started = Instant::now();
    let mut stopped = false;
    while started.elapsed() < Duration::from_secs(3) {
        if !h.playing() && h.position() >= end {
            stopped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        stopped,
        "play did not stop at the end (playing {} at {})",
        h.playing(),
        h.position()
    );

    assert!(!maps_contain_libmpv(), "libmpv was loaded during playback");
    println!("BARCODE 0 9 black 20 22");
    println!("BEEP frame0=1 frame30=1");
    println!("FADER drop=1 saved=-6 ratio={ratio:.3} sink={sink_ratio:.3}");
    println!("MUTE zeros=1");
    println!("END stop=1");
    println!("MAPS libmpv=absent");
}

fn track_volume(h: &Harness, id: &str) -> f64 {
    let tl = h.ok("timeline.get", json!({}));
    tl.result["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()["volume_db"]
        .as_f64()
        .unwrap()
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// The default device accepts a buffer. Without PulseAudio the picture still
/// plays; this test says so and returns.
#[test]
fn the_default_playback_device_accepts_samples() {
    let mut device = live::Device::new("test");
    device.configure(48_000, 2);
    match device.wait_ready(Duration::from_secs(2)) {
        Ok(()) => {
            device.write(&vec![0.0; 48_000 / 50 * 2]);
            let started = Instant::now();
            while device.writes() == 0 && started.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                device.writes() > 0,
                "the device opened but accepted no samples"
            );
            println!("DEVICE open=1 writes={}", device.writes());
        }
        Err(e) => {
            // A missing server or library is this machine. A rejected spec
            // (invalid argument) is a broken device path and must fail.
            let unavailable = e.contains("connection refused") || e.contains("libpulse-simple");
            assert!(unavailable, "playback device rejected the format: {e}");
            eprintln!("playback device not available, skipped: {e}");
            println!("DEVICE open=0");
        }
    }
}
