//! The live session without a window: the viewer's player is the engine
//! (one for the source and the timeline), a harness thread stands in for
//! the GUI's renderer (it presents it and converts its pictures to RGBA
//! with the export's own filters, `Picture::to_rgba`), its sound goes to a
//! stand-in device that records it, and requests go through `Handle::call`
//! exactly as IPC clients' do.
//! Checks that what the viewer shows after seeks and edits is frame-exact,
//! that pictures keep with the sound as it is heard, that edits from other
//! processes appear, and the clipboard and selection behavior.
//!
//! No display, GPU or audio device.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Image, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
use vidcut_av::player::{PacedOutput, Picture, Player, Recording};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, ErrorCode, Project, Response, Store};

const W: usize = 320;
const H: usize = 180;
const RATE: u32 = 48_000;

/// How long after its buffer is due the stand-in device's sound is heard.
const LATENCY: Duration = Duration::from_millis(30);

type Engine = Arc<Player>;

/// The player's sound, as its stand-in device recorded it.
type Sound = Arc<Mutex<Option<Arc<Mutex<Recording>>>>>;

#[derive(Default)]
struct Port {
    engine: Mutex<Option<Engine>>,
    /// Engines attached so far (the same one again counts too).
    attached: AtomicU64,
    presents: Mutex<Vec<Presented>>,
    captures: Mutex<Vec<Capture>>,
    /// Milliseconds the render thread looks away once it next finds a new
    /// picture waiting, like a UI thread busy elsewhere.
    stall: AtomicU64,
    /// Every exact frame the viewer showed, and when it first did.
    shown: Mutex<Vec<(i64, Instant)>>,
}

