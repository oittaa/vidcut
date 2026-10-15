//! The timeline's header column fits in every built-in theme (egui_kittest,
//! no window): the target track button sits within the overview's row,
//! whatever size the theme gives buttons.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Store};

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

#[test]
fn the_target_track_button_fits_beside_the_overview_in_every_theme() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-theme-layout-")
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
    let project = dir.join("p.vidcut");
    Store::create(
        &project,
        &Project::new(Settings {
            fps: Rational::new(25, 1).unwrap(),
            width: 1920,
            height: 1080,
            auto_format: false,
            ..Settings::default()
        }),
    )
    .unwrap();
    let (h, session) = live::start(live::Config {
        project: Some(project),
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_max_steps(50)
        .build_ui_state(
            |ui, ws: &mut Workspace| ws.ui(ui),
            Workspace::new(h.clone()),
        );
    harness.step();
    let mut outside = Vec::new();
    for theme in vidcut::themes::list() {
        call(
            &h,
            "preferences.set",
            json!({ "appearance": { "theme": theme.name } }),
        );
        let title = vidcut::themes::load(&theme.name).unwrap().title;
        let started = Instant::now();
        while h.snapshot().theme.title != title {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "{} didn't show",
                theme.name
            );
            std::thread::sleep(Duration::from_millis(5));
            harness.step();
        }
        // Applied, then laid out in it.
        for _ in 0..3 {
            harness.step();
        }
        let target = harness.get_by_label("Target V1").rect();
        let overview = harness.get_by_label("Timeline overview").rect();
        if target.top() < overview.top() || target.bottom() > overview.bottom() {
            outside.push(format!(
                "{}: the button spans {}..{}, the overview {}..{}",
                theme.name,
                target.top(),
                target.bottom(),
                overview.top(),
                overview.bottom()
            ));
        }
    }
    assert_eq!(outside, Vec::<String>::new());
    drop(harness);
    h.shutdown();
    session.join().unwrap();
}
