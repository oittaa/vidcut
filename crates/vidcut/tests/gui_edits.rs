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
use vidcut::gui::workspace::{
    DynamicMode, DynamicTarget, EditSel, Side, TrackRef, Workspace, heard,
};
use vidcut::live::{self, Audition, Capture, GuiChange, GuiDone, Origin, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::request::{TrimSide, ViewerMode};
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Store};

#[derive(Default)]
struct Port {
    changes: Mutex<Vec<(Vec<GuiChange>, GuiDone)>>,
}

impl live::Renderer for Port {
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
    fn gui(&self, changes: Vec<GuiChange>, done: GuiDone) {
        self.changes.lock().unwrap().push((changes, done));
    }
}

#[derive(Default)]
struct FakeDialogs {
    files: Mutex<Vec<PathBuf>>,
    folder: Mutex<Option<PathBuf>>,
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
        self.folder.lock().unwrap().clone()
    }
    fn pick_files(&self, _: &str, _: &Path, _: &[(&str, &[&str])]) -> Vec<PathBuf> {
        self.picks.fetch_add(1, Ordering::SeqCst);
        self.files.lock().unwrap().clone()
    }
}

static HOME: LazyLock<TempDir> = LazyLock::new(|| {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: every test forces `HOME` first, so no other thread touches the environment yet.
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
    port: Arc<Port>,
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
        let port = Arc::new(Port::default());
        let (h, session) = live::start(live::Config {
            project: Some(path),
            create: false,
            player: None,
            preview_error: Some("no preview in tests".into()),
            renderer: port.clone(),
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
            port,
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

    fn ask(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
        let h = self.h.clone();
        let asked = std::thread::spawn(move || {
            h.call(e, Origin::Ipc, None, &mut |_| true)
                .map(|r| r.result)
                .map_err(|e| e.message)
        });
        let started = Instant::now();
        while !asked.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(30), "{method}");
            let changes: Vec<_> = self.port.changes.lock().unwrap().drain(..).collect();
            for (changes, done) in changes {
                let ctx = self.harness.ctx.clone();
                let made = self.harness.state_mut().gui_change(changes, &ctx);
                done(made.map(|()| self.harness.state().gui_state()));
            }
            self.harness.step();
            std::thread::sleep(Duration::from_millis(2));
        }
        asked.join().unwrap()
    }

    fn settle(&mut self) {
        self.call("player.state", json!({}));
        self.harness.step();
        self.harness.step();
    }

    fn key(&mut self, key: Key) {
        self.harness.key_press(key);
        self.harness.step();
    }

    fn press(&mut self, key: Key, pressed: bool) {
        self.harness.event(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        self.harness.step();
    }

    fn hold(&mut self, key: Key, tap: Key) {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.harness.event(event(true));
        self.harness.step();
        self.harness.key_press(tap);
        self.harness.step();
        self.harness.event(event(false));
        self.settle();
    }

    fn position(&self) -> i64 {
        self.h.snapshot().timeline.position
    }

    fn source_in(&self, clip: &str) -> Value {
        let tl = self.call("timeline.get", json!({}));
        let tracks = tl["tracks"].as_array().unwrap();
        let items = &tracks.iter().find(|t| t["id"] == "V1").unwrap()["items"];
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == clip)
            .unwrap()["source_in"]
            .clone()
    }

    fn click(&mut self, at: Pos2) {
        self.click_with(at, egui::PointerButton::Primary);
    }

    fn click_with(&mut self, at: Pos2, button: egui::PointerButton) {
        for pressed in [true, false] {
            self.harness.event(egui::Event::PointerMoved(at));
            self.harness.event(egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            });
            self.harness.step();
        }
    }

    fn double_click(&mut self, at: Pos2) {
        self.click(at);
        self.click(at);
    }

    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.harness.event(egui::Event::PointerMoved(from));
        self.harness.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.harness.step();
        for i in 1..=4 {
            self.harness.event(egui::Event::PointerMoved(
                from + (to - from) * (i as f32 / 4.0),
            ));
            self.harness.step();
        }
        self.harness.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        self.harness.step();
    }

    fn at(&self, track: &str, frame: i64, dy: f32) -> Pos2 {
        let l = self.harness.state().timeline_layout().unwrap();
        let row = l.lower.get(track).unwrap().rect.rect;
        pos2(l.lower_scale.xf(frame), row.top() + dy)
    }

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

