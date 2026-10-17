use serde_json::json;

use crate::error::ErrorCode;
use crate::test_support::{Fixture, row};

fn fixture() -> Fixture {
    Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)])
}

fn three_clips() -> Fixture {
    let mut f = fixture();
    f.append("m1", 0, 100);
    f.append("m2", 10, 60);
    f.append("m1", 100, 150);
    f
}

fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ "method": method, "params": params })
}

#[test]
fn append_places_clips_back_to_back() {
    let f = three_clips();
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c2", 100, 50, 10),
            row("c3", 150, 50, 100)
        ]
    );
}

#[test]
fn the_smart_indicator_is_red_without_media_past_the_edit() {
    let mut f = three_clips();
    f.append("m2", 0, 100);
    f.append("m1", 100, 150);
    let p = f.editor.project();
    let red: Vec<i64> = [0, 100, 150, 200, 300, 350]
        .into_iter()
        .filter(|&e| !crate::ops::edit_has_handles(p, e))
        .collect();
    assert_eq!(red, vec![0, 200, 300]);
}

#[test]
fn smart_insert_uses_the_nearest_edit() {
    let mut f = three_clips();
    let r = f.ok(req(
        "edit.smart_insert",
        json!({"media": "m3", "range": "0f..20f", "playhead": 110}),
    ));
    assert_eq!(r.result["at"], 100);
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c4", 100, 20, 0),
            row("c2", 120, 50, 10),
            row("c3", 170, 50, 100)
        ]
    );
    let r = f.ok(req(
        "edit.smart_insert",
        json!({"media": "m3", "range": "0f..5f", "playhead": "145f"}),
    ));
    assert_eq!(r.result["at"], 120);
}

#[test]
fn insert_at_splits_the_clip_under_it() {
    let mut f = fixture();
    f.append("m1", 0, 100);
    f.ok(req(
        "edit.insert",
        json!({"media": "m2", "range": "0f..10f", "at": 40}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 40, 0),
            row("c2", 40, 10, 0),
            row("c3", 50, 60, 40)
        ]
    );
}

#[test]
fn move_inserts_at_an_edit_and_closes_the_gap() {
    let mut f = three_clips();
    f.ok(req("edit.move", json!({"clips": "c3", "to": 0})));
    assert_eq!(
        f.v1(),
        vec![
            row("c3", 0, 50, 100),
            row("c1", 50, 100, 0),
            row("c2", 150, 50, 10)
        ]
    );
    let mut f = three_clips();
    f.ok(req("edit.move", json!({"clips": "c1", "to": "end"})));
    assert_eq!(
        f.v1(),
        vec![
            row("c2", 0, 50, 10),
            row("c3", 50, 50, 100),
            row("c1", 100, 100, 0)
        ]
    );
    let mut f = three_clips();
    f.err(req("edit.move", json!({"clips": "c1", "to": 50})));
}

#[test]
fn trim_without_ripple_overwrites_neighbors() {
    let mut f = three_clips();
    f.ok(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "tail", "by": 10, "ripple": false}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 110, 0),
            row("c4", 110, 40, 20),
            row("c3", 150, 50, 100)
        ]
    );
    let mut f = three_clips();
    f.ok(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "head", "by": -10, "ripple": false}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 90, 0),
            row("c2", 90, 60, 0),
            row("c3", 150, 50, 100)
        ]
    );
}

#[test]
fn a_roll_moves_the_cut_and_keeps_the_timeline() {
    let mut f = three_clips();
    f.ok(req(
        "edit.roll",
        json!({"clip": "c1", "edge": "tail", "by": 10}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 110, 0),
            row("c2", 110, 40, 20),
            row("c3", 150, 50, 100)
        ]
    );
}

#[test]
fn a_slip_shows_other_frames_in_the_same_place() {
    let mut f = three_clips();
    f.ok(req("edit.slip", json!({"clip": "c2", "by": 5})));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c2", 100, 50, 15),
            row("c3", 150, 50, 100)
        ]
    );
}

