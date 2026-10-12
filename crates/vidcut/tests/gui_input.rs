//! The GUI under simulated input (egui_kittest, no window or renderer): the
//! real panels against a live session without players. Each gesture must
//! change the project with exactly one request, and the command log must
//! match the undo history.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use vidcut::gui::workspace::Workspace;
use vidcut::live::{self, Capture, Origin, PlayerId, Presented, WindowChange};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_core::model::Settings;
use vidcut_core::time::Rational;
use vidcut_core::{Envelope, Project, Response, Store};

struct NoRenderer;

impl live::Renderer for NoRenderer {
    fn present(&self, _: PlayerId, done: Presented) {
        done(None);
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

/// Lets the session catch up with the GUI's requests, then redraws.
fn settle(h: &live::Handle, harness: &mut Harness<'_, Workspace>) {
    // The session answers in order: once this returns, earlier requests ran.
    call(h, "player.state", json!({}));
    harness.step();
    harness.step();
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

/// The timeline playhead, in frames.
fn playhead(h: &live::Handle) -> i64 {
    call(h, "player.state", json!({})).result["timeline"]["position"]
        .as_i64()
        .unwrap()
}

fn undo_steps(h: &live::Handle) -> usize {
    call(h, "history.get", json!({})).result["undo"]
        .as_array()
        .unwrap()
        .len()
}

/// Presses at `from`, moves in small steps to `to` and releases.
fn drag(harness: &mut Harness<'_, Workspace>, from: Pos2, to: Pos2, mods: Modifiers) {
    harness.event(egui::Event::PointerMoved(from));
    harness.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: mods,
    });
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        harness.event(egui::Event::PointerMoved(from + (to - from) * t));
    }
    harness.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: mods,
    });
    harness.step();
}

/// Two clicks at `pos`, each press and release a frame apart, as a mouse
/// makes them: a double-click.
fn double_click(harness: &mut Harness<'_, Workspace>, pos: Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false, true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
}

/// Whether the Trim Editor is open.
fn trim_editor(harness: &Harness<'_, Workspace>) -> bool {
    harness
        .query_all_by_label_contains("edit at ")
        .next()
        .is_some()
}

/// The lower timeline position of `frame`, `dy` pixels into the track.
fn at(harness: &Harness<'_, Workspace>, frame: i64, dy: f32) -> Pos2 {
    let l = harness.state().timeline_layout().expect("drawn");
    let v1 = l.lower.get("V1").expect("Track 1's row").rect.rect;
    pos2(l.lower_scale.xf(frame), v1.top() + dy)
}

fn v1(h: &live::Handle) -> Vec<(String, i64, i64)> {
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
            )
        })
        .collect()
}

fn setup(dir: &Path) -> (live::Handle, std::thread::JoinHandle<()>) {
    let media = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ))
    .unwrap();
    let path = dir.join("input.vidcut");
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
    let (h, t) = live::start(live::Config {
        project: Some(path),
        create: false,
        players: None,
        preview_error: Some("no preview in tests".into()),
        renderer: Arc::new(NoRenderer),
        notify: Arc::new(|| {}),
        socket: None,
    });
    call(&h, "media.add", json!({ "paths": [media] }));
    // c1 0..60, c2 60..120, c3 120..180.
    for (a, b) in [(0, 60), (100, 160), (200, 260)] {
        call(
            &h,
            "edit.append",
            json!({ "media": "m1", "range": format!("{a}f..{b}f") }),
        );
    }
    call(&h, "player.seek", json!({ "at": 90, "viewer": "timeline" }));
    (h, t)
}