fn cut(at: i64, side: Side) -> DynamicTarget {
    DynamicTarget::Edit(EditSel {
        at,
        side,
        track: TrackRef::MAIN,
    })
}

fn run_heard(target: DynamicTarget, mode: DynamicMode, end_edge: bool, speed: f64) -> Audition {
    let gui = Gui::new();
    let p = gui.h.snapshot().project.clone().unwrap().project;
    let heard = heard(&p, &target, mode, end_edge, 0, speed).unwrap();
    gui.quit();
    heard
}

fn heard_timeline(a: Audition) -> (Project, i64, f64) {
    (*a.timeline, a.at, a.speed)
}

fn track_one() -> Vec<(String, i64, i64)> {
    vec![
        ("c1".into(), 0, 60),
        ("c2".into(), 60, 60),
        ("c3".into(), 120, 60),
    ]
}

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

    gui.key(Key::F10);
    gui.settle();
    assert_eq!(gui.clips("V2"), [("c4".into(), 90, 30)]);
    assert_eq!(gui.clips("V1"), track_one());
    gui.quit();
}

#[test]
fn ctrl_wheel_zooms_the_lower_timeline() {
    let mut gui = Gui::new();
    gui.harness
        .event(egui::Event::PointerMoved(gui.at("V1", 30, 6.0)));
    gui.harness.event_modifiers(
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::COMMAND,
        },
        Modifiers::COMMAND,
    );
    for _ in 0..30 {
        gui.harness.step();
    }
    assert!(gui.harness.state_mut().timeline_options_mut().ppf > 4.0);
    gui.quit();
}

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

#[test]
fn dragging_the_trim_editors_roll_handle_two_frames_rolls_the_edit_two_frames() {
    let mut gui = Gui::new();
    gui.double_click(gui.at("V1", 60, 20.0));
    gui.settle();
    let roll = gui.harness.get_by_label("Roll the edit").rect();
    gui.drag(roll.center(), roll.center() + vec2(2.0 * 4.0, 0.0));
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 62),
            ("c2".into(), 62, 58),
            ("c3".into(), 120, 60)
        ]
    );
    gui.quit();
}

#[test]
fn dragging_the_trim_editors_out_handle_three_frames_left_trims_the_outgoing_clip() {
    let mut gui = Gui::new();
    gui.double_click(gui.at("V1", 60, 20.0));
    gui.settle();
    let out = gui.harness.get_by_label("Trim the Out").rect();
    gui.drag(out.center(), out.center() - vec2(3.0 * 4.0, 0.0));
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 57),
            ("c2".into(), 57, 60),
            ("c3".into(), 117, 60)
        ]
    );
    gui.quit();
}

#[test]
fn one_click_on_a_cut_opens_the_trim_editor() {
    let mut gui = Gui::new();
    gui.click(gui.at("V1", 60, 20.0));
    gui.settle();
    assert!(gui.harness.query_by_label("Roll the edit").is_some());
    gui.quit();
}

#[test]
fn the_trim_editors_pair_beside_the_transport_rolls_a_cut_with_both_sides_selected() {
    let mut gui = Gui::new();
    gui.click(gui.at("V1", 60, 20.0));
    gui.settle();
    gui.harness
        .get_all_by_label("Roll the edit a frame later")
        .next()
        .unwrap()
        .click();
    gui.harness.step();
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 61),
            ("c2".into(), 61, 59),
            ("c3".into(), 120, 60)
        ]
    );
    gui.quit();
}

#[test]
fn esc_in_the_trim_editor_deselects_the_cut() {
    let mut gui = Gui::new();
    gui.click(gui.at("V1", 60, 20.0));
    gui.settle();
    gui.key(Key::Escape);
    gui.settle();
    assert_eq!(gui.harness.state().selected_edit(), None);
    gui.quit();
}

fn dissolve_at_60(gui: &mut Gui) {
    gui.call(
        "edit.dissolve",
        json!({ "at": 60, "length": "10f", "sound": false }),
    );
    gui.settle();
}