impl live::Renderer for Port {
    fn attach(&self, engine: Option<Engine>) {
        *self.engine.lock().unwrap() = engine;
        self.attached.fetch_add(1, Ordering::Relaxed);
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
    port: Arc<Port>,
    /// The player's sound, as its stand-in device recorded it.
    sound: Sound,
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
                    let waiting = |s: &Screen| {
                        engine
                            .as_ref()
                            .and_then(|e| e.present(Instant::now()))
                            .is_some_and(|p| {
                                s.picture
                                    .as_ref()
                                    .is_none_or(|q| !Arc::ptr_eq(q, &p.picture))
                            })
                    };
                    if port.stall.load(Ordering::Relaxed) > 0 && waiting(&s) {
                        let ms = port.stall.swap(0, Ordering::Relaxed);
                        std::thread::sleep(Duration::from_millis(ms));
                    }
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
                    if let Some(f) = s.exact {
                        let mut log = port.shown.lock().unwrap();
                        if log.last().is_none_or(|(g, _)| *g != f) {
                            log.push((f, now));
                        }
                    }
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
        let sound: Sound = Arc::default();
        let audio = {
            let sound = sound.clone();
            Arc::new(move || {
                let (out, rec) = PacedOutput::recording(Duration::from_millis(10), LATENCY);
                *sound.lock().unwrap() = Some(rec);
                Box::new(out) as Box<dyn vidcut_av::player::AudioOutput>
            })
        };
        let (handle, session) = live::start(live::Config {
            project: Some(project.to_path_buf()),
            create: false,
            player: Some(live::PlayerConfig {
                audio,
                budget: vidcut_av::player::DEFAULT_BUDGET,
                gpu_pictures: false,
            }),
            preview_error: None,
            renderer: port.clone(),
            notify: Arc::new(|| {}),
            socket: None,
        });
        Self {
            handle,
            port,
            sound,
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

    /// The player's sound so far.
    fn sound(&self) -> Arc<Mutex<Recording>> {
        self.sound
            .lock()
            .unwrap()
            .clone()
            .expect("the player plays")
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

/// Milliseconds from `a` to `b`, negative when `b` is earlier.
fn ms_between(a: Instant, b: Instant) -> f64 {
    if b >= a {
        (b - a).as_secs_f64() * 1e3
    } else {
        -(a - b).as_secs_f64() * 1e3
    }
}

#[test]
fn live_session() {
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
    // Decoding on the CPU: no GPU needed (tests/gui.rs decodes on both).
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
    // Decision D15: the playhead moved past the appended clip, to the end.
    let state = h.ok("player.state", json!({})).result;
    assert_eq!(state["timeline"]["position"], 150);
    assert_eq!(state["timeline"]["frames"], 150);
    // The end shows black.
    assert_eq!(h.shown(dir), None, "the playhead at the end");

    step("Seeks are frame-exact at every boundary");
    // Seeks are frame-exact at every boundary, and agree with frame.render.
    let expected = |t: i64| -> u32 {
        match t {
            0..60 => (30 + t) as u32,
            60..90 => (t - 60) as u32,
            _ => (200 + t - 90) as u32,
        }
    };
    for t in [0, 1, 59, 60, 61, 89, 90, 91, 149] {
        let started = Instant::now();
        h.ok("player.seek", json!({ "at": t, "viewer": "timeline" }));
        let took = started.elapsed();
        assert_eq!(h.shown(dir), Some(expected(t)), "timeline frame {t}");
        eprintln!("  seek to {t}: {:.1} ms", took.as_secs_f64() * 1e3);
    }
    step("A late renderer");
    // A renderer that gets to a seek's frame late still has that frame on
    // screen when the seek returns (docs/decisions/gui.md §4).
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
    // A scrub (fast) to the same frame shows that frame too: every seek is
    // exact.
    h.stall_next(Duration::from_millis(400));
    let handle = h.handle.clone();
    let a = std::thread::spawn(move || {
        let e = Envelope::from_json(json!({ "method": "player.seek", "params": { "at": 100 } }))
            .unwrap();
        handle.call(e, Origin::Ipc, None, &mut |_| true)
    });
    h.wait_stalled();
    h.ok("player.seek", json!({ "at": 100, "fast": true }));
    a.join().unwrap().expect("the same frame, exact either way");
    assert_eq!(h.shown(dir), Some(expected(100)));

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
    // An edit's reload while that seek is in flight keeps playing from
    // where the clock is, and the seek stays the request.
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
    assert_eq!(
        h.ok("player.state", json!({})).result["timeline"]["playing"],
        true
    );
    h.ok("player.pause", json!({}));
    h.ok("track.set", json!({ "track": "V1", "volume_db": 0.0 }));

    step("Pictures keep with the sound as it is heard");
    // Play the whole timeline from the start, the sound to a stand-in
    // device heard 30 ms after it is due. The fixtures beep at the start
    // of every second of source: c1's source frames 30 and 60 (timeline 0
    // and 30), c3's 210 and 240 (timeline 100 and 130). Each beep's frame
    // must show as its beep is heard.
    h.ok("player.audio_scrubbing", json!({ "on": false }));
    h.ok("player.seek", json!({ "at": 0 }));
    let sound = h.sound();
    let from = sound.lock().unwrap().samples.len() / 2;
    h.port.shown.lock().unwrap().clear();
    h.ok("player.play", json!({}));
    h.wait_for("the end", |s| {
        !s.timeline.playing && s.timeline.position == 150
    });
    let rec = sound.lock().unwrap();
    let mono: Vec<f32> = rec.samples.chunks(2).map(|c| c[0]).collect();
    let onsets: Vec<usize> = vidcut_av::beep::onsets(&mono[from..], 0.1, 4_800)
        .into_iter()
        .map(|i| i + from)
        .collect();
    assert_eq!(onsets.len(), 4, "a beep a second: {onsets:?}");
    let shown = h.port.shown.lock().unwrap().clone();
    let mut offsets = Vec::new();
    for (&onset, frame) in onsets.iter().zip([0, 30, 100, 130]).skip(1) {
        let heard = rec.heard_at(onset, RATE).unwrap();
        let (_, at) = shown
            .iter()
            .find(|(f, _)| *f >= frame)
            .unwrap_or_else(|| panic!("frame {frame} never showed"));
        offsets.push(ms_between(heard, *at));
    }
    drop(rec);
    eprintln!("  picture after its beep (ms): {offsets:.1?}");
    for o in &offsets {
        // The harness presents every 2 ms; a display at 60 Hz sees the
        // clock's frame at its next refresh.
        assert!(
            (-1.0..=12.0).contains(o),
            "picture against sound: {offsets:?}"
        );
    }
    // Every frame showed, in order.
    let frames: Vec<i64> = shown.iter().map(|(f, _)| *f).collect();
    let missing = (1..150).filter(|f| !frames.contains(f)).count();
    assert!(missing <= 3, "{missing} frames never showed while playing");
    assert!(frames.windows(2).all(|w| w[0] < w[1]), "in order");

    step("Audio scrubbing");
    // Off: a step plays nothing. On (decision D42): the frame's sound.
    let quiet_since = |n: usize| {
        let rec = sound.lock().unwrap();
        rec.samples[n * 2..].iter().all(|s| s.abs() < 1e-6)
    };
    h.ok("player.seek", json!({ "at": 29 }));
    std::thread::sleep(Duration::from_millis(100));
    let n = sound.lock().unwrap().samples.len() / 2;
    h.ok("player.step", json!({ "frames": 1 }));
    std::thread::sleep(Duration::from_millis(150));
    assert!(quiet_since(n), "scrubbing is off");
    let state = h.ok("player.audio_scrubbing", json!({})).result;
    assert_eq!(state["audio_scrubbing"], true, "toggled");
    h.ok("player.seek", json!({ "at": 29 }));
    std::thread::sleep(Duration::from_millis(150));
    let n = sound.lock().unwrap().samples.len() / 2;
    h.ok("player.step", json!({ "frames": 1 }));
    std::thread::sleep(Duration::from_millis(150));
    assert!(!quiet_since(n), "frame 30's beep plays");
    assert_eq!(h.shown(dir), Some(expected(30)));

    step("Steps and frame.render");
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
    // Steps quicker than frames decode, as a held arrow key makes them: the
    // last lands exactly. The GUI's overtaken steps aren't errors.
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
    // Playing backward: the clock runs back.
    h.ok("player.play", json!({ "speed": -1.0 }));
    std::thread::sleep(Duration::from_millis(400));
    let state = h.ok("player.pause", json!({})).result;
    let back = state["timeline"]["position"].as_i64().unwrap();
    assert!((100..=120).contains(&back), "played back to {back}");

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
    // Q-style switching keeps each playhead where it was.
    h.ok("viewer.set", json!({ "mode": "timeline" }));
    h.ok("viewer.set", json!({ "mode": "source_tape" }));
    assert_eq!(h.shown(dir), Some(10));

    step("Switching shows the frame kept ready");
    // One engine plays the source and the timeline. The frame of the one
    // not shown is decoded while nothing else is and kept, so switching
    // shows it at once. An edit function from the source moves the
    // timeline's playhead (decision D15): its new frame is kept ready too.
    // The timeline now: m1 50..90, m2 0..30, m1 200..260.
    let engine = || h.port.engine.lock().unwrap().as_ref().map(Arc::as_ptr);
    let one = engine();
    assert!(one.is_some());
    h.ok("player.seek", json!({ "at": 10, "viewer": "timeline" }));
    h.ok("viewer.load", json!({ "media": "m1", "at": "120f" }));
    // Timeline 10..20 becomes m1 120..130; the playhead moves to 20, which
    // still shows m1 frame 70.
    h.ok(
        "edit.overwrite",
        json!({ "media": "m1", "range": "120f..130f", "at": 10 }),
    );
    let took = h.wait_for("the timeline's new frame kept ready", |s| {
        s.timeline.position == 20 && s.timeline.ready
    });
    eprintln!("  the timeline's frame was ready {took:?} after the edit");
    let started = Instant::now();
    h.ok("viewer.set", json!({ "mode": "timeline" }));
    eprintln!("  switched to the timeline in {:?}", started.elapsed());
    assert_eq!(h.shown(dir), Some(70));
    h.wait_for("the source's frame kept ready", |s| s.source.ready);
    let started = Instant::now();
    h.ok("viewer.set", json!({ "mode": "source_clip" }));
    eprintln!("  switched to the source in {:?}", started.elapsed());
    assert_eq!(h.shown(dir), Some(120));
    assert_eq!(engine(), one, "one engine plays both");
    h.ok("history.undo", json!({}));

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

    // A clip from a file's very first frame, and the frames before a
    // keyframe that a GOP decodes after it (B-frames).
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
    // Music on A1 is mixed with Track 1's audio and turned down by its
    // fader; the picture stays exact.
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
    // The Mixer's fader while dragged: the playing preview's level changes
    // at once, the project doesn't.
    let rev = h.ok("project.summary", json!({})).rev;
    h.ok("player.play", json!({}));
    let preview = h.ok(
        "mixer.preview",
        json!({ "track": "A1", "volume_db": -20.0 }),
    );
    assert_eq!(preview.result["live"], true);
    // A track with no sound in the mix: says so.
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
    h.ok("player.mute", json!({ "mute": false }));

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

    step("HDR sources are tone-mapped, turned ones turned upright");
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
    // A phone-style clip stored sideways with a rotation.
    let turned = fixture::ensure(
        &FixtureSpec::new(
            VideoCodec::H264,
            Container::Mp4,
            W as u32,
            H as u32,
            (30, 1),
            30,
        )
        .rotation(90)
        .no_audio(),
    )
    .unwrap();
    let added = h.ok("media.add", json!({ "paths": [pq, turned] }));
    let (pq_id, turned_id) = (
        added.changes.created[0].clone(),
        added.changes.created[1].clone(),
    );
    let end = h.ok("project.summary", json!({})).result["duration"]
        .as_i64()
        .unwrap();
    h.ok(
        "edit.append",
        json!({ "media": pq_id, "range": "5f..+10f" }),
    );
    h.ok(
        "edit.append",
        json!({ "media": turned_id, "range": "3f..+10f" }),
    );
    h.ok(
        "player.seek",
        json!({ "at": end + 3, "viewer": "timeline" }),
    );
    assert_eq!(h.shown(dir), Some(8), "PQ clip in the preview");
    h.ok(
        "player.seek",
        json!({ "at": end + 14, "viewer": "timeline" }),
    );
    assert_eq!(h.shown(dir), Some(7), "turned clip, upright");

    step("Many cuts: the first seek doesn't wait for them");
    // 120 clips of 4 frames from all over a long fixture, appended at once.
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
    h.ok(
        "player.seek",
        json!({ "at": total - 3, "viewer": "timeline" }),
    );
    eprintln!("first seek after 120 cuts: {:?}", started.elapsed());
    assert_eq!(h.shown(dir), Some(13 * 119 + 5 + 1));
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
    // A long still, then a 24 fps sequence on the 30 fps timeline, its
    // pictures picked as export picks them.
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

    step("Proxies play in the viewer, frame for frame");
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
    h.ok("project.set", json!({ "use_proxies": false }));

    step("An unreadable file fails the seek, with the reason");
    let gone = dir.join("unreadable.mp4");
    std::fs::copy(fixture_file(300), &gone).unwrap();
    let added = h.ok("media.add", json!({ "paths": [gone] }));
    let media = added.changes.created[0].clone();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gone, std::fs::Permissions::from_mode(0o000)).unwrap();
    let started = Instant::now();
    let e = h
        .call("viewer.load", json!({ "media": media }))
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Media, "{e}");
    assert!(started.elapsed() < Duration::from_secs(5), "{e}");
    let state = h.ok("player.state", json!({})).result;
    assert!(state["source"]["error"].is_string(), "{state}");
    std::fs::set_permissions(&gone, std::fs::Permissions::from_mode(0o644)).unwrap();

    step("Switching back: the frame decoded meanwhile, the fader held");
    // A new project: a new engine, nothing decoded yet.
    h.ok(
        "project.open",
        json!({ "path": dir.join("switch.vidcut"), "create": true }),
    );
    h.ok("media.add", json!({ "paths": [fixture_file(300)] }));
    h.ok("edit.append", json!({ "media": "m1", "range": "0f..90f" }));
    h.ok("player.audio_scrubbing", json!({ "on": false }));
    h.ok("player.seek", json!({ "at": 10, "viewer": "timeline" }));
    h.ok("viewer.load", json!({ "media": "m1", "at": "200f" }));
    // Nothing else to publish a snapshot meanwhile: the pool's pictures
    // and sound are done.
    h.wait_for("the pool's thumbnails and waveform", |s| {
        s.thumbs.contains_key("m1") && s.waveforms.contains_key("m1")
    });
    // An edit function from the source moves the timeline's playhead to
    // frame 50, never decoded: it decodes while the source shows, and the
    // snapshot says when it is ready.
    h.ok(
        "edit.overwrite",
        json!({ "media": "m1", "range": "250f..260f", "at": 40 }),
    );
    let took = h.wait_for("the timeline's frame decoded while hidden", |s| {
        s.timeline.position == 50 && s.timeline.ready
    });
    eprintln!("  the timeline's frame was decoded {took:?} after the edit");
    h.ok("viewer.set", json!({ "mode": "timeline" }));
    assert_eq!(h.shown(dir), Some(50));
    // A fader held while the viewer goes to the source and back (Q twice)
    // is heard as held: the beeps at frames 0 and 30 come 40 dB down.
    let sound = h.sound();
    let peak_of_play = || {
        h.ok("player.seek", json!({ "at": 0, "viewer": "timeline" }));
        let from = sound.lock().unwrap().samples.len();
        h.ok("player.play", json!({}));
        std::thread::sleep(Duration::from_millis(1300));
        h.ok("player.pause", json!({}));
        let rec = sound.lock().unwrap();
        rec.samples[from..].iter().fold(0f32, |m, s| m.max(s.abs()))
    };
    // Track 1's clips carry their sound: its fader is V1's.
    let preview = h.ok(
        "mixer.preview",
        json!({ "track": "V1", "volume_db": -40.0 }),
    );
    assert_eq!(preview.result["live"], true);
    h.ok("viewer.set", json!({ "mode": "source_clip" }));
    h.ok("viewer.set", json!({ "mode": "timeline" }));
    let held = peak_of_play();
    h.ok("mixer.preview", json!({ "track": "V1", "volume_db": 0.0 }));
    let saved = peak_of_play();
    eprintln!("  beeps peak at {held:.4} held at -40 dB, {saved:.4} at 0 dB");
    assert!(saved > 0.1, "the beeps play: {saved}");
    assert!(held < saved * 0.05, "held {held}, saved {saved}");

    drop(h);
}
