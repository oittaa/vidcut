//! Edit operation tests. Each cites the Cut-page spec (docs/cut-page-spec.md)
//! or manual page it checks. Media: m1 = 250 frames with audio, m2 = 100
//! frames with audio, m3 = 200 frames without audio (25 fps).

use serde_json::json;

use crate::error::ErrorCode;
use crate::test_support::{Fixture, row};

fn fixture() -> Fixture {
    Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)])
}

/// c1 = m1 0..100 at 0, c2 = m2 10..60 at 100, c3 = m1 100..150 at 150.
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
    // p. 697: Append ignores the playhead and goes after the last clip.
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
fn smart_insert_uses_the_nearest_edit() {
    // p. 696–697: Smart Insert inserts at the edit point nearest the playhead
    // and ripples Track 1.
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
    // 145 is equidistant from the edits at 120 and 170: the earlier one wins.
    let r = f.ok(req(
        "edit.smart_insert",
        json!({"media": "m3", "range": "0f..5f", "playhead": "145f"}),
    ));
    assert_eq!(r.result["at"], 120);
}

#[test]
fn insert_at_splits_the_clip_under_it() {
    // Deviation X5: exact-frame insert for scripts.
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
fn overwrite_writes_in_place() {
    // p. 701: Overwrite replaces material without moving other clips.
    let mut f = three_clips();
    f.ok(req(
        "edit.overwrite",
        json!({"media": "m3", "range": "0f..30f", "at": 80}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 80, 0),
            row("c4", 80, 30, 0),
            row("c6", 110, 40, 20),
            row("c3", 150, 50, 100)
        ]
    );
    // Decision D10: with Ripple On, an overwrite past the end starts at the end.
    f.ok(req(
        "edit.overwrite",
        json!({"media": "m3", "range": "0f..10f", "at": 500}),
    ));
    assert_eq!(f.v1().last().unwrap(), &row("c7", 200, 10, 0));
}

#[test]
fn overwrite_uses_timeline_marks() {
    // p. 701 and decision D7: In places the source In; a lone Out backtimes;
    // both marks make a three-point edit.
    let mut f = three_clips();
    f.ok(req("marks.set", json!({"in": 20, "out": 50})));
    f.ok(req(
        "edit.overwrite",
        json!({"media": "m3", "range": "0f..100f"}),
    ));
    assert_eq!(f.v1()[1], row("c4", 20, 30, 0));
    let mut f = three_clips();
    f.ok(req("marks.set", json!({"out": 50})));
    f.ok(req(
        "edit.overwrite",
        json!({"media": "m3", "range": "0f..20f"}),
    ));
    assert_eq!(f.v1()[1], row("c4", 30, 20, 0));
    // Not enough source media for the marked range.
    let mut f = three_clips();
    f.ok(req("marks.set", json!({"in": 0, "out": 150})));
    let e = f.err(req(
        "edit.overwrite",
        json!({"media": "m2", "range": "50f..60f"}),
    ));
    assert_eq!(e.code, ErrorCode::InvalidEdit);
}

#[test]
fn ripple_overwrite_replaces_and_ripples() {
    // p. 698: replaces the clip under the playhead; later clips move by the
    // length difference.
    let mut f = three_clips();
    f.ok(req(
        "edit.ripple_overwrite",
        json!({"media": "m3", "range": "0f..80f", "at": 120}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c4", 100, 80, 0),
            row("c3", 180, 50, 100)
        ]
    );
    // With an In–Out range the source length wins (decision D8).
    let mut f = three_clips();
    f.ok(req("marks.set", json!({"in": 50, "out": 120})));
    f.ok(req(
        "edit.ripple_overwrite",
        json!({"media": "m3", "range": "0f..10f"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 50, 0),
            row("c4", 50, 10, 0),
            row("c6", 60, 30, 30),
            row("c3", 90, 50, 100)
        ]
    );
    // Marks persist (decision D6); without them there must be a clip under `at`.
    f.ok(req("marks.clear", json!({})));
    let e = f.err(req(
        "edit.ripple_overwrite",
        json!({"media": "m3", "range": "0f..10f", "at": 999}),
    ));
    assert_eq!(e.code, ErrorCode::InvalidEdit);
}

#[test]
fn split_at_points() {
    // p. 723: Split at the playhead; several points at once for scripts.
    let mut f = fixture();
    f.append("m1", 0, 100);
    let r = f.ok(req("edit.split", json!({"at": [30, "60f"]})));
    assert_eq!(r.result["created"], json!(["c2", "c3"]));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 30, 0),
            row("c2", 30, 30, 30),
            row("c3", 60, 40, 60)
        ]
    );
    // Splitting exactly on an edit point does nothing and is an error.
    f.err(req("edit.split", json!({"at": 30})));
    // Decision D2: with a selection, only selected clips are split.
    f.err(req("edit.split", json!({"at": 70, "clips": ["c1"]})));
    f.ok(req("edit.split", json!({"at": 70, "clips": ["c3"]})));
    assert_eq!(f.v1().len(), 4);
}