fn v1_transitions(gui: &Gui) -> Vec<(i64, i64)> {
    let p = gui.call("project.get", json!({}));
    let v1 = p["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "V1")
        .unwrap()
        .clone();
    v1["transitions"].as_array().map_or_else(Vec::new, |ts| {
        ts.iter()
            .map(|t| (t["before"].as_i64().unwrap(), t["after"].as_i64().unwrap()))
            .collect()
    })
}

#[test]
fn a_click_on_a_dissolve_shows_it_between_the_trim_editors_strips() {
    let mut gui = Gui::new();
    dissolve_at_60(&mut gui);
    gui.click(gui.at("V1", 57, 20.0));
    gui.settle();
    assert!(gui.harness.query_by_label("The dissolve's end").is_some());
    gui.quit();
}

#[test]
fn dragging_the_dissolves_end_two_frames_out_lengthens_it_two_frames_each_side() {
    let mut gui = Gui::new();
    dissolve_at_60(&mut gui);
    gui.click(gui.at("V1", 57, 20.0));
    gui.settle();
    let start = gui
        .harness
        .get_by_label("The dissolve's start")
        .rect()
        .center();
    let end = gui
        .harness
        .get_by_label("The dissolve's end")
        .rect()
        .center();
    let frame = (end.x - start.x) / 10.0;
    gui.drag(end, end + vec2(2.0 * frame, 0.0));
    gui.settle();
    assert_eq!(v1_transitions(&gui), [(7, 7)]);
    gui.quit();
}

#[test]
fn center_on_edit_from_a_dissolves_menu_recenters_it() {
    let mut gui = Gui::new();
    gui.call(
        "edit.dissolve",
        json!({ "at": 60, "length": "10f", "align": "end", "sound": false }),
    );
    gui.settle();
    gui.click_with(gui.at("V1", 55, 20.0), egui::PointerButton::Secondary);
    gui.settle();
    gui.harness.get_by_label("Center on Edit").click();
    gui.harness.step();
    gui.settle();
    assert_eq!(v1_transitions(&gui), [(5, 5)]);
    gui.quit();
}

#[test]
fn delete_with_a_dissolve_selected_takes_the_dissolve_away() {
    let mut gui = Gui::new();
    dissolve_at_60(&mut gui);
    gui.click(gui.at("V1", 57, 20.0));
    gui.settle();
    gui.key(Key::Delete);
    gui.settle();
    assert_eq!((v1_transitions(&gui), gui.clips("V1").len()), (vec![], 3));
    gui.quit();
}

#[test]
fn change_transition_duration_from_a_dissolves_menu_takes_frames() {
    let mut gui = Gui::new();
    dissolve_at_60(&mut gui);
    gui.click_with(gui.at("V1", 57, 20.0), egui::PointerButton::Secondary);
    gui.settle();
    gui.harness
        .get_by_label("Change Transition Duration…")
        .click();
    gui.harness.step();
    gui.settle();
    gui.harness.get_by_label("Frames").click();
    gui.harness.step();
    let label = gui
        .harness
        .get_all_by_label("Duration")
        .map(|n| n.rect())
        .max_by(|a, b| a.top().total_cmp(&b.top()))
        .unwrap();
    gui.click(pos2(label.right() + 40.0, label.center().y));
    gui.harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    gui.harness.event(egui::Event::Text("20".into()));
    gui.harness.step();
    gui.harness.get_by_label("Change").click();
    gui.harness.step();
    gui.settle();
    assert_eq!(v1_transitions(&gui), [(10, 10)]);
    gui.quit();
}

#[test]
fn u_after_a_click_on_a_cut_selects_the_incoming_side() {
    let mut gui = Gui::new();
    gui.click(gui.at("V1", 60, 20.0));
    gui.key(Key::U);
    let side = gui.harness.state().gui_state().edit.map(|e| e.side);
    assert_eq!(side, Some(TrimSide::In));
    gui.quit();
}

#[test]
fn dragging_a_dissolves_end_on_the_track_lengthens_it_evenly() {
    let mut gui = Gui::new();
    dissolve_at_60(&mut gui);
    gui.drag(gui.at("V1", 65, 20.0), gui.at("V1", 67, 20.0));
    gui.settle();
    assert_eq!(v1_transitions(&gui), [(7, 7)]);
    gui.quit();
}

#[test]
fn a_click_on_the_cut_under_a_dissolve_shows_the_dissolve_in_the_trim_editor() {
    let mut gui = Gui::new();
    dissolve_at_60(&mut gui);
    gui.click(gui.at("V1", 60, 20.0));
    gui.settle();
    assert!(gui.harness.query_by_label("The dissolve's end").is_some());
    gui.quit();
}

#[test]
fn dragging_an_uneven_dissolves_end_two_frames_out_lengthens_only_the_side_with_media() {
    let mut gui = Gui::new();
    gui.call("edit.slip", json!({ "clip": "c2", "by": "-90f" }));
    gui.call("edit.dissolve", json!({ "at": 60, "sound": false }));
    gui.settle();
    gui.click(gui.at("V1", 65, 20.0));
    gui.settle();
    let cut = gui.harness.get_by_label("Roll the edit").rect().center();
    let end = gui
        .harness
        .get_by_label("The dissolve's end")
        .rect()
        .center();
    let frame = (end.x - cut.x) / 15.0;
    gui.drag(end, end + vec2(2.0 * frame, 0.0));
    gui.settle();
    assert_eq!(v1_transitions(&gui), [(10, 17)]);
    gui.quit();
}

#[test]
fn the_trim_editors_out_handle_trims_at_the_lower_timelines_scale_with_a_dissolve_shown() {
    let mut gui = Gui::new();
    dissolve_at_60(&mut gui);
    gui.click(gui.at("V1", 57, 20.0));
    gui.settle();
    let out = gui.harness.get_by_label("Trim the Out").rect();
    gui.drag(out.center(), out.center() - vec2(3.0 * 4.0, 0.0));
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 57),
            ("c2".into(), 57, 60),
            ("c3".into(), 117, 60)
        ]
    );
    gui.quit();
}

