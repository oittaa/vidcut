//! The Keyboard Shortcuts window (egui_kittest, no window): F1, the Help
//! button and `gui.show help` open it, a row's ? opens its explanation,
//! the search narrows the rows, and F1 or Esc closes it.

use std::sync::Arc;

use eframe::egui::{self, Key, accesskit};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Presented, WindowChange};

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

const DYNAMIC_TRIM: &str = "What Dynamic Trim does";
const RIPPLE_OVERWRITE: &str = "What Ripple Overwrite does";

fn has(harness: &Harness<'_, Workspace>, label: &str) -> bool {
    harness.query_by_label(label).is_some()
}

#[test]
fn the_shortcuts_window_opens_explains_and_closes() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-help-")
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
        .with_size(egui::vec2(1400.0, 900.0))
        .build_ui_state(
            |ui, ws: &mut Workspace| ws.ui(ui),
            Workspace::new(h.clone()),
        );
    harness.step();
    assert!(!has(&harness, DYNAMIC_TRIM), "open before F1");

    harness.key_press(Key::F1);
    harness.run();
    assert!(has(&harness, DYNAMIC_TRIM), "F1 didn't open it");

    harness.get_by_role(accesskit::Role::TextInput).focus();
    harness.run();
    harness
        .get_by_role(accesskit::Role::TextInput)
        .type_text("dynamic");
    harness.run();
    assert_eq!(
        (has(&harness, DYNAMIC_TRIM), has(&harness, RIPPLE_OVERWRITE)),
        (true, false),
        "the search didn't narrow the rows"
    );

    harness.get_by_label(DYNAMIC_TRIM).click();
    harness.run();
    assert_eq!(
        harness
            .get_by_label(DYNAMIC_TRIM)
            .accesskit_node()
            .toggled(),
        Some(accesskit::Toggled::True),
        "its ? didn't open the explanation"
    );

    // The first Esc leaves the search field.
    harness.key_press(Key::Escape);
    harness.run();
    harness.key_press(Key::Escape);
    harness.run();
    assert!(!has(&harness, DYNAMIC_TRIM), "Esc didn't close it");

    harness.get_by_label("Help").click();
    harness.run();
    assert!(has(&harness, RIPPLE_OVERWRITE), "Help didn't open it");
    harness.key_press(Key::F1);
    harness.run();
    assert!(!has(&harness, RIPPLE_OVERWRITE), "F1 didn't close it");

    // What `gui.show help` runs in the app.
    let ctx = harness.ctx.clone();
    harness
        .state_mut()
        .show_panel(vidcut_core::request::GuiPanel::Help, &ctx);
    harness.run();
    assert!(
        has(&harness, RIPPLE_OVERWRITE),
        "gui.show help didn't open it"
    );
    drop(harness);
    h.shutdown();
    session.join().unwrap();
}
