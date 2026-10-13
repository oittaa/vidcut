//! Importing media under simulated input (egui_kittest, no window): numbered
//! images are asked about, one clip per run (an image sequence, spec D39) or
//! separate stills (deviation X17), and the import is one request. Media
//! that moved are marked and relinked from a folder (deviation X1).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use vidcut::gui::export::Dialogs;
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
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

/// A file dialog that picks whatever the test put in `files`.
struct FakeDialogs {
    files: Arc<Mutex<Vec<PathBuf>>>,
    /// How many times the dialog was opened.
    picks: Arc<Mutex<u32>>,
    /// The folder each one started in.
    folders: Arc<Mutex<Vec<PathBuf>>>,
    /// What a folder dialog picks.
    folder: Arc<Mutex<Option<PathBuf>>>,
}

impl Dialogs for FakeDialogs {
    fn save_file(&self, _: &str, _: &Path, _: &str, _: &[&str]) -> Option<PathBuf> {
        None
    }
    fn pick_file(&self, _: &str, folder: &Path, _: &[&str]) -> Option<PathBuf> {
        self.folders.lock().unwrap().push(folder.to_path_buf());
        None
    }
    fn pick_folder(&self, _: &str, _: &Path) -> Option<PathBuf> {
        self.folder.lock().unwrap().clone()
    }
    fn pick_files(&self, _: &str, folder: &Path, _: &[(&str, &[&str])]) -> Vec<PathBuf> {
        *self.picks.lock().unwrap() += 1;
        self.folders.lock().unwrap().push(folder.to_path_buf());
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
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-import-")
        .tempdir_in(vidcut_av::testdata::temp_dir())
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
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    // The project is open before the first key: the session opens it on a
    // thread of its own, and a request waits for it.
    assert_eq!(media_count(&h), 0);
    let picked = Arc::new(Mutex::new(Vec::new()));
    let picks = Arc::new(Mutex::new(0));
    let folders = Arc::new(Mutex::new(Vec::new()));
    let folder = Arc::new(Mutex::new(None));
    let dialogs = Arc::new(FakeDialogs {
        files: picked.clone(),
        picks: picks.clone(),
        folders: folders.clone(),
        folder: folder.clone(),
    });
    let mut ws = Workspace::new(h.clone());
    ws.set_dialogs(dialogs.clone());
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

    // 2. The same frames as separate stills: each one a still. The dialog
    // starts where the last files came from; the first one started in the
    // project's folder.
    *picked.lock().unwrap() = vec![frames[0].clone(), frames[1].clone()];
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the second dialog", |_| {
        folders.lock().unwrap().len() == 2
    });
    let images = frames[0].parent().unwrap().to_path_buf();
    assert_eq!(
        *folders.lock().unwrap(),
        [dir.to_path_buf(), images.clone()]
    );
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

    // 3. Files with no numbered run come in at once. (A file new to the
    // pool: adding one already there changes nothing.)
    let other = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        31,
    ))
    .unwrap();
    *picked.lock().unwrap() = vec![other];
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the request", |_| gui_edits(&h).len() == 3);
    assert!(!has(&harness, "Numbered images"));

    // 4. With the question up, timeline shortcuts do nothing. Escape cancels
    // like the Cancel button: nothing is imported.
    *picked.lock().unwrap() = vec![frames[8].clone(), frames[9].clone()];
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the question", |h| has(h, "Numbered images"));
    harness.step();
    let edits_before = gui_edits(&h).len();
    let media_before = media_count(&h);
    let picks_before = *picks.lock().unwrap();
    // A lone video: if Ctrl+I still opens the dialog, this imports at once.
    *picked.lock().unwrap() = vec![video.clone()];
    harness.key_press(egui::Key::I);
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    harness.step();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(400) {
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    assert!(has(&harness, "Numbered images"));
    assert_eq!(gui_edits(&h).len(), edits_before, "I set a mark");
    assert_eq!(media_count(&h), media_before);
    assert_eq!(
        *picks.lock().unwrap(),
        picks_before,
        "Ctrl+I opened a dialog"
    );
    harness.key_press(egui::Key::Escape);
    harness.step();
    assert!(!has(&harness, "Numbered images"));
    assert_eq!(media_count(&h), media_before);
    assert_eq!(gui_edits(&h).len(), edits_before);

    // 5. Separate stills imports even when the rate field is empty. Importing
    // as sequences first must refuse that field, so the test knows it is empty.
    *picked.lock().unwrap() = vec![frames[10].clone(), frames[11].clone()];
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the question", |h| has(h, "Numbered images"));
    harness.step();
    harness.query_all_by_value("30").next().unwrap().focus();
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    harness.step();
    harness.key_press(egui::Key::Backspace);
    harness.step();
    harness.query_all_by_label("Import").last().unwrap().click();
    until(&mut harness, "the rate error", |h| {
        has(h, "isn't a frame rate")
    });
    assert_eq!(media_count(&h), media_before);
    harness.get_by_label("Separate stills").click();
    harness.step();
    harness.query_all_by_label("Import").last().unwrap().click();
    until(&mut harness, "the import", |_| {
        media_count(&h) == media_before + 2
    });
    let (method, params) = gui_edits(&h).last().cloned().unwrap();
    assert_eq!(method, "media.add");
    assert_eq!(params["sequence"], json!(false));
    assert!(params.get("fps").is_none() || params["fps"].is_null());
    let listed = call(&h, "media.list", json!({})).result;
    let id = listed.as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap();
    let media = call(&h, "media.get", json!({ "media": id })).result;
    assert_eq!(media["still"], true);

    // 6. Open Project starts in the project's folder: none was opened from a
    // dialog yet. Media imports remember their folder across runs: a new
    // window starts where the last files came from.
    folders.lock().unwrap().clear();
    harness.get_by_label("File").click();
    // A menu ignores input in the frame it opens in.
    harness.step();
    harness.step();
    harness.get_by_label("Open Project…").click();
    until(&mut harness, "the project dialog", |_| {
        !folders.lock().unwrap().is_empty()
    });
    assert_eq!(*folders.lock().unwrap(), [dir.to_path_buf()]);
    drop(harness);
    folders.lock().unwrap().clear();
    picked.lock().unwrap().clear();
    let mut ws = Workspace::new(h.clone());
    ws.set_dialogs(dialogs);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    harness.step();
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::I);
    until(&mut harness, "the import dialog", |_| {
        !folders.lock().unwrap().is_empty()
    });
    assert_eq!(*folders.lock().unwrap(), [images]);

    // 7. A file that moved is marked missing; Relink Media looks for it in
    // the folder chosen and its subfolders, and relinks it as it is the same
    // file.
    let footage = dir.join("footage");
    std::fs::create_dir_all(&footage).unwrap();
    std::fs::copy(&video, footage.join("moved.mp4")).unwrap();
    let added = call(
        &h,
        "media.add",
        json!({ "paths": [footage.join("moved.mp4")] }),
    );
    let id = added.result["media"][0].as_str().unwrap().to_string();
    let disk = dir.join("disk");
    std::fs::create_dir_all(&disk).unwrap();
    // The pool's workers read a file just added for a while, and on Windows
    // a folder holding a file that is open can't move: until they let go.
    let started = std::time::Instant::now();
    while let Err(e) = std::fs::rename(&footage, disk.join("footage")) {
        let busy = cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied;
        assert!(
            busy && started.elapsed() < std::time::Duration::from_secs(20),
            "moving the footage: {e}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    until(&mut harness, "the missing file", |h| {
        has(h, "Relink Media (1)")
    });
    let listed = call(&h, "media.get", json!({ "media": id })).result;
    assert_eq!(listed["missing"], true);
    // The viewer says so, as Source Clip and on the timeline (where the
    // clip is red).
    call(&h, "viewer.load", json!({ "media": id }));
    until(&mut harness, "offline in the source", |h| {
        has(h, "Media Offline")
    });
    let clip = call(&h, "edit.append", json!({ "media": id })).result["clip"]
        .as_str()
        .unwrap()
        .to_string();
    let start = h.snapshot().project().unwrap().clip(&clip).unwrap().start;
    call(&h, "viewer.set", json!({ "mode": "timeline" }));
    call(
        &h,
        "player.seek",
        json!({ "at": start, "viewer": "timeline" }),
    );
    harness.step();
    until(&mut harness, "offline on the timeline", |h| {
        has(h, "Media Offline")
    });
    *folder.lock().unwrap() = Some(disk.clone());
    harness.get_by_label("Relink Media (1)").click();
    until(&mut harness, "the result", |h| has(h, "1 relinked"));
    let m = call(&h, "media.get", json!({ "media": id })).result;
    let moved = disk.join("footage").join("moved.mp4");
    let moved = vidcut_core::dirs::canonical(&moved).unwrap();
    assert_eq!(m["path"], json!(moved));
    assert!(m.get("missing").is_none(), "{m}");
    until(&mut harness, "no missing file", |h| {
        !has(h, "Relink Media (") && !has(h, "Media Offline")
    });
    harness.get_by_label("Close").click();
    until(&mut harness, "the window closed", |h| !has(h, "1 relinked"));

    drop(harness);
    h.shutdown();
    let _ = session.join();
}
