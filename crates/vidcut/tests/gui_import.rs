//! Importing media under simulated input (egui_kittest, no window): numbered
//! images are asked about, one clip per run (an image sequence, spec D39) or
//! separate stills (deviation X17), and the import is one request.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use vidcut::gui::export::Dialogs;
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, _: PlayerId, done: Presented) {
        done();
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

/// A file dialog that picks whatever the test put in `files`.
struct FakeDialogs {
    files: Arc<Mutex<Vec<PathBuf>>>,
}

impl Dialogs for FakeDialogs {
    fn save_file(&self, _: &str, _: &Path, _: &str, _: &[&str]) -> Option<PathBuf> {
        None
    }
    fn pick_file(&self, _: &str, _: &Path, _: &[&str]) -> Option<PathBuf> {
        None
    }
    fn pick_folder(&self, _: &str, _: &Path) -> Option<PathBuf> {
        None
    }
    fn pick_files(&self, _: &str, _: &[(&str, &[&str])]) -> Vec<PathBuf> {
        self.files.lock().unwrap().clone()
    }
}

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Steps the GUI until `ready` holds (the file dialog answers on a thread).
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

/// The GUI's requests that changed the project, as (method, params).
fn gui_edits(h: &live::Handle) -> Vec<(String, Value)> {
    let log = call(h, "gui.log", json!({ "limit": 1000 })).result;
    log.as_array()
        .unwrap()
        .iter()
        .filter(|e| e["origin"] == "gui" && e["changed"] == true)
        .map(|e| {
            (
                e["method"].as_str().unwrap().to_string(),
                e["params"].clone(),
            )
        })
        .collect()
}

fn media_count(h: &live::Handle) -> usize {
    call(h, "media.list", json!({}))
        .result
        .as_array()
        .unwrap()
        .len()
}

#[test]
fn numbered_images_are_asked_about() {
    let dir = std::env::temp_dir().join(format!("vidcut-gui-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
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
    let frames = fixture::ensure_images(&ImageSpec::sequence(1, 24, 4, 320, 180)).unwrap();
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
        players: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    let picked = Arc::new(Mutex::new(Vec::new()));
    let mut ws = Workspace::new(h.clone());
    ws.set_dialogs(Arc::new(FakeDialogs {
        files: picked.clone(),
    }));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    harness.step();

    // 1. Some frames of a PNG run and a video: the run is offered as one
    // clip at the project's rate, typed over with 24. One request adds both.
    *picked.lock().unwrap() = vec![frames[3].clone(), frames[4].clone(), video.clone()];
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the question", |h| has(h, "Numbered images"));
    harness.step();
    assert!(has(&harness, "img_[0001-0024].png: 24 files"));
    // The rate field (the first match; the other is its text run).
    harness.query_all_by_value("30").next().unwrap().focus();
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    harness.step();
    harness
        .query_all_by_value("30")
        .next()
        .unwrap()
        .type_text("24");
    harness.step();
    harness.query_all_by_label("Import").last().unwrap().click();
    until(&mut harness, "the import", |_| media_count(&h) == 2);
    let edits = gui_edits(&h);
    assert_eq!(edits.len(), 1, "{edits:#?}");
    let (method, params) = &edits[0];
    assert_eq!(method, "apply");
    let add = &params["commands"][0]["params"];
    assert_eq!(
        (&add["sequence"], &add["fps"]),
        (&json!(true), &json!("24"))
    );
    let media = call(&h, "media.get", json!({ "media": "m1" })).result;
    assert_eq!(media["name"], "img_[0001-0024].png");
    assert_eq!(media["frames"], 30, "24 pictures at 24 fps: a second");

    // 2. The same frames as separate stills: each one a still.
    *picked.lock().unwrap() = vec![frames[0].clone(), frames[1].clone()];
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the question", |h| has(h, "Numbered images"));
    harness.step();
    harness.get_by_label("Separate stills").click();
    harness.step();
    harness.query_all_by_label("Import").last().unwrap().click();
    until(&mut harness, "the import", |_| media_count(&h) == 4);
    let (method, params) = gui_edits(&h).last().cloned().unwrap();
    assert_eq!(method, "media.add");
    assert_eq!(params["sequence"], json!(false));
    let media = call(&h, "media.get", json!({ "media": "m3" })).result;
    assert_eq!(media["still"], true);

    // 3. Files with no numbered run come in at once.
    *picked.lock().unwrap() = vec![video.clone()];
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the request", |_| gui_edits(&h).len() == 3);
    assert!(!has(&harness, "Numbered images"));

    drop(harness);
    h.shutdown();
    let _ = session.join();
    let _ = std::fs::remove_dir_all(&dir);
}
