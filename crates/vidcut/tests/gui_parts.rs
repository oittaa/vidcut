use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

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

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn until(
    harness: &mut Harness<'_, Workspace>,
    what: &str,
    ready: impl Fn(&Harness<'_, Workspace>) -> bool,
) {
    let started = Instant::now();
    while !ready(harness) {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
}

fn has(harness: &Harness<'_, Workspace>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

fn greyed(harness: &Harness<'_, Workspace>, text: &str) -> bool {
    harness
        .get_by_label_contains(text)
        .accesskit_node()
        .is_disabled()
}

#[test]
fn what_cant_be_exported_is_greyed_out() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-parts-")
        .tempdir_in(vidcut_av::testdata::temp_dir())
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
        std::env::set_var(
            "VIDCUT_FFMPEG_WITHOUT",
            "libsvtav1,libaom-av1,librav1e,libmp3lame",
        );
    }
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ))
    .unwrap();
    let project = dir.join("p.vidcut");
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
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    call(&h, "media.add", json!({ "paths": [media] }));
    call(&h, "edit.append", json!({ "media": "m1" }));
    call(
        &h,
        "preferences.set",
        json!({ "gpu": { "acceleration": false } }),
    );
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(
            |ui, ws: &mut Workspace| ws.ui(ui),
            Workspace::new(h.clone()),
        );
    harness.step();

    harness.get_by_label("Quick Export").click();
    until(&mut harness, "preset tiles", |h| has(h, "ProRes 422 HQ\n"));
    harness.step();
    assert!(greyed(&harness, "AV1\n"));
    assert!(!greyed(&harness, "ProRes 422 HQ\n"));
    assert!(!greyed(&harness, "Export…"));

    harness.key_press(egui::Key::Escape);
    harness.step();
    harness.get_by_label("Export").click();
    until(&mut harness, "the dialog", |h| has(h, "Start Export"));
    harness.step();
    harness.get_by_label("Preset").click();
    until(&mut harness, "the presets", |h| has(h, "Audio: MP3"));
    harness.step();
    assert!(greyed(&harness, "Audio: MP3"));
    assert!(!greyed(&harness, "Audio: WAV"));

    drop(harness);
    h.shutdown();
    let _ = session.join();
    std::thread::sleep(Duration::from_millis(10));
}