#[test]
fn a_slide_moves_the_clip_between_its_neighbors() {
    let mut f = three_clips();
    f.ok(req("edit.slide", json!({"clip": "c2", "by": 10})));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 110, 0),
            row("c2", 110, 50, 10),
            row("c3", 160, 40, 110)
        ]
    );
}

#[test]
fn a_clip_heard_only_past_its_picture_keeps_its_audio() {
    let mut f = three_clips();
    f.ok(req(
        "edit.audio_roll",
        json!({"clip": "c3", "edge": "head", "by": 6}),
    ));
    f.ok(req(
        "edit.audio_roll",
        json!({"clip": "c1", "edge": "tail", "by": 50}),
    ));
    let range = |f: &crate::test_support::Fixture, c: &str| {
        f.editor.project().clip(c).unwrap().audio_range()
    };
    assert_eq!(
        range(&f, "c2"),
        Some((150, 156)),
        "heard only past its picture"
    );
    f.ok(req("edit.delete", json!({"clips": "c3"})));
    assert_eq!(
        (range(&f, "c1"), range(&f, "c2")),
        (Some((0, 100)), Some((100, 150)))
    );
}

#[test]
fn a_clip_moved_to_another_track_is_modified() {
    let mut f = three_clips();
    let r = f.ok(req(
        "edit.move",
        json!({"clips": "c3", "to": "150f", "track": "V2"}),
    ));
    assert_eq!(r.changes.modified, vec!["c3"]);
}

#[test]
fn dry_run_reports_without_committing() {
    let mut f = three_clips();
    let before = f.editor.project().clone();
    let r = f.ok(json!({"method": "edit.delete", "params": {"clips": "c2", "dry_run": true}}));
    assert!(r.dry_run);
    assert_eq!(r.changes.deleted, vec!["c2"]);
    assert_eq!(r.changes.modified, vec!["c3"]);
    assert_eq!(f.editor.project(), &before);
}

#[test]
fn a_portrait_phone_clip_makes_a_portrait_project() {
    let mut info =
        crate::test_support::media_info(2.0, crate::time::Rational::new(30, 1).unwrap(), true);
    info.video.as_mut().unwrap().rotation = 90;
    let mut f = Fixture::new(&[]);
    f.env = std::mem::take(&mut f.env).with("/media/phone.mp4", info);
    let mut settings = f.editor.project().settings.clone();
    settings.auto_format = true;
    f.editor = crate::Editor::new(crate::Project::new(settings), Default::default());
    f.ok(req("media.add", json!({"paths": ["/media/phone.mp4"]})));
    let s = &f.editor.project().settings;
    assert_eq!((s.width, s.height), (1080, 1920));
}

#[test]
fn media_remove_needs_force_when_used() {
    let mut f = three_clips();
    f.err(req("media.remove", json!({"media": "m1"})));
    f.ok(req("media.remove", json!({"media": "m1", "force": true})));
    assert_eq!(f.v1(), vec![row("c2", 0, 50, 10)]);
    f.ok(req("media.remove", json!({"media": "m3"})));
}

#[test]
fn splitting_inside_an_l_cut_resets_it() {
    let mut f = three_clips();
    f.ok(req(
        "edit.audio_roll",
        json!({"clip": "c1", "edge": "tail", "by": 10}),
    ));
    f.ok(req("edit.split", json!({"at": 105})));
    let p = f.editor.project();
    assert_eq!(p.clip("c1").unwrap().audio.as_ref().unwrap().tail, 0);
    assert_eq!(p.clip("c2").unwrap().audio_range(), Some((100, 105)));
    assert!(crate::validate::violations(p).is_empty());
}

#[test]
fn joining_at_the_playhead_undoes_a_split() {
    let mut f = three_clips();
    f.ok(req("edit.split", json!({"at": 40})));
    let r = f.ok(req("edit.join", json!({"at": 40})));
    assert_eq!(r.result["joined"], json!(["c1"]));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c2", 100, 50, 10),
            row("c3", 150, 50, 100)
        ]
    );
}

