//! The timeline tools under simulated input (egui_kittest): Ripple Delete
//! Silence previews its parts and removes them, Detect Scene Cuts, Dynamic
//! Trim (W, then L/K) and the Boring Detector's settings. Each gesture
//! changes the project with exactly one request.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture, VideoCodec};
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

fn call(h: &live::Handle, method: &str, params: Value) -> Response {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    h.call(e, Origin::Ipc, None, &mut |_| true)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn settle(h: &live::Handle, harness: &mut Harness<'_, Workspace>) {
    call(h, "player.state", json!({}));
    harness.step();
    harness.step();
}

fn until(
    harness: &mut Harness<'_, Workspace>,
    what: &str,
    ready: impl Fn(&Harness<'_, Workspace>) -> bool,
) {
    let started = Instant::now();
    while !ready(harness) {
        if started.elapsed() > Duration::from_secs(30) {
            let labels: Vec<String> = harness
                .query_all_by(|_| true)
                .filter_map(|n| n.value())
                .filter(|l| !l.is_empty())
                .collect();
            panic!("timed out: {what}; showing {labels:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
}

fn has(harness: &Harness<'_, Workspace>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

/// Requests the GUI sent that changed the project, oldest first.
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

fn clips(h: &live::Handle) -> Vec<(String, i64, i64, i64)> {
    let tl = call(h, "timeline.get", json!({})).result;
    tl["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "clip")
        .map(|i| {
            (
                i["id"].as_str().unwrap().to_string(),
                i["start"].as_i64().unwrap(),
                i["duration"].as_i64().unwrap(),
                i["source_in"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// Opens `menu` and clicks `item` in it. A menu ignores input in the frame it
/// opens in, so it gets a second frame first.
fn menu_item(harness: &mut Harness<'_, Workspace>, menu: &str, item: &str) {
    harness.get_by_label(menu).click();
    harness.step();
    harness.step();
    harness.get_by_label(item).click();
    harness.step();
}

#[test]
fn tools_send_one_request_each() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-tools-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
    }
    // m1: ten seconds with a beep each second; m2: five 30-frame shots.
    let beeps = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ))
    .unwrap();
    let shots = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 150)
            .picture(Picture::Shots { frames: 30 })
            .no_audio(),
    )
    .unwrap();
    let path = dir.join("tools.vidcut");
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
        players: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    call(&h, "media.add", json!({ "paths": [beeps, shots] }));
    // c1 = all of m1 (0..300), c2 = m2 (300..450).
    call(&h, "edit.append", json!({ "media": "m1" }));
    call(&h, "edit.append", json!({ "media": "m2" }));
    let mut ws = Workspace::new(h.clone());
    ws.timeline_options_mut().snapping = false;
    let mut harness = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    settle(&h, &mut harness);
    let edits0 = gui_edits(&h).len();
    let undo0 = call(&h, "history.get", json!({})).result["undo"]
        .as_array()
        .unwrap()
        .len();

    // 1. Ripple Delete Silence: the dialog previews the parts (a
    // dry run, not an edit), then Remove sends one request.
    call(&h, "selection.set", json!({ "clips": ["c1"] }));
    settle(&h, &mut harness);
    menu_item(&mut harness, "Timeline Actions", "Ripple Delete Silence…");
    harness.step();
    until(&mut harness, "the parts", |h| has(h, "Removes 10 parts"));
    assert_eq!(gui_edits(&h).len(), edits0, "previews change nothing");
    harness.get_by_label("Remove").click();
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = gui_edits(&h).last().cloned().unwrap();
    assert_eq!(
        (m.as_str(), &p["clips"]),
        ("edit.ripple_delete_silence", &json!(["c1"]))
    );
    assert_eq!(clips(&h).iter().filter(|c| c.0 != "c2").count(), 10);
    assert!(!has(&harness, "Removes 10 parts"), "the dialog closed");

    // 2. Detect Scene Cuts on the shots clip.
    call(&h, "selection.set", json!({ "clips": ["c2"] }));
    settle(&h, &mut harness);
    menu_item(&mut harness, "Timeline Actions", "Detect Scene Cuts");
    until(&mut harness, "the cuts", |_| clips(&h).len() == 15);
    let (m, _) = gui_edits(&h).last().cloned().unwrap();
    assert_eq!(m, "edit.detect_scene_cuts");

    // 3. Dynamic Trim: W on the edit before the third shot, then L
    // runs a roll for half a second and K applies it as one request.
    let shot3 = clips(&h).iter().find(|c| c.3 == 60).unwrap().1;
    call(
        &h,
        "player.seek",
        json!({ "at": shot3, "viewer": "timeline" }),
    );
    call(&h, "selection.set", json!({ "clips": [] }));
    settle(&h, &mut harness);
    harness.key_press(egui::Key::V);
    harness.key_press(egui::Key::W);
    harness.step();
    assert!(has(&harness, "Dynamic Trim +0"), "the two-up shows");
    harness.key_press(egui::Key::L);
    for _ in 0..30 {
        harness.step();
    }
    harness.key_press(egui::Key::K);
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = gui_edits(&h).last().cloned().unwrap();
    assert_eq!(m, "edit.roll");
    let by = p["by"].as_i64().unwrap();
    assert!((14..=16).contains(&by), "half a second at 30 fps: {by}");
    harness.key_press(egui::Key::W);
    harness.step();
    assert!(!has(&harness, "Dynamic Trim +"), "W leaves");
    assert_eq!(
        harness.state().selected_edit(),
        Some(shot3 + by),
        "the edit followed the trim"
    );

    // 4. The Boring Detector's settings are view options, not edits.
    menu_item(&mut harness, "Timeline Options", "Analyze Timeline Edits…");
    harness.step();
    harness.get_by_label("Analyze").click();
    harness.step();
    assert!(harness.state_mut().timeline_options_mut().boring);

    // Three edits, three undo steps.
    let edits = gui_edits(&h);
    assert_eq!(edits.len() - edits0, 3, "{edits:#?}");
    assert_eq!(
        call(&h, "history.get", json!({})).result["undo"]
            .as_array()
            .unwrap()
            .len(),
        undo0 + 3
    );

    // Undoing the Dynamic Trim takes its cut back where it was: no cut is
    // left where the selection was, so the selection goes too.
    call(&h, "history.undo", json!({ "steps": 1 }));
    settle(&h, &mut harness);
    assert_eq!(
        harness.state().selected_edit(),
        None,
        "nothing selected where no cut is"
    );

    drop(harness);
    h.shutdown();
    let _ = session.join();
    std::thread::sleep(Duration::from_millis(10));
}
