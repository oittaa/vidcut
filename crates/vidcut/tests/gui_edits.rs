//! Bugs the GUI had, each checked with one gesture under simulated input
//! (egui_kittest, no window). The tests click by position and press keys,
//! and check the project the gesture left. None looks for a button's or a
//! window's text.
//!
//! Every project starts with Track 1 holding c1 at 0..60, c2 at 60..120
//! and c3 at 120..180, cut from one 300-frame clip (m1) at 30 fps.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use tempfile::TempDir;
use vidcut::gui::export::Dialogs;
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
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

/// File dialogs that answer at once with `files`.
#[derive(Default)]
struct FakeDialogs {
    files: Mutex<Vec<PathBuf>>,
    /// How many times a file dialog was opened.
    picks: AtomicUsize,
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
    fn pick_files(&self, _: &str, _: &Path, _: &[(&str, &[&str])]) -> Vec<PathBuf> {
        self.picks.fetch_add(1, Ordering::SeqCst);
        self.files.lock().unwrap().clone()
    }
}

/// The GUI's state, config and cache, shared by the tests.
static HOME: LazyLock<TempDir> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: each test reads HOME before it starts anything, so this runs
    // before any session or GUI reads these.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_CACHE_HOME", dir.path().join("cache"));
    }
    dir
});

struct Gui {
    h: live::Handle,
    session: std::thread::JoinHandle<()>,
    harness: Harness<'static, Workspace>,
    dialogs: Arc<FakeDialogs>,
    _dir: TempDir,
}

impl Gui {
    fn new() -> Self {
        let _ = &*HOME;
        let media = fixture::ensure(&FixtureSpec::new(
            VideoCodec::H264,
            Container::Mp4,
            320,
            180,
            (30, 1),
            300,
        ))
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.vidcut");
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
        for range in ["0f..60f", "100f..160f", "200f..260f"] {
            call(&h, "edit.append", json!({ "media": "m1", "range": range }));
        }
        let dialogs = Arc::new(FakeDialogs::default());
        let mut ws = Workspace::new(h.clone());
        ws.set_dialogs(dialogs.clone());
        // Four pixels a frame from the timeline's left edge.
        ws.timeline_options_mut().snapping = false;
        ws.timeline_options_mut().ppf = 4.0;
        let harness = Harness::builder()
            .with_size(vec2(1600.0, 1000.0))
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(50)
            .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
        let mut gui = Self {
            h,
            session,
            harness,
            dialogs,
            _dir: dir,
        };
        gui.settle();
        gui
    }

    fn call(&self, method: &str, params: Value) -> Value {
        call(&self.h, method, params)
    }

    /// Lets the session catch up with the GUI's requests, then redraws.
    fn settle(&mut self) {
        self.call("player.state", json!({}));
        self.harness.step();
        self.harness.step();
    }

    fn key(&mut self, key: Key) {
        self.harness.key_press(key);
        self.harness.step();
    }

    fn click(&mut self, at: Pos2) {
        for pressed in [true, false] {
            self.harness.event(egui::Event::PointerMoved(at));
            self.harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            self.harness.step();
        }
    }

    /// Where `frame` is on `track` in the lower timeline, `dy` pixels below
    /// the track's top.
    fn at(&self, track: &str, frame: i64, dy: f32) -> Pos2 {
        let l = self.harness.state().timeline_layout().unwrap();
        let row = l.lower.get(track).unwrap().rect.rect;
        pos2(l.lower_scale.xf(frame), row.top() + dy)
    }