#[test]
fn a_cut_that_skips_frames_is_not_joined() {
    let mut f = fixture();
    f.append("m1", 0, 50);
    f.append("m1", 60, 100);
    f.err(req("edit.join", json!({"at": 50})));
}

#[test]
fn selected_parts_join_into_the_first() {
    let mut f = three_clips();
    f.ok(req("edit.split", json!({"at": [20, 40, 60]})));
    f.ok(req("edit.join", json!({"clips": ["c4", "c5", "c6"]})));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 20, 0),
            row("c4", 20, 80, 20),
            row("c2", 100, 50, 10),
            row("c3", 150, 50, 100)
        ]
    );
}

#[test]
fn parts_with_an_l_cut_between_them_join() {
    let mut f = three_clips();
    f.ok(req("edit.split", json!({"at": 50})));
    f.ok(req(
        "edit.audio_roll",
        json!({"clip": "c1", "edge": "tail", "by": 10}),
    ));
    f.ok(req("edit.join", json!({"at": 50})));
    let c1 = f.editor.project().clip("c1").unwrap();
    assert_eq!((c1.duration, c1.audio_range()), (100, Some((0, 100))));
}

fn tape_order() -> serde_json::Value {
    json!(["m1", "m2", "m3"])
}

#[test]
fn tape_marks_on_one_clip_mirror_source_marks() {
    let mut f = fixture();
    let r = f.ok(req(
        "tape.marks.set",
        json!({"in": 50, "out": 100, "order": tape_order()}),
    ));
    assert_eq!(r.result["in"], 50);
    assert_eq!(r.result["out"], 100);
    assert_eq!(
        r.result["pieces"],
        json!([{"media": "m1", "in": 50, "out": 100}])
    );
    let p = f.editor.project();
    assert_eq!(p.tape_marks.mark_in, Some(50));
    assert_eq!(p.tape_marks.mark_out, Some(100));
    let m1 = p.media("m1").unwrap().marks.clone();
    assert_eq!((m1.mark_in, m1.mark_out), (Some(50), Some(100)));
    assert!(p.media("m2").unwrap().marks.is_empty());
}

#[test]
fn tape_marks_clear() {
    let mut f = fixture();
    f.ok(req(
        "tape.marks.set",
        json!({"in": 50, "out": 100, "order": tape_order()}),
    ));
    f.ok(req("tape.marks.clear", json!({"media": "m1"})));
    let p = f.editor.project();
    assert!(p.tape_marks.is_empty());
    assert!(p.media("m1").unwrap().marks.is_empty());

    f.ok(req(
        "tape.marks.set",
        json!({"in": 200, "out": 300, "order": tape_order()}),
    ));
    f.ok(req("tape.marks.clear", json!({})));
    assert!(f.editor.project().tape_marks.is_empty());
}

fn tape_span() -> Fixture {
    let mut f = three_clips();
    f.ok(req(
        "tape.marks.set",
        json!({"in": 200, "out": 300, "order": tape_order()}),
    ));
    f
}

#[test]
fn a_tape_span_placed_on_top_is_one_block() {
    let mut f = tape_span();
    let r = f.ok(req("edit.place_on_top", json!({"tape": true, "at": 52})));
    assert_eq!(r.result["track"], "V2");
    assert_eq!(r.result["clips"], json!(["c4", "c5"]));
    assert_eq!(
        f.track("V2"),
        vec![row("c4", 52, 50, 200), row("c5", 102, 50, 0)]
    );
}

#[test]
fn a_tape_span_inserts_and_ripples_as_one_block() {
    let mut f = tape_span();
    f.ok(req("edit.insert", json!({"tape": true, "at": 100})));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c4", 100, 50, 200),
            row("c5", 150, 50, 0),
            row("c2", 200, 50, 10),
            row("c3", 250, 50, 100),
        ]
    );
}