#[test]
fn the_trim_editors_roll_handle_rolls_under_a_dissolve_starting_on_the_edit() {
    let mut gui = Gui::new();
    gui.call(
        "edit.dissolve",
        json!({ "at": 60, "length": "10f", "align": "start", "sound": false }),
    );
    gui.settle();
    gui.click(gui.at("V1", 65, 20.0));
    gui.settle();
    let roll = gui.harness.get_by_label("Roll the edit").rect();
    gui.drag(roll.center(), roll.center() + vec2(2.0 * 4.0, 0.0));
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 62),
            ("c2".into(), 62, 58),
            ("c3".into(), 120, 60)
        ]
    );
    gui.quit();
}

#[test]
fn a_drag_on_the_cut_rolls_it_under_a_dissolve_one_frame_before_it() {
    let mut gui = Gui::new();
    gui.call(
        "edit.dissolve",
        json!({ "at": 60, "before": "1f", "after": "9f", "sound": false }),
    );
    gui.settle();
    gui.drag(gui.at("V1", 60, 20.0), gui.at("V1", 62, 20.0));
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 62),
            ("c2".into(), 62, 58),
            ("c3".into(), 120, 60)
        ]
    );
    gui.quit();
}

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

#[test]
fn import_folder_brings_a_folder_in_as_a_bin_and_names_what_it_skipped() {
    let mut gui = Gui::new();
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-folder-")
        .tempdir_in(vidcut_av::testdata::temp_dir())
        .unwrap();
    let shoot = tmp.path().join("Shoot");
    std::fs::create_dir_all(shoot.join("Day 1")).unwrap();
    let clip = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        30,
    ))
    .unwrap();
    std::fs::copy(&clip, shoot.join("Day 1").join("take.mp4")).unwrap();
    std::fs::write(shoot.join("notes.txt"), "shot list").unwrap();
    *gui.dialogs.folder.lock().unwrap() = Some(shoot);

    gui.harness.get_by_label("Import Folder").click();
    let started = Instant::now();
    while gui.call("media.list", json!({})).as_array().unwrap().len() < 2 {
        assert!(started.elapsed() < Duration::from_secs(30), "no import");
        std::thread::sleep(Duration::from_millis(5));
        gui.harness.step();
    }
    assert_eq!(
        gui.call("bins.list", json!({})),
        json!(["Shoot", "Shoot/Day 1"])
    );
    let notices = gui.h.snapshot().notices.clone();
    assert_eq!(
        notices.back().map(|n| n.text.as_str()),
        Some(
            "Import Media Folder: 1 clip in bin Shoot; skipped a file that isn't media: notes.txt"
        )
    );
    gui.quit();
}

