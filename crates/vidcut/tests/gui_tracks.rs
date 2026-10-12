//! The multi-track GUI under simulated input (egui_kittest): the target
//! track, drags onto a new track, Place On Top, the mixer and bins. Each
//! gesture changes the project with exactly one request.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Modifiers, Pos2, accesskit::Role, pos2, vec2};
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

/// Lets the session catch up with the GUI's requests, then redraws.
fn settle(h: &live::Handle, harness: &mut Harness<'_, Workspace>) {
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

/// Requests over IPC (this test's own) that changed the project.
fn changes_over_ipc(h: &live::Handle) -> usize {
    let log = call(h, "gui.log", json!({ "limit": 1000 })).result;
    log.as_array()
        .unwrap()
        .iter()
        .filter(|e| e["origin"] == "ipc" && e["changed"] == true)
        .count()
}

fn undo_steps(h: &live::Handle) -> usize {
    call(h, "history.get", json!({})).result["undo"]
        .as_array()
        .unwrap()
        .len()
}

fn drag(harness: &mut Harness<'_, Workspace>, from: Pos2, to: Pos2) {
    harness.event(egui::Event::PointerMoved(from));
    harness.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        harness.event(egui::Event::PointerMoved(from + (to - from) * t));
    }
    harness.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.step();
}

/// Like [`drag`], a frame per step, as a hand would (widgets such as
/// sliders only see a drag that lasts more than one frame).
fn drag_slowly(harness: &mut Harness<'_, Workspace>, from: Pos2, to: Pos2) {
    harness.event(egui::Event::PointerMoved(from));
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.step();
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        harness.event(egui::Event::PointerMoved(from + (to - from) * t));
        harness.step();
    }
    harness.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.step();
}

/// One audio offset field (`head` or `tail`) of clip `id`.
fn audio_field(h: &live::Handle, id: &str, field: &str) -> i64 {
    let tl = call(h, "timeline.get", json!({})).result;
    tl["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["items"].as_array())
        .flatten()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("no clip {id}"))["audio"][field]
        .as_i64()
        .unwrap_or_else(|| panic!("{id} has no audio.{field}"))
}

/// Clips on `track` as (id, start, duration).
fn clips(h: &live::Handle, track: &str) -> Vec<(String, i64, i64)> {
    let tl = call(h, "timeline.get", json!({})).result;
    let Some(t) = tl["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == track)
    else {
        return Vec::new();
    };
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

fn setup(dir: &Path) -> (live::Handle, std::thread::JoinHandle<()>) {
    let video = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        320,
        180,
        (30, 1),
        300,
    ))
    .unwrap();
    let music = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 120).audio_only(),
    )
    .unwrap();
    let path = dir.join("tracks.vidcut");
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
    call(&h, "media.add", json!({ "paths": [video, music] }));
    // c1 0..60, c2 60..120, c3 120..180 on Track 1, and an empty V2.
    for (a, b) in [(0, 60), (100, 160), (200, 260)] {
        call(
            &h,
            "edit.append",
            json!({ "media": "m1", "range": format!("{a}f..{b}f") }),
        );
    }
    call(&h, "track.add", json!({ "kind": "video" }));
    call(&h, "player.seek", json!({ "at": 90, "viewer": "timeline" }));
    (h, t)
}

