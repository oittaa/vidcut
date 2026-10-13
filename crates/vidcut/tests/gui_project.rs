//! A new project's format and Project Settings under simulated input
//! (egui_kittest, no window): New Project creates the file with the size and
//! frame rate chosen in one `project.open`, and Project Settings changes
//! them with one `project.set`, the frame rate only while the timeline is
//! empty (decision D37).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};
use vidcut::gui::export::Dialogs;
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::{Envelope, ErrorCode, Response, Result};

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

/// A save dialog that answers with `path`.
struct SaveAs {
    path: PathBuf,
}

impl Dialogs for SaveAs {
    fn save_file(&self, _: &str, _: &Path, _: &str, _: &[&str]) -> Option<PathBuf> {
        Some(self.path.clone())
    }
    fn pick_file(&self, _: &str, _: &Path, _: &[&str]) -> Option<PathBuf> {
        None
    }
    fn pick_folder(&self, _: &str, _: &Path) -> Option<PathBuf> {
        None
    }
    fn pick_files(&self, _: &str, _: &Path, _: &[(&str, &[&str])]) -> Vec<PathBuf> {
        Vec::new()
    }
}

fn try_call(h: &live::Handle, method: &str, params: Value) -> Result<Response> {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
}

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    try_call(h, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Steps the GUI until `ready` holds (dialogs and requests answer on other
/// threads).
fn until(harness: &mut Harness<'_, Workspace>, what: &str, ready: impl Fn() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
}

/// The open project's settings, if one is open.
fn settings(h: &live::Handle) -> Option<Value> {
    try_call(h, "project.get", json!({}))
        .ok()
        .map(|r| r.result["settings"].clone())
}

/// The GUI's requests, as (method, params).
fn gui_requests(h: &live::Handle) -> Vec<(String, Value)> {
    let log = call(h, "gui.log", json!({ "limit": 1000 })).result;
    log.as_array()
        .unwrap()
        .iter()
        .filter(|e| e["origin"] == "gui")
        .map(|e| {
            (
                e["method"].as_str().unwrap().to_string(),
                e["params"].clone(),
            )
        })
        .collect()
}

/// Opens `menu` and clicks `item` (a fresh menu ignores its first frame).
fn menu_item(harness: &mut Harness<'_, Workspace>, menu: &str, item: &str) {
    harness.get_by_label(menu).click();
    harness.step();
    harness.step();
    harness.get_by_label(item).click();
    harness.step();
    // A new window is measured in its first frame and placed in the next.
    harness.step();
}

/// Picks `item` from the combo box read as `label`.
fn choose(harness: &mut Harness<'_, Workspace>, label: &str, item: &str) {
    harness.get_by_label(label).click();
    harness.step();
    harness.step();
    harness.get_by_label(item).click();
    harness.step();
}

/// Types `text` over the frame rate field.
fn type_rate(harness: &mut Harness<'_, Workspace>, text: &str) {
    harness.get_by_label("Frame rate value").focus();
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    harness.step();
    harness.get_by_label("Frame rate value").type_text(text);
    harness.step();
}

#[test]
fn a_new_project_takes_the_format_chosen() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-project-")
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
    let video = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ))
    .unwrap();
    let (h, session) = live::start(live::Config {
        project: None,
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    let path = dir.join("film.vidcut");
    let mut ws = Workspace::new(h.clone());
    ws.set_dialogs(Arc::new(SaveAs { path: path.clone() }));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    harness.step();

    // 1. New Project, the format chosen now: HD at 12.5 frames a second, a
    // rate no list offers. One project.open creates the file with them.
    harness.get_by_label("New Project…").click();
    harness.step();
    harness.step();
    harness.get_by_label("Choose it now:").click();
    harness.step();
    choose(&mut harness, "Resolution", "HD 1280 × 720");
    type_rate(&mut harness, "12.5");
    assert!(harness.query_by_label_contains("a rate above 0").is_none());
    // Drop-frame timecode is for 29.97 and 59.94 (decision D47).
    assert!(harness.query_by_label("Drop-frame").is_none());
    harness.get_by_label("Create…").click();
    until(&mut harness, "the new project", || settings(&h).is_some());
    let s = settings(&h).unwrap();
    assert_eq!(
        (&s["fps"], &s["width"], &s["height"], &s["auto_format"]),
        (&json!("25/2"), &json!(1280), &json!(720), &json!(false))
    );
    let requests = gui_requests(&h);
    assert_eq!(requests.len(), 1, "{requests:#?}");
    assert_eq!(
        requests[0],
        (
            "project.open".to_string(),
            json!({ "path": path, "create": true, "fps": "12.5", "width": 1280, "height": 720 })
        )
    );

    // 2. Project Settings: another size and rate, drop-frame at it, one
    // project.set.
    menu_item(&mut harness, "File", "Project Settings…");
    choose(&mut harness, "Resolution", "Vertical 1080 × 1920");
    type_rate(&mut harness, "30000/1001");
    harness.get_by_label("Drop-frame").click();
    harness.step();
    harness.get_by_label("Apply").click();
    until(&mut harness, "the new settings", || {
        settings(&h).is_some_and(|s| s["width"] == 1080)
    });
    let s = settings(&h).unwrap();
    assert_eq!(
        (&s["fps"], &s["width"], &s["height"], &s["drop_frame"]),
        (
            &json!("30000/1001"),
            &json!(1080),
            &json!(1920),
            &json!(true)
        )
    );
    let requests = gui_requests(&h);
    let (method, params) = requests.last().unwrap();
    assert_eq!(requests.len(), 2, "{requests:#?}");
    assert_eq!(
        (
            method.as_str(),
            &params["width"],
            &params["height"],
            &params["fps"],
            &params["drop_frame"]
        ),
        (
            "project.set",
            &json!(1080),
            &json!(1920),
            &json!("30000/1001"),
            &json!(true)
        )
    );
    // It closes in the frame after the click.
    harness.step();
    assert!(harness.query_by_label("Apply").is_none(), "closed");

    // 3. With clips in the timeline the rate stays: its field is off.
    call(&h, "media.add", json!({ "paths": [video] }));
    call(&h, "edit.append", json!({ "media": "m1" }));
    menu_item(&mut harness, "File", "Project Settings…");
    assert!(
        harness
            .get_by_label("Frame rate value")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        harness
            .query_by_label_contains("only while the timeline is empty")
            .is_some()
    );
    harness.get_by_label("Cancel").click();
    harness.step();
    harness.step();
    assert!(harness.query_by_label("Apply").is_none(), "cancelled");

    // 4. A format for a file that already exists, or without create, is
    // refused rather than ignored.
    let e = try_call(
        &h,
        "project.open",
        json!({ "path": path, "create": true, "fps": "25" }),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
    let e = try_call(&h, "project.open", json!({ "path": path, "width": 640 })).unwrap_err();
    assert_eq!(e.code, ErrorCode::Usage, "{e}");

    drop(harness);
    h.shutdown();
    session.join().unwrap();
}
