//! A theme file set with `preferences.set` shows in the running GUI at
//! once (egui_kittest, no window).

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};
use egui_kittest::Harness;
use serde_json::{Value, json};
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
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

fn call(h: &live::Handle, method: &str, params: Value) {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"));
}

fn panel(harness: &Harness<'_, Workspace>) -> Color32 {
    harness.ctx.global_style().visuals.panel_fill
}

#[test]
fn a_theme_file_set_in_the_preferences_shows_at_once() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-theme-")
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
    let file = dir.join("mine.toml");
    std::fs::write(&file, "extends = \"neon\"\n[colors]\npanel = \"#123456\"\n").unwrap();
    let (h, session) = live::start(live::Config {
        project: None,
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_max_steps(50)
        .build_ui_state(
            |ui, ws: &mut Workspace| ws.ui(ui),
            Workspace::new(h.clone()),
        );
    harness.step();
    // The default theme, Starship.
    assert_eq!(panel(&harness), Color32::from_rgb(0x0A, 0x0F, 0x16));
    call(
        &h,
        "preferences.set",
        json!({ "appearance": { "theme": file } }),
    );
    let started = Instant::now();
    while panel(&harness) != Color32::from_rgb(0x12, 0x34, 0x56) {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the theme didn't show: the panels are {:?}",
            panel(&harness)
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    drop(harness);
    h.shutdown();
    session.join().unwrap();
}