#[test]
fn track_gestures_send_one_request_each() {
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-gui-tracks-")
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
    let ipc0 = changes_over_ipc(&h);
    // The GUI requests the steps so far sent: each adds its own.
    let mut gestures = 0;
    let last = |h: &live::Handle| gui_edits(h).last().cloned().unwrap();

    // 1. V2's header makes it the target track: F10 overwrites
    // there, not on Track 1. Clicking it again keeps it the target.
    for _ in 0..2 {
        harness.get_by_label("V2").click();
        harness.step();
    }
    call(
        &h,
        "media.marks.set",
        json!({ "media": "m1", "in": 10, "out": 40 }),
    );
    call(&h, "selection.set", json!({ "media": "m1" }));
    settle(&h, &mut harness);
    harness.key_press(egui::Key::F10);
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!((m.as_str(), &p["track"]), ("edit.overwrite", &json!("V2")));
    assert_eq!(clips(&h, "V2"), vec![("c4".into(), 90, 30)]);
    assert_eq!(clips(&h, "V1").len(), 3, "Track 1 untouched");

    gestures += 1;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 1");

    // 2. Dragging c2 off Track 1 into the strip above the top track: a new
    // track V3; Track 1 closes up.
    let l = harness.state().timeline_layout().unwrap();
    let v1 = l.lower.get("V1").unwrap().rect.rect;
    let grab = pos2(l.lower_scale.xf(70), v1.top() + 20.0);
    let strip = pos2(grab.x, l.lower.span().0 - 5.0);
    drag(&mut harness, grab, strip);
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!(
        (m.as_str(), &p["clips"], &p["track"]),
        ("edit.move", &json!(["c2"]), &json!("V3"))
    );
    assert_eq!(clips(&h, "V3"), vec![("c2".into(), 60, 60)]);
    assert_eq!(
        clips(&h, "V1"),
        vec![("c1".into(), 0, 60), ("c3".into(), 60, 60)]
    );

    gestures += 1;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 2");

    // 3. Place On Top at 100: above the highest track with clips there
    // (V3), so on a new V4.
    call(
        &h,
        "media.marks.set",
        json!({ "media": "m1", "in": 0, "out": 30 }),
    );
    call(
        &h,
        "player.seek",
        json!({ "at": 100, "viewer": "timeline" }),
    );
    settle(&h, &mut harness);
    harness.get_by_label("Place On Top").click();
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!((m.as_str(), &p["at"]), ("edit.place_on_top", &json!(100)));
    assert_eq!(clips(&h, "V4").len(), 1);
    assert_eq!(clips(&h, "V4")[0].1, 100);

    gestures += 1;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 3");

    // 3b. A track header's V turns its video off, and on again; it is lit
    // while the video is on, as A is while the audio is.
    let v1_toggle = |harness: &Harness<'_, Workspace>| {
        harness
            .query_all_by_label("V")
            .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .expect("V1's V")
            .rect()
    };
    for (n, enabled) in [false, true].into_iter().enumerate() {
        let at = v1_toggle(&harness);
        harness
            .get_all_by_label("V")
            .find(|v| v.rect() == at)
            .unwrap()
            .click();
        harness.step();
        settle(&h, &mut harness);
        let (m, p) = last(&h);
        assert_eq!(
            (m.as_str(), &p["track"], &p["video_enabled"]),
            ("track.set", &json!("V1"), &json!(enabled)),
            "click {n}"
        );
    }
    gestures += 2;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 3b");

    // 4. The mixer: dragging Track 1's fader is one track.set, sent on
    // release (one undo step); S solos the track.
    harness.get_by_label("Mixer").click();
    harness.step();
    harness.step();
    // Stereo project, no player: one bar per channel on each strip and the
    // mix. Four video tracks carry sound here.
    assert_eq!(
        harness.query_all_by_label("mix meter").count(),
        2,
        "mix meter is one bar per channel"
    );
    assert_eq!(
        harness.query_all_by_label("track meter").count(),
        8,
        "four strips, stereo"
    );
    // The mix strip keeps its own width at the end of a full row: its name
    // is one line, not a letter a line.
    let mix = harness.get_by_label("Mix").rect();
    assert!(mix.height() < 20.0 && mix.width() > mix.height(), "{mix:?}");
    let mut faders: Vec<egui::Rect> = harness
        .query_all_by_role(Role::Slider)
        .map(|n| n.rect())
        .collect();
    faders.sort_by(|a, b| a.left().total_cmp(&b.left()));
    let fader = *faders.first().expect("a fader per strip");
    let before = gui_edits(&h).len();
    drag_slowly(
        &mut harness,
        fader.center(),
        fader.center() + vec2(0.0, 40.0),
    );
    settle(&h, &mut harness);
    let edits = gui_edits(&h);
    assert_eq!(edits.len(), before + 1, "one request per fader drag");
    let (m, p) = edits.last().unwrap().clone();
    assert_eq!((m.as_str(), &p["track"]), ("track.set", &json!("V1")));
    let db = p["volume_db"].as_f64().unwrap();
    assert!(db < 0.0, "dragged down: {db} dB");
    let tracks = call(&h, "timeline.get", json!({})).result["tracks"].clone();
    assert_eq!(tracks[0]["volume_db"].as_f64(), Some(db));
    let solo = harness
        .query_all_by_label("S")
        .filter(|n| n.rect().left() >= fader.left() - 60.0)
        .min_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .expect("Track 1's solo button")
        .rect();
    harness.event(egui::Event::PointerMoved(solo.center()));
    harness
        .get_all_by_label("S")
        .find(|n| n.rect() == solo)
        .unwrap()
        .click();
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!(
        (m.as_str(), &p["track"], &p["solo"]),
        ("track.set", &json!("V1"), &json!(true))
    );
    // The top bar's toggle closes it (the panel's heading says Mixer too).
    harness
        .get_all_by_label("Mixer")
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap()
        .click();
    harness.step();

    gestures += 2;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 4");

    // 4b. The bottom track sits on the timeline's lower edge: a sound clip
    // dragged just below it goes to a new audio track.
    call(
        &h,
        "edit.append",
        json!({ "media": "m2", "range": "0f..30f" }),
    );
    settle(&h, &mut harness);
    let (id, start, _) = clips(&h, "A1")[0].clone();
    let l = harness.state().timeline_layout().unwrap();
    let a1 = l.lower.get("A1").unwrap().rect.rect;
    assert_eq!(l.lower.span().1, a1.bottom(), "A1 is the bottom row");
    assert_eq!(a1.bottom(), l.lower.area.bottom(), "on the lower edge");
    let grab = pos2(l.lower_scale.xf(start + 6), a1.center().y);
    drag(&mut harness, grab, pos2(grab.x, a1.bottom() + 10.0));
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!(
        (m.as_str(), &p["clips"], &p["track"]),
        ("edit.move", &json!([id]), &json!("A2"))
    );
    assert_eq!(clips(&h, "A2").len(), 1);

    gestures += 1;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 4b");

    // 5. Bins: File > New Bin asks for a name (offering the next free
    // "Bin N", selected so typing replaces it), then a pool clip dropped on
    // the bin moves into it.
    harness.get_by_label("File").click();
    harness.step();
    harness.get_by_label("New Bin").click();
    // The window is measured in its first frame and placed in the next.
    harness.step();
    harness.step();
    assert!(
        harness.query_by_label("Create").is_some(),
        "the naming window"
    );
    harness.event(egui::Event::Text("Clips".into()));
    harness.key_press(egui::Key::Enter);
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!((m.as_str(), &p["path"]), ("bins.add", &json!("Clips")));
    harness.get_by_label("List").click();
    harness.step();
    let name = call(&h, "media.get", json!({ "media": "m2" })).result["name"]
        .as_str()
        .unwrap()
        .to_string();
    let label = if name.chars().count() > 40 {
        format!("{}…", name.chars().take(39).collect::<String>())
    } else {
        name
    };
    let row = harness.get_by_label(&label).rect().center();
    let bin = harness.get_by_label("📁 Clips").rect().center();
    drag(&mut harness, row, bin);
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!(
        (m.as_str(), &p["media"], &p["bin"]),
        ("media.move", &json!(["m2"]), &json!("Clips"))
    );
    let bin_of = |h: &live::Handle, id: &str| {
        call(h, "media.get", json!({ "media": id })).result["bin"].clone()
    };
    assert_eq!(bin_of(&h, "m2"), json!("Clips"));
    assert!(harness.query_by_label(&label).is_none(), "gone from Master");
    // Opening the bin shows it, and Source Tape follows.
    harness.get_by_label("📁 Clips").click_secondary();
    harness.step();
    harness.get_by_label("Open").click();
    harness.step();
    settle(&h, &mut harness);
    assert_eq!(call(&h, "pool.set", json!({})).result["bin"], "Clips");
    assert!(harness.query_by_label(&label).is_some(), "shown in its bin");
    call(&h, "viewer.set", json!({ "mode": "source_tape" }));
    let state = call(&h, "player.state", json!({})).result;
    assert_eq!(state["source"]["content"], "source_tape", "{state:#}");
    assert_eq!(state["source"]["media"], "m2", "{state:#}");

    // Dragging a bin onto another is one bins.rename (spec §3).
    call(&h, "pool.set", json!({ "bin": "" }));
    settle(&h, &mut harness);
    harness.get_by_label("File").click();
    harness.step();
    harness.get_by_label("New Bin").click();
    harness.step();
    harness.step();
    harness.key_press(egui::Key::Enter);
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!((m.as_str(), &p["path"]), ("bins.add", &json!("Bin 1")));
    let from = harness.get_by_label("📁 Bin 1").rect().center();
    let onto = harness.get_by_label("📁 Clips").rect().center();
    let before = gui_edits(&h).len();
    drag(&mut harness, from, onto);
    settle(&h, &mut harness);
    let edits = gui_edits(&h);
    assert_eq!(edits.len(), before + 1, "one request for a bin drag");
    let (m, p) = edits.last().unwrap();
    assert_eq!(
        (m.as_str(), &p["path"], &p["to"]),
        ("bins.rename", &json!("Bin 1"), &json!("Clips/Bin 1"))
    );
    assert_eq!(
        call(&h, "project.summary", json!({})).result["bins"],
        json!(["Clips", "Clips/Bin 1"])
    );
    assert_eq!(bin_of(&h, "m2"), json!("Clips"));
    assert!(
        harness.query_by_label("📁 Bin 1").is_none(),
        "moved out of Master"
    );

    gestures += 4;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 5");

    // 6. An edit picked on V2 stays on V2 (spec D36): with V1 cut at the same
    // frame, `.` rolls the V2 edit, and so does Dynamic Trim, where `,` waits
    // for K (D32).
    call(&h, "viewer.set", json!({ "mode": "timeline" }));
    let e = clips(&h, "V1")[0].2;
    for at in [e - 20, e] {
        call(
            &h,
            "edit.overwrite",
            json!({ "media": "m1", "range": "0f..20f", "at": at, "track": "V2", "video_only": true }),
        );
    }
    settle(&h, &mut harness);
    let v2 = clips(&h, "V2");
    let outgoing = v2.iter().find(|c| c.1 + c.2 == e).unwrap().0.clone();
    let l = harness.state().timeline_layout().unwrap();
    let row = l.lower.get("V2").unwrap().rect.rect;
    let at = pos2(l.lower_scale.xf(e), row.top() + 6.0);
    drag(&mut harness, at, at);
    settle(&h, &mut harness);
    harness.key_press(egui::Key::Period);
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!(
        (m.as_str(), &p["clip"], &p["by"]),
        ("edit.roll", &json!(outgoing), &json!(1)),
        "the V2 edit, not V1's"
    );
    let before = gui_edits(&h).len();
    harness.key_press(egui::Key::W);
    harness.key_press(egui::Key::L);
    for _ in 0..12 {
        harness.step();
    }
    harness.key_press(egui::Key::Comma);
    harness.step();
    settle(&h, &mut harness);
    assert_eq!(
        gui_edits(&h).len(),
        before,
        "`,` waits while Dynamic Trim runs"
    );
    harness.key_press(egui::Key::K);
    harness.step();
    settle(&h, &mut harness);
    let edits = gui_edits(&h);
    assert_eq!(edits.len(), before + 1, "one request when K stops it");
    let (m, p) = edits.last().unwrap();
    assert_eq!((m.as_str(), &p["clip"]), ("edit.roll", &json!(outgoing)));
    // L rolled it forward, a frame per frame of the run at most.
    let by = p["by"].as_i64().unwrap_or(0);
    assert!((1..=12).contains(&by), "rolled by {by}");
    harness.key_press(egui::Key::W);
    harness.step();

    gestures += 2;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 6");

    // 7. Removing a track renumbers those above it: an edit picked on V2
    // isn't kept as "V2" once that is another track (V3 with an edit at
    // the same frame here).
    let e = clips(&h, "V2")
        .iter()
        .find(|c| c.0 == outgoing)
        .map(|c| c.1 + c.2)
        .unwrap();
    call(
        &h,
        "edit.overwrite",
        json!({ "media": "m1", "range": "0f..20f", "at": e - 20, "track": "V3", "video_only": true }),
    );
    settle(&h, &mut harness);
    let l = harness.state().timeline_layout().unwrap();
    let row = l.lower.get("V2").unwrap().rect.rect;
    let at = pos2(l.lower_scale.xf(e), row.top() + 6.0);
    drag(&mut harness, at, at);
    settle(&h, &mut harness);
    call(&h, "track.remove", json!({ "track": "V2", "force": true }));
    settle(&h, &mut harness);
    let before = gui_edits(&h).len();
    harness.key_press(egui::Key::Period);
    harness.step();
    settle(&h, &mut harness);
    assert_eq!(gui_edits(&h).len(), before, "no edit is selected any more");

    gestures += 0;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 7");

    // 8. A click on the J/L handle selects that audio edit (D20). The
    // handle sits 8 frames into the next clip, so a picture-cut selection
    // would roll the cut at `cut` and leave the offset at 8. `.` and `,`
    // move the offset instead.
    let v1 = clips(&h, "V1");
    let (outgoing, cut) = (v1[0].0.clone(), v1[0].1 + v1[0].2);
    let incoming = v1[1].0.clone();
    assert_eq!(
        v1[1].1, cut,
        "two clips meet, so the audio band has a handle"
    );
    let offset = 8;
    call(
        &h,
        "edit.audio_roll",
        json!({ "clip": outgoing, "edge": "tail", "by": offset }),
    );
    assert_eq!(
        (
            audio_field(&h, &outgoing, "tail"),
            audio_field(&h, &incoming, "head")
        ),
        (offset, offset)
    );
    let picture: Vec<(i64, i64)> = clips(&h, "V1").iter().map(|c| (c.1, c.2)).collect();
    call(
        &h,
        "player.seek",
        json!({ "at": cut + offset, "viewer": "timeline" }),
    );
    settle(&h, &mut harness);
    let l = harness.state().timeline_layout().unwrap();
    let tr = l.lower.get("V1").unwrap().rect;
    let y = (tr.audio_top + tr.rect.bottom()) / 2.0;
    assert!(
        tr.in_audio(y),
        "the click is on the J/L handle, in the audio band"
    );
    let at = pos2(l.lower_scale.xf(cut + offset), y);
    drag(&mut harness, at, at);
    settle(&h, &mut harness);
    let before = gui_edits(&h).len();
    harness.key_press(egui::Key::Period);
    harness.step();
    settle(&h, &mut harness);
    let edits = gui_edits(&h);
    assert_eq!(
        edits.len(),
        before + 1,
        "the handle click selected the audio edit"
    );
    let (m, p) = edits.last().unwrap();
    assert_eq!(
        (m.as_str(), &p["clip"], &p["edge"], &p["by"]),
        (
            "edit.audio_roll",
            &json!(outgoing),
            &json!("tail"),
            &json!(1)
        ),
        "`.` moves the audio offset; a picture cut would send edit.roll"
    );
    assert_eq!(
        clips(&h, "V1")
            .iter()
            .map(|c| (c.1, c.2))
            .collect::<Vec<_>>(),
        picture,
        "the picture cut stays at {cut}"
    );
    assert_eq!(
        (
            audio_field(&h, &outgoing, "tail"),
            audio_field(&h, &incoming, "head")
        ),
        (offset + 1, offset + 1),
        "the nudge changed the J/L offset"
    );
    // `,` moves the same audio edit back: the selection followed `.`.
    harness.key_press(egui::Key::Comma);
    harness.step();
    settle(&h, &mut harness);
    let (m, p) = last(&h);
    assert_eq!(
        (m.as_str(), &p["clip"], &p["edge"], &p["by"]),
        (
            "edit.audio_roll",
            &json!(outgoing),
            &json!("tail"),
            &json!(-1)
        ),
        "`,` moves the audio offset the other way"
    );
    assert_eq!(
        (
            audio_field(&h, &outgoing, "tail"),
            audio_field(&h, &incoming, "head")
        ),
        (offset, offset)
    );
    assert_eq!(
        clips(&h, "V1")
            .iter()
            .map(|c| (c.1, c.2))
            .collect::<Vec<_>>(),
        picture
    );

    gestures += 2;
    assert_eq!(gui_edits(&h).len() - edits0, gestures, "step 8");

    // Each change is one undo step: the gestures' and this test's own
    // requests over IPC alike. (Each step adds its own count above, so a new
    // step changes no line another step's PR also changes.)
    assert_eq!(
        undo_steps(&h) - undo0,
        gestures + changes_over_ipc(&h) - ipc0
    );

    drop(harness);
    h.shutdown();
    let _ = session.join();
    std::thread::sleep(Duration::from_millis(10));
}