#[test]
fn delete_ripples_on_track_one_and_lift_leaves_a_gap() {
    // p. 724–725: Delete ripples Track 1; decision D9: lift leaves a gap.
    let mut f = three_clips();
    f.ok(req("edit.delete", json!({"clips": "c2"})));
    assert_eq!(f.v1(), vec![row("c1", 0, 100, 0), row("c3", 100, 50, 100)]);
    let mut f = three_clips();
    f.ok(req("edit.delete", json!({"clips": "c2", "lift": true})));
    assert_eq!(f.v1(), vec![row("c1", 0, 100, 0), row("c3", 150, 50, 100)]);
    let mut f = three_clips();
    f.ok(req("edit.delete", json!({"clips": ["c1", "c3"]})));
    assert_eq!(f.v1(), vec![row("c2", 0, 50, 10)]);
    // Clip references by position.
    let mut f = three_clips();
    f.ok(req("edit.delete", json!({"clips": "V1@120f"})));
    assert_eq!(f.v1().len(), 2);
}

#[test]
fn delete_range_and_close_gap() {
    // p. 725: Delete with timeline In/Out removes the range.
    let mut f = three_clips();
    f.ok(req("edit.delete_range", json!({"range": "80f..120f"})));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 80, 0),
            row("c5", 80, 30, 30),
            row("c3", 110, 50, 100)
        ]
    );
    let mut f = three_clips();
    f.ok(req("edit.delete", json!({"clips": "c2", "lift": true})));
    f.ok(req("edit.close_gap", json!({"at": 120})));
    assert_eq!(f.v1(), vec![row("c1", 0, 100, 0), row("c3", 100, 50, 100)]);
    f.err(req("edit.close_gap", json!({"at": 120})));
}

#[test]
fn move_inserts_at_an_edit_and_closes_the_gap() {
    // p. 721–722: dropping on an edit point inserts there; Track 1 closes the
    // gap the clip leaves.
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
fn move_overwrite_and_replace() {
    // p. 721: hovering before dropping overwrites; dropping straight onto a
    // clip replaces it and ripples.
    let mut f = three_clips();
    f.ok(req(
        "edit.move",
        json!({"clips": "c3", "to": 20, "mode": "overwrite"}),
    ));
    // Clearing 20..70 splits c1 at 20 (c4) and c4 at 70 (c5); c4 is removed.
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 20, 0),
            row("c3", 20, 50, 100),
            row("c5", 70, 30, 70),
            row("c2", 100, 50, 10)
        ]
    );
    let mut f = three_clips();
    f.ok(req("edit.replace", json!({"clip": "c3", "onto": "c1"})));
    assert_eq!(f.v1(), vec![row("c3", 0, 50, 100), row("c2", 50, 50, 10)]);
}

#[test]
fn trim_ripples_on_track_one() {
    // p. 716, 726: resizing on Track 1 with Ripple On moves later clips.
    let mut f = three_clips();
    f.ok(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "tail", "by": 10}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 110, 0),
            row("c2", 110, 50, 10),
            row("c3", 160, 50, 100)
        ]
    );
    f.ok(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "head", "by": "10f"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 110, 0),
            row("c2", 110, 40, 20),
            row("c3", 150, 50, 100)
        ]
    );
    f.ok(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "tail", "to": 90}),
    ));
    assert_eq!(f.v1()[0], row("c1", 0, 90, 0));
    assert_eq!(f.v1()[1], row("c2", 90, 40, 20));
}

#[test]
fn trim_without_ripple_overwrites_neighbors() {
    // p. 716: Alt (ripple off) trims in place; extending writes over the neighbor.
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
fn trims_respect_source_media() {
    let mut f = three_clips();
    f.err(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "head", "by": -1}),
    ));
    f.err(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "tail", "by": 41}),
    ));
    f.ok(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "tail", "by": 40}),
    ));
    f.err(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "tail", "by": -100}),
    ));
    f.err(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "tail", "by": 5, "to": 10}),
    ));
}

#[test]
fn roll_and_slip() {
    // p. 727: roll moves an edit, keeping the program length; slip changes
    // which source frames a clip shows.
    let mut f = three_clips();
    f.ok(req(
        "edit.roll",
        json!({"clip": "c1", "edge": "tail", "by": 5}),
    ));
    assert_eq!(f.v1()[..2], [row("c1", 0, 105, 0), row("c2", 105, 45, 15)]);
    f.err(req(
        "edit.roll",
        json!({"clip": "c2", "edge": "head", "by": -21}),
    ));
    f.ok(req("edit.slip", json!({"clip": "c2", "by": 40})));
    assert_eq!(f.v1()[1], row("c2", 105, 45, 55));
    f.err(req("edit.slip", json!({"clip": "c2", "by": 1})));
    f.err(req(
        "edit.roll",
        json!({"clip": "c3", "edge": "tail", "by": 1}),
    ));
}