#[test]
fn a_dynamic_trim_run_applies_where_the_media_ends() {
    let mut gui = Gui::new();
    gui.call("selection.set", json!({ "clips": ["c3"] }));
    gui.settle();
    gui.key(Key::W);
    for _ in 0..4 {
        gui.key(Key::J);
    }
    let started = Instant::now();
    while gui.source_in("c3") != json!(240) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no trim landed"
        );
        gui.harness.step();
    }
    gui.quit();
}

#[test]
fn a_dynamic_trim_run_that_reaches_the_media_end_says_why() {
    let mut gui = Gui::new();
    gui.call("selection.set", json!({ "clips": ["c3"] }));
    gui.settle();
    gui.key(Key::W);
    for _ in 0..4 {
        gui.key(Key::J);
    }
    let started = Instant::now();
    while gui
        .harness
        .query_by_label_contains("Stopped here")
        .is_none()
    {
        assert!(started.elapsed() < Duration::from_secs(10), "no word why");
        gui.harness.step();
    }
    gui.quit();
}

#[test]
fn the_lower_timeline_follows_a_dynamic_trim_run() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 55, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.key(Key::L);
    for _ in 0..30 {
        gui.harness.step();
    }
    let l = gui.harness.state().timeline_layout().unwrap();
    let centre = l.lower_scale.frame(l.lower_ruler.center().x);
    assert!(centre > 61.0, "the lower timeline stayed at frame {centre}");
    gui.quit();
}

#[test]
fn a_cut_mid_run_with_media_on_both_sides_has_media_on_both_sides() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 55, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.key(Key::L);
    for _ in 0..10 {
        gui.harness.step();
    }
    assert_eq!(
        gui.harness.state_mut().drawn_cut_media(),
        Some((true, true))
    );
    gui.quit();
}

#[test]
fn stopping_a_run_keeps_the_lower_timeline_on_the_cut() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 55, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.settle();
    gui.key(Key::L);
    for _ in 0..30 {
        gui.harness.step();
    }
    gui.key(Key::K);
    let l = gui.harness.state().timeline_layout().unwrap();
    let centre = l.lower_scale.frame(l.lower_ruler.center().x);
    assert!(
        centre > 61.0,
        "the lower timeline went back to frame {centre}"
    );
    gui.quit();
}

#[test]
fn a_roll_forward_plays_the_incoming_clip_from_the_cut() {
    let (q, at, speed) = heard_timeline(run_heard(
        cut(60, Side::Both),
        DynamicMode::Slip,
        false,
        1.0,
    ));
    let c2 = q.clip("c2").unwrap();
    assert_eq!((at, speed, c2.source_at(at)), (60, 1.0, 100));
}

#[test]
fn a_roll_backward_plays_the_incoming_clip_back_from_the_cut() {
    let (q, at, speed) = heard_timeline(run_heard(
        cut(60, Side::Both),
        DynamicMode::Slip,
        false,
        -1.0,
    ));
    let c2 = q.clip("c2").unwrap();
    assert_eq!((at, speed, c2.start, c2.source_at(at)), (60, -1.0, 1, 100));
}

#[test]
fn an_out_trim_forward_plays_the_outgoing_clip_on_past_the_cut() {
    let (q, at, speed) =
        heard_timeline(run_heard(cut(60, Side::Out), DynamicMode::Slip, false, 1.0));
    let c1 = q.clip("c1").unwrap();
    assert_eq!((at, speed, c1.duration), (59, 1.0, 300));
}

#[test]
fn an_in_trim_forward_plays_the_incoming_clip_from_its_first_frame() {
    let (q, at, _) = heard_timeline(run_heard(cut(60, Side::In), DynamicMode::Slip, false, 1.0));
    let c2 = q.clip("c2").unwrap();
    assert_eq!((at, c2.source_at(at)), (60, 100));
}

#[test]
fn a_slip_forward_plays_the_clips_media_backward_from_its_first_frame() {
    let (q, at, speed) = heard_timeline(run_heard(
        DynamicTarget::Clips(vec!["c2".into()]),
        DynamicMode::Slip,
        false,
        1.0,
    ));
    let c2 = q.clip("c2").unwrap();
    assert_eq!(
        (c2.source_at(at), c2.source_at(at - 1), speed),
        (100, 99, -1.0)
    );
}