#[test]
fn a_tape_span_ripple_overwrites_a_clip() {
    let mut f = tape_span();
    f.ok(req(
        "edit.ripple_overwrite",
        json!({"tape": true, "at": 120}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c4", 100, 50, 200),
            row("c5", 150, 50, 0),
            row("c3", 200, 50, 100),
        ]
    );
}

#[test]
fn a_tape_span_fills_the_timeline_marks() {
    let mut f = tape_span();
    f.ok(req("marks.set", json!({"in": 10, "out": 80})));
    let r = f.ok(req("edit.overwrite", json!({"tape": true})));
    assert_eq!(r.result["clips"], json!(["c4", "c5"]));
    let p = f.editor.project();
    let placed = |id: &str| {
        let c = p.clip(id).unwrap();
        (c.media.clone(), c.start, c.duration, c.source_in)
    };
    assert_eq!(placed("c4"), ("m1".into(), 10, 50, 200));
    assert_eq!(placed("c5"), ("m2".into(), 60, 20, 0));
}

#[test]
fn removing_media_on_the_tape_clears_its_marks() {
    let mut f = tape_span();
    f.ok(req("media.remove", json!({"media": "m3"})));
    assert!(f.editor.project().tape_marks.is_empty());
}

#[test]
fn live_requests_need_a_running_gui() {
    let mut f = three_clips();
    for (method, params) in [
        ("player.seek", json!({"at": "1s"})),
        ("viewer.set", json!({"mode": "source"})),
        ("selection.get", json!({})),
        ("gui.screenshot", json!({"out": "/tmp/x.png"})),
    ] {
        let e = f.err(req(method, params));
        assert_eq!(e.code, ErrorCode::NoInstance, "{method}: {}", e.message);
    }
}

#[test]
fn drag_and_drop_edits_ignore_timeline_marks() {
    let mut f = three_clips();
    f.ok(req("marks.set", json!({"in": 10, "out": 20})));
    f.ok(req(
        "edit.overwrite",
        json!({"media": "m3", "range": "0f..5f", "at": 120, "use_marks": false}),
    ));
    assert_eq!(f.v1()[2], row("c4", 120, 5, 0));
    f.ok(req(
        "edit.ripple_overwrite",
        json!({"media": "m3", "range": "0f..30f", "at": 160, "use_marks": false}),
    ));
    assert_eq!(f.v1().last().unwrap(), &row("c7", 150, 30, 0));
    let marks = &f.editor.project().marks;
    assert_eq!((marks.mark_in, marks.mark_out), (Some(10), Some(20)));
}

#[test]
fn change_duration_to_the_end_of_the_media() {
    let mut f = three_clips();
    f.ok(req(
        "edit.change_duration",
        json!({"clips": "c2", "duration": "end"}),
    ));
    assert_eq!(f.v1()[1], row("c2", 100, 90, 10));
    assert_eq!(f.v1()[2], row("c3", 190, 50, 100));
}

#[test]
fn an_edit_that_changes_nothing_is_no_undo_step() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 100);
    let rev = f
        .ok(json!({"method": "track.set", "params": {"track": "V1", "volume_db": -6.0}}))
        .rev;
    let same = f.ok(json!({"method": "track.set", "params": {"track": "V1", "volume_db": -6.0}}));
    assert_eq!(same.rev, rev);
    assert!(same.changes.is_empty());
    let undo = f.ok(json!({"method": "history.undo", "params": {}}));
    assert_eq!(undo.rev, rev + 1);
    let v1 = f.editor.project().track("V1").unwrap().volume_db;
    assert_eq!(v1, 0.0, "the undo took back the real change");
}

#[test]
fn a_clip_shows_its_media_color_until_it_has_its_own() {
    let mut f = three_clips();
    f.ok(req("media.set", json!({"media": "m1", "color": "teal"})));
    f.ok(req("clip.set", json!({"clips": "c3", "color": "pink"})));
    let p = f.editor.project();
    let shown: Vec<Option<&str>> = ["c1", "c2", "c3"]
        .iter()
        .map(|id| p.clip_color(p.clip(id).unwrap()))
        .collect();
    assert_eq!(shown, vec![Some("teal"), None, Some("pink")]);
}

#[test]
fn an_unknown_clip_color_is_refused() {
    let mut f = three_clips();
    f.err(req("clip.set", json!({"clips": "c1", "color": "red"})));
}
