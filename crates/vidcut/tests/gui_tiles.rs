//! The timeline's filmstrip (spec §5): each tile gets its own frame, made
//! in the background and kept on disk, and a new window takes the tiles
//! from there, even once the media can't be decoded any more.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::vec2;
use egui_kittest::Harness;
use serde_json::{Value, json};
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, _: PlayerId, done: Presented) {
        done(None);
    }
    fn capture_viewer(&self, _: PlayerId, done: Capture) {
        done(Err(vidcut_core::Error::usage("no renderer in tests")));
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn window(h: &live::Handle) -> Harness<'static, Workspace> {
    let mut ws = Workspace::new(h.clone());
    // 2 px a frame: the 20 s clip is 1200 px long.
    ws.timeline_options_mut().ppf = 2.0;
    let mut harness = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    harness.step();
    harness
}

/// Steps until the window keeps at least `n` tiles, none is on its way and
/// the disk holds as many; returns how many it keeps. Idle-class threads make
/// the tiles, and a busy machine starves them for seconds: the deadline is
/// generous.
fn tiles_settle(harness: &mut Harness<'_, Workspace>, cache: &Path, n: usize) -> usize {
    let started = Instant::now();
    loop {
        harness.step();
        let kept = harness.state().timeline_tiles().0;
        let pending = harness.state().timeline_tiles_pending();
        // The disk only once they are all made: reading it decodes them.
        let on_disk = (kept >= n && pending == 0).then(|| blocks(cache).1);
        if on_disk.as_ref().is_some_and(|f| f.len() >= kept) {
            return kept;
        }
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "{kept} tiles, {pending} on their way, on disk {on_disk:?}, waiting for {n}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The tile block files in the thumbnail cache, and the frames they hold.
fn blocks(cache: &Path) -> (usize, Vec<i64>) {
    let dir = cache.join("vidcut/thumbs");
    let mut frames = Vec::new();
    let mut files = 0;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.contains("-b") && name.ends_with(".thumbs") && name.contains(".t") {
            files += 1;
            for (f, _) in vidcut::thumbcache::load_tiles(&e.path()).unwrap() {
                frames.push(f);
            }
        }
    }
    frames.sort_unstable();
    (files, frames)
}

#[test]
fn tiles_are_made_kept_on_disk_and_found_again() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-tiles-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let cache = dir.join("cache");
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_CACHE_HOME", &cache);
        std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
    }
    let fixture = match fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 600).gop(120),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let clip = dir.join("clip.mp4");
    std::fs::copy(&fixture, &clip).unwrap();
    let project = dir.join("tiles.vidcut");
    Store::create(
        &project,
        &Project::new(Settings {
            fps: Rational::new(30, 1).unwrap(),
            width: 320,
            height: 180,
            auto_format: false,
            ..Settings::default()
        }),
    )
    .unwrap();
    let (h, session) = live::start(live::Config {
        project: Some(project),
        create: false,
        players: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    call(&h, "media.add", json!({ "paths": [clip] }));
    call(&h, "edit.append", json!({ "media": "m1" }));
    // The fixed playhead in the middle: the clip's start in view.
    call(&h, "player.seek", json!({ "at": 0, "viewer": "timeline" }));

    // 1. Every visible tile gets a picture of its own: the clip's first
    // frame for the first tile, frames of the clip further on for the rest.
    let mut harness = window(&h);
    let kept = tiles_settle(&mut harness, &cache, 3);
    let (files, frames) = blocks(&cache);
    assert!(files >= 1, "tiles go to disk in blocks");
    assert_eq!(frames.first(), Some(&0), "{frames:?}");
    assert!(frames.iter().all(|f| (0..600).contains(f)), "{frames:?}");
    assert_eq!(frames.len(), kept, "each tile kept once: {frames:?}");
    drop(harness);

    // 2. A new window finds them on disk: the clip can't be decoded now.
    std::fs::rename(dir.join("clip.mp4"), dir.join("moved.mp4")).unwrap();
    let mut harness = window(&h);
    assert_eq!(tiles_settle(&mut harness, &cache, kept), kept);
    drop(harness);

    h.shutdown();
    let _ = session.join();
}
