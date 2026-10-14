//! Opening an OpenTimelineIO file from the start screen (deviation X9),
//! under simulated input (egui_kittest, no window).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use vidcut::gui::export::Dialogs;
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
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

/// File dialogs: Open Project picks `pick`, a save dialog names `save`.
struct FakeDialogs {
    pick: PathBuf,
    save: PathBuf,
}

impl Dialogs for FakeDialogs {
    fn save_file(&self, _: &str, _: &Path, _: &str, _: &[&str]) -> Option<PathBuf> {
        Some(self.save.clone())
    }
    fn pick_file(&self, _: &str, _: &Path, _: &[&str]) -> Option<PathBuf> {
        Some(self.pick.clone())
    }
    fn pick_folder(&self, _: &str, _: &Path) -> Option<PathBuf> {
        None
    }
}

/// Runs a request headless on `project`.
fn run(project: &Path, method: &str, params: Value) -> Value {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    vidcut::session::run(Some(project), e)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
        .result
}

/// Steps the GUI until `ready` holds.
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

/// Clicks `label` in a window that just came up: its first frame only
/// measures it.
fn click_new(harness: &mut Harness<'_, Workspace>, label: &str) {
    harness.step();
    harness.get_by_label(label).click();
}

#[test]
fn another_editors_file_opens_after_its_questions() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-open-")
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
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        90,
    ))
    .unwrap();
    // A file of two clips as another editor writes it: no vidcut project
    // in it.
    let source = dir.join("source.vidcut");
    run(&source, "project.create", json!({ "path": source }));
    run(&source, "media.add", json!({ "paths": [media] }));
    for range in ["0f..30f", "40f..70f"] {
        run(
            &source,
            "edit.append",
            json!({ "media": "m1", "range": range }),
        );
    }
    let otio = dir.join("cut.otio");
    run(&source, "export.start", json!({ "output": otio }));
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&otio).unwrap()).unwrap();
    doc["metadata"]["vidcut"]
        .as_object_mut()
        .unwrap()
        .remove("project");
    std::fs::write(&otio, doc.to_string()).unwrap();
    // The name next to it is taken.
    Store::create(&dir.join("cut.vidcut"), &Project::new(Settings::default())).unwrap();

    let (h, session) = live::start(live::Config {
        project: None,
        create: false,
        player: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    let other = dir.join("other.vidcut");
    let mut ws = Workspace::new(h.clone());
    ws.set_dialogs(Arc::new(FakeDialogs {
        pick: otio,
        save: other.clone(),
    }));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    harness.step();

    harness.get_by_label("Open Project…").click();
    until(&mut harness, "the taken name's question", |h| {
        has(h, "Save As…")
    });
    click_new(&mut harness, "Save As…");
    until(&mut harness, "the format question", |h| {
        has(h, "Choose it now")
    });
    click_new(&mut harness, "Create");
    until(&mut harness, "the project", |_| {
        h.snapshot()
            .project
            .as_ref()
            .is_some_and(|p| p.path.file_name() == other.file_name())
    });
    let snap = h.snapshot();
    let p = &snap.project.as_ref().unwrap().project;
    assert_eq!(p.tracks[0].clips.len(), 2);

    drop(harness);
    h.shutdown();
    let _ = session.join();
}