#[test]
fn audio_roll_makes_j_and_l_cuts() {
    // p. 728: dragging the audio edit moves only the audio.
    let mut f = three_clips();
    f.ok(req(
        "edit.audio_roll",
        json!({"clip": "c1", "edge": "tail", "by": 5}),
    ));
    let p = f.editor.project();
    let (c1, c2) = (p.clip("c1").unwrap(), p.clip("c2").unwrap());
    assert_eq!(
        (
            c1.audio.as_ref().unwrap().tail,
            c2.audio.as_ref().unwrap().head
        ),
        (5, 5)
    );
    assert_eq!(c1.audio_range(), Some((0, 105)));
    assert_eq!(c2.audio_range(), Some((105, 150)));
    // Decision D14: when the clip at an L cut gets a new neighbor, the cut resets.
    f.ok(req("edit.delete", json!({"clips": "c2"})));
    assert_eq!(
        f.editor
            .project()
            .clip("c1")
            .unwrap()
            .audio
            .as_ref()
            .unwrap()
            .tail,
        0
    );
    // Audio needs source media to extend into.
    let mut f = three_clips();
    f.err(req(
        "edit.audio_roll",
        json!({"clip": "c2", "edge": "head", "by": -11}),
    ));
}

#[test]
fn trim_to_playhead_and_change_duration() {
    // p. 729 and decision D4; p. 693 Change Clip Duration.
    let mut f = three_clips();
    f.ok(req(
        "edit.trim_to_playhead",
        json!({"edge": "tail", "at": 80}),
    ));
    assert_eq!(f.v1()[..2], [row("c1", 0, 80, 0), row("c2", 80, 50, 10)]);
    f.ok(req(
        "edit.trim_to_playhead",
        json!({"edge": "head", "at": 90}),
    ));
    assert_eq!(f.v1()[1], row("c2", 80, 40, 20));
    f.ok(req(
        "edit.change_duration",
        json!({"clips": ["c1", "c3"], "duration": "2s"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 50, 0),
            row("c2", 50, 40, 20),
            row("c3", 90, 50, 100)
        ]
    );
}

#[test]
fn clip_properties_marks_and_markers() {
    let mut f = three_clips();
    f.ok(req(
        "clip.set",
        json!({"clips": "c2", "enabled": false, "gain_db": -6.0, "muted": true, "name": "B-roll"}),
    ));
    let c2 = f.editor.project().clip("c2").unwrap().clone();
    assert!(!c2.enabled);
    assert_eq!(c2.name.as_deref(), Some("B-roll"));
    assert_eq!(
        (
            c2.audio.as_ref().unwrap().gain_db,
            c2.audio.as_ref().unwrap().muted
        ),
        (-6.0, true)
    );
    f.err(req("clip.set", json!({"clips": "c2", "gain_db": 30.0})));
    f.err(req("marks.set", json!({"in": 50, "out": 50})));
    f.ok(req("marks.set", json!({"in": "1s", "out": "2s"})));
    assert_eq!(
        (
            f.editor.project().marks.mark_in,
            f.editor.project().marks.mark_out
        ),
        (Some(25), Some(50))
    );
    f.ok(req("marks.clear", json!({})));
    let r = f.ok(req(
        "markers.add",
        json!({"at": "00:00:02:00", "name": "chorus"}),
    ));
    assert_eq!(r.result["marker"], "k1");
    f.ok(req("markers.move", json!({"marker": "k1", "to": 75})));
    assert_eq!(f.editor.project().markers[0].at, 75);
    f.ok(req("markers.remove", json!({"marker": "k1"})));
    f.err(req("markers.remove", json!({"marker": "k1"})));
}

#[test]
fn revisions_undo_redo_and_conflicts() {
    let mut f = fixture();
    let rev0 = f.editor.project().rev;
    let base = f.editor.project().clone();
    f.append("m1", 0, 100);
    f.append("m2", 0, 50);
    assert_eq!(f.editor.project().rev, rev0 + 2);
    let r = f.ok(req("history.undo", json!({"steps": 2})));
    assert_eq!(r.rev, rev0 + 3, "revisions only grow");
    assert_eq!(f.editor.project().tracks, base.tracks);
    f.ok(req("history.redo", json!({})));
    assert_eq!(f.v1().len(), 1);
    f.err(req("history.redo", json!({"steps": 5})));
    // if_rev guards against concurrent edits.
    let rev = f.editor.project().rev;
    let e = f.err(json!({"method": "edit.delete", "params": {"clips": "c1", "if_rev": rev - 1}}));
    assert_eq!(e.code, ErrorCode::Conflict);
    f.ok(json!({"method": "edit.delete", "params": {"clips": "c1", "if_rev": rev}}));
    // A new edit drops the redo stack.
    f.ok(req("history.undo", json!({})));
    f.append("m3", 0, 10);
    f.err(req("history.redo", json!({})));
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
fn batches_are_atomic_and_one_undo_step() {
    let mut f = fixture();
    let before = f.editor.project().clone();
    let e = f.err(req(
        "apply",
        json!({"commands": [
            {"method": "edit.append", "params": {"media": "m1"}},
            {"method": "edit.delete", "params": {"clips": "c99"}}
        ]}),
    ));
    assert!(
        e.message.starts_with("command 1 (edit.delete)"),
        "{}",
        e.message
    );
    assert_eq!(f.editor.project(), &before);
    let r = f.ok(req(
        "apply",
        json!({"label": "rough cut", "commands": [
            {"method": "edit.append", "params": {"media": "m1", "range": "0f..100f"}},
            {"method": "edit.append", "params": {"media": "m2", "range": "0f..50f"}},
            {"method": "edit.split", "params": {"at": 50}}
        ]}),
    ));
    assert_eq!(r.changes.created, vec!["c1", "c2", "c3"]);
    assert_eq!(f.v1().len(), 3);
    assert_eq!(f.editor.history().undo.last().unwrap().label, "rough cut");
    f.ok(req("history.undo", json!({})));
    assert!(f.v1().is_empty());
}

#[test]
fn bad_requests_are_usage_errors() {
    let mut f = fixture();
    let e = f.err(req(
        "edit.append",
        json!({"media": "m1", "rnage": "0f..10f"}),
    ));
    assert_eq!(e.code, ErrorCode::Usage);
    assert!(e.message.contains("rnage"), "{}", e.message);
    assert_eq!(
        f.err(req("edit.append", json!({"media": "m9"}))).code,
        ErrorCode::NotFound
    );
    assert_eq!(
        f.err(req(
            "edit.append",
            json!({"media": "m1", "range": "0f..300f"})
        ))
        .code,
        ErrorCode::InvalidEdit
    );
    assert_eq!(
        f.err(req("edit.append", json!({"media": "m1", "range": "0..10"})))
            .code,
        ErrorCode::Usage
    );
    assert_eq!(
        f.err(req(
            "edit.smart_insert",
            json!({"media": "m1", "playhead": "playhead"})
        ))
        .code,
        ErrorCode::Usage
    );
}

#[test]
fn first_video_sets_the_project_format() {
    // p. 654: importing into an empty project offers the media's frame rate.
    let mut f = Fixture::new(&[]);
    f.env = std::mem::take(&mut f.env).with(
        "/media/a.mp4",
        crate::test_support::media_info(
            2.0,
            crate::time::Rational::new(30000, 1001).unwrap(),
            true,
        ),
    );
    let mut settings = f.editor.project().settings.clone();
    settings.auto_format = true;
    f.editor = crate::Editor::new(crate::Project::new(settings), Default::default());
    f.ok(req(
        "media.add",
        json!({"paths": ["/media/a.mp4", "/media/a.mp4"]}),
    ));
    let p = f.editor.project();
    assert_eq!(p.settings.fps.to_string(), "30000/1001");
    assert!(!p.settings.auto_format);
    assert_eq!(p.media.len(), 1, "adding the same file twice reuses it");
    // The frame rate can't change once clips exist.
    f.append("m1", 0, 10);
    f.err(req("project.set", json!({"fps": "25"})));
    f.ok(req("project.set", json!({"ripple_on": false})));
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
    // Decision D14 (found by the property test): a split inside the zone where
    // the previous clip's audio overlaps would leave a piece without audio.
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
fn timeline_json_is_stable() {
    // Golden output agents parse: review snapshot changes deliberately.
    let mut f = three_clips();
    f.ok(req("edit.delete", json!({"clips": "c2", "lift": true})));
    f.ok(req("marks.set", json!({"in": 10, "out": 20})));
    f.ok(req("markers.add", json!({"at": 5, "name": "intro"})));
    let r = f.ok(req("timeline.get", json!({})));
    insta::assert_json_snapshot!("timeline_get", r.result);
    let r = f.ok(req("timeline.at", json!({"at": "00:00:06:05"})));
    insta::assert_json_snapshot!("timeline_at", r.result);
    let e = f.err(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "head", "by": -1}),
    ));
    insta::assert_json_snapshot!("trim_error", e);
}