#[test]
fn a_slip_plays_the_clip_as_the_timeline_plays_it() {
    let gui = Gui::new();
    gui.call("track.set", json!({ "track": "V1", "muted": true }));
    gui.call(
        "edit.change_speed",
        json!({ "clips": ["c2"], "speed": -1.0, "ripple": false }),
    );
    let p = gui.h.snapshot().project.clone().unwrap().project;
    let target = DynamicTarget::Clips(vec!["c2".into()]);
    let (q, at, _) = heard_timeline(heard(&p, &target, DynamicMode::Slip, false, 0, 1.0).unwrap());
    gui.quit();
    let clips: usize = q.tracks.iter().map(|t| t.clips.len()).sum();
    let c2 = q.clip("c2").unwrap();
    assert_eq!(
        (
            q.track("V1").unwrap().muted,
            clips,
            c2.source_at(at),
            c2.source_at(at - 1)
        ),
        (true, 1, 159, 160)
    );
}

#[test]
fn w_shows_the_timeline_viewer() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 60, "viewer": "timeline" }));
    gui.call("viewer.set", json!({ "mode": "source_tape" }));
    gui.settle();
    gui.key(Key::W);
    gui.settle();
    assert_eq!(gui.h.snapshot().viewer, ViewerMode::Timeline);
    gui.quit();
}

#[test]
fn a_dynamic_trim_run_shows_the_timeline_viewer() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 60, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.call("viewer.set", json!({ "mode": "source_tape" }));
    gui.settle();
    gui.key(Key::L);
    gui.settle();
    assert_eq!(gui.h.snapshot().viewer, ViewerMode::Timeline);
    gui.quit();
}

#[test]
fn a_slide_at_the_clips_end_plays_the_next_clip_from_its_first_frame() {
    let (q, at, speed) = heard_timeline(run_heard(
        DynamicTarget::Clips(vec!["c2".into()]),
        DynamicMode::Slide,
        true,
        1.0,
    ));
    let c3 = q.clip("c3").unwrap();
    assert_eq!((at, speed, c3.source_at(at)), (120, 1.0, 200));
}

#[test]
fn a_seek_right_after_a_run_stops_moves_the_lower_timeline() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 55, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.settle();
    gui.key(Key::L);
    for _ in 0..30 {
        gui.harness.step();
    }
    gui.key(Key::K);
    gui.key(Key::Home);
    gui.settle();
    let l = gui.harness.state().timeline_layout().unwrap();
    let centre = l.lower_scale.frame(l.lower_ruler.center().x);
    assert!(centre < 1.0, "the lower timeline stayed at frame {centre}");
    gui.quit();
}

#[test]
fn w_takes_the_playhead_to_the_nearest_cut() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 50, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.settle();
    assert_eq!(gui.position(), 60);
    gui.quit();
}

#[test]
fn w_takes_the_playhead_to_the_nearer_end_of_the_selected_clip() {
    let mut gui = Gui::new();
    gui.call("selection.set", json!({ "clips": ["c2"] }));
    gui.call("player.seek", json!({ "at": 100, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.settle();
    assert_eq!(gui.position(), 120);
    gui.quit();
}

#[test]
fn w_again_deselects_the_cut() {
    let mut gui = Gui::new();
    gui.key(Key::W);
    gui.key(Key::W);
    gui.settle();
    assert_eq!(gui.call("selection.get", json!({}))["edit"], Value::Null);
    gui.quit();
}

#[test]
fn period_in_dynamic_trim_rolls_the_cut_a_frame() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 55, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.key(Key::Period);
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 61),
            ("c2".into(), 61, 59),
            ("c3".into(), 120, 60)
        ]
    );
    gui.quit();
}

#[test]
fn k_and_l_in_dynamic_trim_slip_a_clip_to_an_earlier_frame() {
    let mut gui = Gui::new();
    gui.call("selection.set", json!({ "clips": ["c2"] }));
    gui.settle();
    gui.key(Key::W);
    gui.hold(Key::K, Key::L);
    gui.settle();
    assert_eq!(gui.source_in("c2"), json!(99));
    gui.quit();
}