    /// The clips on `track` as (id, start, duration).
    fn clips(&self, track: &str) -> Vec<(String, i64, i64)> {
        let tl = self.call("timeline.get", json!({}));
        let t = tl["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == track)
            .unwrap();
        t["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["type"] == "clip")
            .map(|i| {
                (
                    i["id"].as_str().unwrap().to_string(),
                    i["start"].as_i64().unwrap(),
                    i["duration"].as_i64().unwrap(),
                )
            })
            .collect()
    }

    /// How many requests from the GUI changed the project.
    fn gui_changes(&self) -> usize {
        let log = self.call("gui.log", json!({ "limit": 1000 }));
        log.as_array()
            .unwrap()
            .iter()
            .filter(|e| e["origin"] == "gui" && e["changed"] == true)
            .count()
    }

    fn quit(self) {
        drop(self.harness);
        self.h.shutdown();
        let _ = self.session.join();
    }
}

fn call(h: &live::Handle, method: &str, params: Value) -> Value {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
        .result
}

fn track_one() -> Vec<(String, i64, i64)> {
    vec![
        ("c1".into(), 0, 60),
        ("c2".into(), 60, 60),
        ("c3".into(), 120, 60),
    ]
}

/// Clicking the target track's name a second time used to make Track 1 the
/// target again.
#[test]
fn a_second_click_keeps_the_target_track() {
    let mut gui = Gui::new();
    gui.call("track.add", json!({ "kind": "video" }));
    gui.call("track.set", json!({ "track": "V2", "name": "Titles" }));
    gui.settle();
    for _ in 0..2 {
        gui.harness.get_by_label("Titles").click();
        gui.harness.step();
    }
    gui.call(
        "media.marks.set",
        json!({ "media": "m1", "in": 10, "out": 40 }),
    );
    gui.call("selection.set", json!({ "media": "m1" }));
    gui.call("player.seek", json!({ "at": 90, "viewer": "timeline" }));
    gui.settle();

    // F10 overwrites on the target track.
    gui.key(Key::F10);
    gui.settle();
    assert_eq!(gui.clips("V2"), [("c4".into(), 90, 30)]);
    assert_eq!(gui.clips("V1"), track_one());
    gui.quit();
}

/// A second roll sent before the first one landed used to be lost.
#[test]
fn two_quick_rolls_add_up() {
    let mut gui = Gui::new();
    gui.click(gui.at("V1", 60, 6.0));
    gui.key(Key::Period);
    gui.key(Key::Period);
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 62),
            ("c2".into(), 62, 58),
            ("c3".into(), 120, 60)
        ]
    );
    assert_eq!(gui.harness.state().selected_edit(), Some(62));
    gui.quit();
}

/// A cut clicked on V2 used to be rolled on Track 1, which has a cut at the
/// same frame.
#[test]
fn a_cut_clicked_on_v2_rolls_on_v2() {
    let mut gui = Gui::new();
    gui.call("track.add", json!({ "kind": "video" }));
    for at in [40, 60] {
        gui.call(
            "edit.overwrite",
            json!({ "media": "m1", "range": "0f..20f", "at": at, "track": "V2", "video_only": true }),
        );
    }
    gui.settle();
    gui.click(gui.at("V2", 60, 6.0));
    gui.key(Key::Period);
    gui.settle();
    assert_eq!(
        gui.clips("V2"),
        [("c4".into(), 40, 21), ("c5".into(), 61, 19)]
    );
    assert_eq!(gui.clips("V1"), track_one());
    gui.quit();
}

/// A click on the J/L handle, the audio edit drawn 8 frames after the
/// picture cut, used to select the picture cut: `.` then rolled the picture.
#[test]
fn a_click_on_the_jl_handle_moves_the_audio_edit() {
    let mut gui = Gui::new();
    gui.call(
        "edit.audio_roll",
        json!({ "clip": "c1", "edge": "tail", "by": 8 }),
    );
    gui.settle();
    let l = gui.harness.state().timeline_layout().unwrap();
    let v1 = l.lower.get("V1").unwrap().rect;
    let audio_band = (v1.audio_top + v1.rect.bottom()) / 2.0;
    gui.click(pos2(l.lower_scale.xf(68), audio_band));
    gui.key(Key::Period);
    gui.settle();
    let tl = gui.call("timeline.get", json!({}));
    let tracks = tl["tracks"].as_array().unwrap();
    let items = &tracks.iter().find(|t| t["id"] == "V1").unwrap()["items"];
    assert_eq!(
        (&items[0]["audio"]["tail"], &items[1]["audio"]["head"]),
        (&json!(9), &json!(9))
    );
    assert_eq!(gui.clips("V1"), track_one());
    gui.quit();
}

/// While the question about numbered images was up, I still set a mark and
/// Ctrl+I opened another file dialog. Escape now cancels the import.
#[test]
fn the_numbered_images_question_takes_the_keys() {
    let mut gui = Gui::new();
    let frames = fixture::ensure_images(&ImageSpec::sequence(1, 4, 4, 320, 180)).unwrap();
    *gui.dialogs.files.lock().unwrap() = frames[..2].to_vec();
    gui.harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    let started = Instant::now();
    while !gui.harness.state().asking() {
        assert!(started.elapsed() < Duration::from_secs(10), "no question");
        std::thread::sleep(Duration::from_millis(5));
        gui.harness.step();
    }
    let changes = gui.gui_changes();

    gui.key(Key::I);
    gui.harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    gui.harness.step();
    // A dialog opens on a thread of its own.
    std::thread::sleep(Duration::from_millis(100));
    gui.settle();
    assert!(gui.harness.state().asking());
    assert_eq!(gui.gui_changes(), changes, "nothing changed the project");
    assert_eq!(gui.dialogs.picks.load(Ordering::SeqCst), 1);

    gui.key(Key::Escape);
    gui.settle();
    assert!(!gui.harness.state().asking());
    assert_eq!(
        gui.call("media.list", json!({})).as_array().unwrap().len(),
        1
    );
    gui.quit();
}
