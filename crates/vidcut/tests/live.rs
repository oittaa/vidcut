//! The live session without a window: both players render through mpv's
//! software renderer on a harness thread (standing in for the GUI), and
//! requests go through `Handle::call` exactly as IPC clients' do. Checks that
//! what the viewer shows after seeks and edits is frame-exact, that edits from
//! other processes appear, and the clipboard and selection behavior.
//!
//! Needs libmpv (skips without it), no display.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Image, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};
use vidcut_mpv::{Mpv, SwRenderer, ffi::MPV_RENDER_UPDATE_FRAME};

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
        let stop = Arc::new(AtomicBool::new(false));
        // The "render thread": renders both players into buffers whenever
        // mpv has a frame or the session asks to present them, and answers
        // captures.
        let render = {
            let (port, stop) = (port.clone(), stop.clone());
            let players = [source.clone(), timeline.clone()];
            std::thread::spawn(move || {
                let mut renderers: Vec<SwRenderer> = players
                    .iter()
                    .map(|m| SwRenderer::new(m).unwrap())
                    .collect();
                let mut bufs = vec![vec![0u8; W * H * 4]; 2];
                while !stop.load(Ordering::Relaxed) {
                    if port.stall.load(Ordering::Relaxed) > 0
                        && renderers.iter().any(|r| r.next_frame_is_new())
                    {
                        let ms = port.stall.swap(0, Ordering::Relaxed);
                        std::thread::sleep(Duration::from_millis(ms));
                    }
                    let presents: Vec<_> = port.presents.lock().unwrap().drain(..).collect();
                    for (i, r) in renderers.iter_mut().enumerate() {
                        let present = presents.iter().any(|(id, _)| id.index() == i);
                        if r.update() & MPV_RENDER_UPDATE_FRAME != 0 || present {
                            r.render_rgb0(W, H, W * 4, &mut bufs[i]).unwrap();
                        }
                    }
                    for (_, done) in presents {
                        done();
                    }
                    let captures: Vec<_> = port.captures.lock().unwrap().drain(..).collect();
                    for (id, done) in captures {
                        // RGB0 → opaque RGBA.
                        let mut rgba = bufs[id.index()].clone();
                        for px in rgba.as_chunks_mut::<4>().0 {
                            px[3] = 255;
                        }
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
            renderer: port.clone(),
            notify: Arc::new(|| {}),
            socket: None,
        });
        Some(Self {
            handle,
            port,
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
    step("A late renderer");
    // A renderer that gets to a seek's frame only after libmpv stopped
    // waiting for it (200 ms) still has that frame on screen when the seek
    // returns (docs/decisions/gui.md §4).
    for t in [61, 149] {
        h.stall_next(Duration::from_millis(500));
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(h.shown(&dir), Some(expected(t)), "late renderer, frame {t}");
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
    assert_eq!(h.shown(&dir), Some(expected(120)));
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

    step("Several tracks: B-roll on top, music mixed");
    // M6: B-roll on V2 covers Track 1 in the preview, frame-exactly.
    h.ok(
        "edit.overwrite",
        json!({ "media": "m1", "range": "250f..270f", "at": 30, "track": "V2" }),
    );
    for (t, want) in [(29, 29), (30, 250), (49, 269), (50, 50)] {
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        assert_eq!(h.shown(&dir), Some(want), "with B-roll, timeline frame {t}");
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
    assert_eq!(h.shown(&dir), Some(251));
    assert_eq!(
        h.ok("player.state", json!({})).result["timeline"]["error"],
        Value::Null
    );
    // Track 1 ripples: the B-roll and the music follow (decision D21).
    h.ok("edit.delete", json!({ "clips": "c1" }));
    h.ok("player.seek", json!({ "at": 0, "viewer": "timeline" }));
    assert_eq!(h.shown(&dir), Some(250), "the B-roll moved with Track 1");
    h.ok("player.seek", json!({ "at": 20, "viewer": "timeline" }));
    assert_eq!(h.shown(&dir), Some(220));
    // The monitoring mute (p. 642) changes nothing in the project.
    let rev = h.ok("project.summary", json!({})).rev;
    let s = h.ok("player.mute", json!({}));
    assert_eq!(s.result["muted"], true);
    assert_eq!(h.ok("project.summary", json!({})).rev, rev);

    step("Interlaced sources are deinterlaced");
    // Both fields show the frame's barcode; in the top eighth a block moves
    // each field, so a frame shown woven combs there (M8).
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
            h.shown(&dir),
            Some(want),
            "interlaced clip, timeline frame {t}"
        );
        let shot = image(&dir.join("shot.png"));
        assert!(combing(&shot) < 10.0, "frame {t} shows woven fields");
    }

    step("HDR sources are tone-mapped");
    // The barcode's white is SDR reference white in PQ: shown without tone
    // mapping it is gray and doesn't read (M8).
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
    assert_eq!(h.shown(&dir), Some(8), "PQ clip in the preview");

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
    assert_eq!(h.shown(&dir), Some(13 * 119 + 5 + 1));
    let all = h.wait_for("every tail checked", |_| {
        h.ok("player.state", json!({})).result["preview_pending"] == 0
    });
    eprintln!("all tail checks done {all:?} later");
    for k in [0, 1, 37, 118, 119] {
        let at = first + 4 * k;
        for (t, want) in [(at, 13 * k + 5), (at + 3, 13 * k + 8)] {
            h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
            assert_eq!(
                h.shown(&dir),
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
    assert_eq!(h.shown(&dir), Some(13 * 60 + 6));
    h.ok("edit.delete", json!({ "clips": format!("V1@{at}f") }));
    h.ok("edit.delete", json!({ "clips": format!("V1@{at}f") }));
    assert_eq!(
        h.shown(&dir),
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
        assert_eq!(h.shown(&dir), Some(want), "timeline frame {t}");
    }
    // The source viewer shows a still as long as the still duration.
    h.ok("viewer.load", json!({ "media": still_id }));
    let state = h.ok("player.state", json!({})).result;
    assert_eq!(state["source"]["frames"], 150, "{state}");
    assert_eq!(h.shown(&dir), Some(77));

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
    let _ = std::fs::remove_dir_all(&dir);
}