#[test]
fn k_and_l_held_in_dynamic_trim_roll_the_cut_slowly_in_one_undo_step() {
    let mut gui = Gui::new();
    gui.call("player.seek", json!({ "at": 55, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::W);
    gui.press(Key::K, true);
    gui.press(Key::L, true);
    for _ in 0..60 {
        gui.harness.step();
    }
    gui.press(Key::L, false);
    gui.press(Key::K, false);
    gui.settle();
    let rolled = gui.clips("V1")[0].2 - 60;
    assert!(
        (9..=13).contains(&rolled),
        "a second held rolled the cut {rolled} frames, not about 11"
    );
    gui.call("history.undo", json!({}));
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 60),
            ("c2".into(), 60, 60),
            ("c3".into(), 120, 60)
        ]
    );
    gui.quit();
}

#[test]
fn a_clips_menu_stays_open() {
    let mut gui = Gui::new();
    gui.click_with(gui.at("V1", 90, 6.0), egui::PointerButton::Secondary);
    for _ in 0..5 {
        gui.harness.step();
    }
    assert!(egui::Popup::is_any_open(&gui.harness.ctx));
    gui.quit();
}

#[test]
fn period_rolls_the_edit_a_request_selected() {
    let mut gui = Gui::new();
    gui.ask("selection.set", json!({ "edit": 55 })).unwrap();
    gui.key(Key::Period);
    gui.settle();
    assert_eq!(
        gui.clips("V1"),
        [
            ("c1".into(), 0, 61),
            ("c2".into(), 61, 59),
            ("c3".into(), 120, 60)
        ]
    );
    gui.quit();
}

#[test]
fn period_moves_the_audio_edit_a_request_selected() {
    let mut gui = Gui::new();
    gui.call(
        "edit.audio_roll",
        json!({ "clip": "c1", "edge": "tail", "by": 8 }),
    );
    gui.settle();
    gui.ask("selection.set", json!({ "edit": 60, "side": "audio" }))
        .unwrap();
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

#[test]
fn a_side_alone_moves_the_selection_to_the_audio_edit() {
    let mut gui = Gui::new();
    gui.call(
        "edit.audio_roll",
        json!({ "clip": "c1", "edge": "tail", "by": 8 }),
    );
    gui.settle();
    gui.ask("selection.set", json!({ "edit": 60 })).unwrap();
    let selected = gui
        .ask("selection.set", json!({ "side": "audio" }))
        .unwrap();
    assert_eq!(
        selected["edit"],
        json!({ "at": 68, "track": "V1", "side": "audio" })
    );
    gui.quit();
}

#[test]
fn f10_overwrites_on_the_target_a_request_picked() {
    let mut gui = Gui::new();
    gui.call("track.add", json!({ "kind": "video" }));
    gui.ask("gui.target", json!({ "track": "V2" })).unwrap();
    gui.call(
        "media.marks.set",
        json!({ "media": "m1", "in": 10, "out": 40 }),
    );
    gui.call("selection.set", json!({ "media": "m1" }));
    gui.call("player.seek", json!({ "at": 90, "viewer": "timeline" }));
    gui.settle();
    gui.key(Key::F10);
    gui.settle();
    assert_eq!(gui.clips("V2"), [("c4".into(), 90, 30)]);
    gui.quit();
}

#[test]
fn gui_timeline_zooms_the_lower_timeline() {
    let mut gui = Gui::new();
    gui.ask("gui.timeline", json!({ "zoom": 8.0 })).unwrap();
    gui.settle();
    let l = gui.harness.state().timeline_layout().unwrap();
    assert_eq!(l.lower_scale.xf(31) - l.lower_scale.xf(30), 8.0);
    gui.quit();
}

#[test]
fn gui_timeline_refuses_a_zoom_out_of_range() {
    let mut gui = Gui::new();
    assert!(gui.ask("gui.timeline", json!({ "zoom": 50.0 })).is_err());
    assert_eq!(gui.harness.state_mut().timeline_options_mut().ppf, 4.0);
    gui.quit();
}

#[test]
fn gui_close_closes_the_mixer() {
    let mut gui = Gui::new();
    gui.ask("gui.show", json!({ "panel": "mixer" })).unwrap();
    let state = gui.ask("gui.close", json!({ "panel": "mixer" })).unwrap();
    assert_eq!(state["windows"], json!([]));
    gui.quit();
}

#[test]
fn the_pools_search_field_follows_pool_set() {
    let mut gui = Gui::new();
    gui.call("pool.set", json!({ "search": "interview" }));
    gui.settle();
    let field = gui
        .harness
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .min_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
        .expect("the pool's search field");
    assert_eq!(field.value().as_deref(), Some("interview"));
    gui.quit();
}
