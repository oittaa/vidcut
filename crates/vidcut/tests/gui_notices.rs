//! The status bar's notices go away (egui_kittest, no window): an error
//! fades by itself, and its × dismisses it at once.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Presented, WindowChange};
use vidcut_core::Envelope;

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, done: Presented) {
        done(None);
    }
    fn capture_viewer(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no renderer in tests")));
    }
    fn capture_window(&self, done: Capture) {
        done(Err(vidcut_core::Error::usage("no window in tests")));
    }
    fn window(&self, _: WindowChange) {}
    fn quit(&self) {}
}

/// Whether a notice shows: each has its ×.
fn shown(harness: &Harness<'_, Workspace>) -> bool {
    harness.query_by_label("Dismiss").is_some()
}

/// Opens `file` from the GUI, as File > Open Project does, and steps until
/// its error shows.
fn open_broken(h: &live::Handle, harness: &mut Harness<'_, Workspace>, file: &std::path::Path) {
    let open = json!({ "method": "project.open", "params": { "path": file } });
    h.send(Envelope::from_json(open).unwrap());
    let started = Instant::now();
    while !shown(harness) {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "no notice showed"
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
}

#[test]
fn an_error_notice_fades_and_its_cross_dismisses_it() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-notices-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
    }
    let broken = dir.join("broken.vidcut");
    std::fs::write(&broken, "not a project").unwrap();
    let (h, session) = live::start(live::Config {
        project: None,
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    // Each step is a quarter of a second on the GUI's clock.
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(0.25)
        .build_ui_state(
            |ui, ws: &mut Workspace| ws.ui(ui),
            Workspace::new(h.clone()),
        );
    harness.step();
    open_broken(&h, &mut harness, &broken);
    // 15 seconds on: still there.
    for _ in 0..60 {
        harness.step();
    }
    assert!(shown(&harness), "gone after 15 s");
    // 25 seconds on: gone.
    for _ in 0..40 {
        harness.step();
    }
    assert!(!shown(&harness), "still there after 25 s");
    open_broken(&h, &mut harness, &broken);
    harness.get_by_label("Dismiss").click();
    harness.step();
    harness.step();
    assert!(!shown(&harness), "still there after its × was clicked");
    drop(harness);
    h.shutdown();
    session.join().unwrap();
}
