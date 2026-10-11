//! A failed history sidecar write, as a command routed into the running GUI
//! sees it: the edit is kept, the status bar says so, and the JSON result
//! carries the same warning.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_core::model::Settings;
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

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method} {params}: {e}"))
}

fn notices(h: &live::Handle) -> String {
    h.snapshot()
        .notices
        .iter()
        .map(|n| n.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The one history file under `state`.
fn sidecar_file(state: &Path) -> PathBuf {
    let dir = state.join("vidcut").join("history");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("history dir {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    files.pop().unwrap()
}

#[test]
fn gui_command_puts_the_history_warning_on_the_result() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-hist-gui-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let state = dir.join("state");
    // SAFETY: this integration test is its own process.
    unsafe { std::env::set_var("XDG_STATE_HOME", &state) };

    let path = dir.join("p.vidcut");
    Store::create(&path, &Project::new(Settings::default())).unwrap();
    let (h, session) = live::start(live::Config {
        project: Some(path.clone()),
        create: false,
        players: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });

    let first = call(&h, "project.set", json!({ "ripple_on": false }));
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
    // A directory where the history file goes: writing it fails.
    let history = sidecar_file(&state);
    std::fs::remove_file(&history).unwrap();
    std::fs::create_dir(&history).unwrap();

    let failed = call(&h, "project.set", json!({ "ripple_on": true }));
    assert_eq!(failed.warnings.len(), 1, "{:?}", failed.warnings);
    assert!(
        failed.warnings[0].contains("undo history isn't"),
        "{}",
        failed.warnings[0]
    );
    assert!(
        notices(&h).contains(failed.warnings[0].as_str()),
        "status bar: {}",
        notices(&h)
    );
    // The edit is in the file, and this session still has its undo step.
    let reloaded = Store::open(&path).unwrap().load().unwrap();
    assert!(reloaded.project().settings.ripple_on);
    let live_undo = call(&h, "history.get", json!({}));
    assert_eq!(live_undo.result["undo"][0], "Project: Ripple On");

    h.shutdown();
    session.join().unwrap();
}
