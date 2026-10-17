use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
            started.elapsed() < Duration::from_secs(30),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
}

fn speed_requests(h: &live::Handle) -> Vec<Value> {
    let log = call(h, "gui.log", json!({ "limit": 1000 })).result;
    log.as_array()
        .unwrap()
        .iter()
        .filter(|e| e["origin"] == "gui" && e["method"] == "edit.change_speed")
        .map(|e| e["params"].clone())
        .collect()
}

fn has(harness: &Harness<'_, Workspace>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

#[test]
fn change_clip_speed_sends_one_request() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-speed-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
    }
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ))
    .unwrap();
    let path = dir.join("speed.vidcut");
    Store::create(
        &path,
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
        project: Some(path),
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    call(&h, "media.add", json!({ "paths": [media] }));
    call(
        &h,
        "edit.append",
        json!({ "media": "m1", "range": "0f..100f" }),
    );
    call(&h, "selection.set", json!({ "clips": ["c1"] }));
    let ripple = call(&h, "project.get", json!({})).result["settings"]["ripple_on"].clone();
    let ws = Workspace::new(h.clone());
    let mut harness = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    call(&h, "player.state", json!({}));
    harness.step();
    harness.step();

    harness.key_press(egui::Key::R);
    until(&mut harness, "the dialog", |h| has(h, "Reverse Speed"));
    harness.get_by_label("Reverse Speed").click();
    harness.step();
    harness.get_by_label("Change").click();
    until(&mut harness, "the request", |_| {
        speed_requests(&h).len() == 1
    });
    let r = &speed_requests(&h)[0];
    assert_eq!(
        (&r["clips"], &r["speed"], &r["ripple"]),
        (&json!(["c1"]), &json!(-1.0), &ripple)
    );
    until(&mut harness, "the dialog closed", |h| {
        !has(h, "Reverse Speed")
    });

    harness.get_by_label("Edit Actions").click();
    harness.step();
    harness.step();
    harness
        .query_all_by_label_contains("Change Clip Speed…")
        .next()
        .unwrap()
        .click();
    until(&mut harness, "the dialog", |h| has(h, "Freeze Frame"));
    harness.get_by_label("Freeze Frame").click();
    harness.step();
    harness.get_by_label("Change").click();
    until(&mut harness, "the request", |_| {
        speed_requests(&h).len() == 2
    });
    assert_eq!(speed_requests(&h)[1]["speed"], json!(0.0));

    drop(harness);
    h.shutdown();
    let _ = session.join();
}