#[test]
fn gestures_send_one_request_each() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-input-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    // SAFETY: the only test in this binary; set before any thread starts.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", dir.join("state"));
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
    }
    let (h, session) = setup(dir);
    let mut ws = Workspace::new(h.clone());
    // Predictable geometry: 4 px per frame, no snapping, fixed playhead.
    ws.timeline_options_mut().snapping = false;
    ws.timeline_options_mut().ppf = 4.0;
    let mut harness = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(50)
        .build_ui_state(|ui, ws: &mut Workspace| ws.ui(ui), ws);
    settle(&h, &mut harness);
    let undo0 = undo_steps(&h);
    let edits0 = gui_edits(&h).len();

    // 1. A pool clip selected, the Append button: one edit.append.
    call(&h, "selection.set", json!({ "media": "m1" }));
    settle(&h, &mut harness);
    harness.get_by_label("Append").click();
    harness.step();
    settle(&h, &mut harness);
    let edits = gui_edits(&h);
    assert_eq!(edits.len(), edits0 + 1, "{edits:#?}");
    assert_eq!(edits.last().unwrap().0, "edit.append");
    assert_eq!(v1(&h).len(), 4);
    let end = v1(&h).iter().map(|c| c.1 + c.2).max().unwrap();
    assert_eq!(
        playhead(&h),
        end,
        "an edit function moves the playhead (D15)"
    );

    // Bring the first clips back into the (fixed-playhead) lower timeline.
    call(&h, "player.seek", json!({ "at": 90, "viewer": "timeline" }));
    settle(&h, &mut harness);

    // 2. Dragging c1's tail 5 frames right in the lower timeline: a rippled
    // trim.
    let from = at(&harness, 60, 40.0) - vec2(4.0, 0.0);
    drag(&mut harness, from, from + vec2(20.0, 0.0), Modifiers::NONE);
    settle(&h, &mut harness);
    let (m, p) = gui_edits(&h).last().unwrap().clone();
    assert_eq!(
        (m.as_str(), &p["clip"], &p["by"]),
        ("edit.trim", &json!("c1"), &json!(5))
    );
    assert_eq!(v1(&h)[0], ("c1".into(), 0, 65));

    // 3. Dragging c3 (now 125..185) onto the c1|c2 edit (65): insert there
    //.
    let grab = at(&harness, 150, 40.0);
    let edit = at(&harness, 65, 40.0);
    drag(&mut harness, grab, edit, Modifiers::NONE);
    settle(&h, &mut harness);
    let (m, p) = gui_edits(&h).last().unwrap().clone();
    assert_eq!(
        (m.as_str(), &p["mode"], &p["to"]),
        ("edit.move", &json!("insert"), &json!(65))
    );
    assert_eq!(
        v1(&h).iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        vec!["c1", "c3", "c2", "c4"]
    );

    // 4. Dragging the pool clip (list view) past the end of the timeline:
    // append (spec §7.3).
    harness.get_by_label("List").click();
    harness.step();
    // The pool list shows the file name (elided to 40 characters).
    let name = call(&h, "media.get", json!({ "media": "m1" })).result["name"]
        .as_str()
        .unwrap()
        .to_string();
    let label = if name.chars().count() > 40 {
        format!("{}…", name.chars().take(39).collect::<String>())
    } else {
        name
    };
    let row = harness.get_by_label(&label).rect().center();
    // Dropped on the upper timeline (the whole program), past the last clip.
    let end = v1(&h).iter().map(|c| c.1 + c.2).max().unwrap();
    let l = harness.state().timeline_layout().unwrap();
    let upper_v1 = l.upper.get("V1").unwrap().rect.rect;
    let past = pos2(l.upper_scale.xf(end) + 20.0, upper_v1.center().y);
    let before = playhead(&h);
    drag(&mut harness, row, past, Modifiers::NONE);
    settle(&h, &mut harness);
    assert_eq!(gui_edits(&h).last().unwrap().0, "edit.append");
    assert_eq!(playhead(&h), before, "a drop leaves the playhead (D15)");

    // 5. V selects the edit nearest the playhead, `.` rolls it a frame.
    call(&h, "player.seek", json!({ "at": 66, "viewer": "timeline" }));
    settle(&h, &mut harness);
    harness.key_press(egui::Key::V);
    harness.key_press(egui::Key::Period);
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = gui_edits(&h).last().unwrap().clone();
    assert_eq!((m.as_str(), &p["by"]), ("edit.roll", &json!(1)));

    // 6. Ctrl+Z undoes it.
    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    harness.step();
    settle(&h, &mut harness);
    assert_eq!(gui_edits(&h).last().unwrap().0, "history.undo");

    // Every gesture made exactly one change, and the history agrees: six
    // edits, five undo steps left (the last one undid the roll).
    let edits = gui_edits(&h);
    assert_eq!(edits.len() - edits0, 6, "{edits:#?}");
    assert_eq!(undo_steps(&h), undo0 + 4);

    // The Trim Editor: a double-click on a cut opens it. A trim moves the
    // selection before it lands, and the editor stays open through it.
    let first = v1(&h)[0].clone();
    let cut = first.1 + first.2;
    call(
        &h,
        "player.seek",
        json!({ "at": cut, "viewer": "timeline" }),
    );
    settle(&h, &mut harness);
    let pos = at(&harness, cut, 20.0);
    double_click(&mut harness, pos);
    harness.step();
    assert!(trim_editor(&harness), "a double-click on a cut opens it");
    let edits0 = gui_edits(&h).len();
    // The Roll +1 (the buttons go Out, Roll, In), twice without waiting:
    // the second builds on the first even before it lands.
    for _ in 0..2 {
        harness.query_all_by_label("+1").nth(1).unwrap().click();
        harness.step();
        assert!(trim_editor(&harness), "open while the trim lands");
    }
    settle(&h, &mut harness);
    assert!(trim_editor(&harness), "open once it landed");
    let edits = gui_edits(&h);
    assert_eq!(edits.len(), edits0 + 2, "{edits:#?}");
    assert!(edits[edits0..].iter().all(|(m, _)| m == "edit.roll"));
    assert_eq!(v1(&h)[0].2, first.2 + 2, "the cut rolled twice");
    assert_eq!(harness.state().selected_edit(), Some(cut + 2));
    harness.key_press(egui::Key::Escape);
    harness.step();
    assert!(!trim_editor(&harness), "Esc closes it");
    // A trim that can't be made: the clip after this cut starts at its
    // media's first frame, so its In can't move earlier. The selection
    // stays on the cut.
    call(
        &h,
        "edit.append",
        json!({ "media": "m1", "range": "0f..30f" }),
    );
    let cut = v1(&h).last().unwrap().1;
    call(
        &h,
        "player.seek",
        json!({ "at": cut, "viewer": "timeline" }),
    );
    settle(&h, &mut harness);
    let pos = at(&harness, cut, 20.0);
    double_click(&mut harness, pos);
    harness.step();
    assert!(trim_editor(&harness));
    let edits0 = gui_edits(&h).len();
    harness.query_all_by_label("−1").nth(2).unwrap().click();
    harness.step();
    settle(&h, &mut harness);
    assert_eq!(gui_edits(&h).len(), edits0, "nothing changed");
    assert_eq!(harness.state().selected_edit(), Some(cut));
    assert!(trim_editor(&harness));
    harness.key_press(egui::Key::Escape);
    harness.step();

    // Ctrl+C, then Ctrl+V over the middle of a longer clip: the playhead
    // goes to the end of the pasted clip, not to the end of what is left of
    // the clip it cut into (D15).
    let (copied, _, len) = v1(&h).last().unwrap().clone();
    let (_, start, dur) = v1(&h).into_iter().max_by_key(|c| c.2).unwrap();
    assert!(dur > 3 * len, "a clip to paste into");
    call(&h, "selection.set", json!({ "clips": [copied] }));
    settle(&h, &mut harness);
    harness.event(egui::Event::Copy);
    harness.step();
    let into = start + dur / 3;
    call(
        &h,
        "player.seek",
        json!({ "at": into, "viewer": "timeline" }),
    );
    settle(&h, &mut harness);
    harness.event(egui::Event::Paste(String::new()));
    harness.step();
    settle(&h, &mut harness);
    assert_eq!(gui_edits(&h).last().unwrap().0, "edit.paste");
    assert!(
        v1(&h)
            .iter()
            .any(|c| c.1 == into + len && c.1 + c.2 == start + dur)
    );
    assert_eq!(playhead(&h), into + len, "after the pasted clip (D15)");

    // Smooth with 1,000 clips: split the long clip into 1,000 pieces
    // and time whole frames of the GUI (layout, timelines, pool, viewer).
    for _ in 0..4 {
        call(&h, "edit.append", json!({ "media": "m1" }));
    }
    let end = v1(&h).iter().map(|c| c.1 + c.2).max().unwrap();
    let cuts: Vec<i64> = (1..end).collect();
    call(&h, "edit.split", json!({ "at": cuts }));
    let clips = v1(&h).len();
    assert!(clips >= 1000, "{clips} clips");
    // Zoomed out so all of them are on screen at once.
    harness.state_mut().timeline_options_mut().ppf = 0.5;
    settle(&h, &mut harness);
    let started = std::time::Instant::now();
    for _ in 0..30 {
        harness.step();
    }
    let per_frame = started.elapsed() / 30;
    eprintln!("{clips} clips: {per_frame:?} per frame");
    assert!(
        per_frame < Duration::from_millis(40),
        "{per_frame:?} per frame"
    );

    drop(harness);
    h.shutdown();
    let _ = session.join();
    std::thread::sleep(Duration::from_millis(10));
}
