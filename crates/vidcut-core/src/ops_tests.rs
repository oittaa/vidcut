//! Edit operation tests. Each cites the spec section or decision
//! (docs/cut-page-spec.md) it checks. Media: m1 = 250 frames with audio, m2 = 100
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
    // Append ignores the playhead and goes after the last clip.
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
    // Red where a clip shows its media's first or last frame.
    let mut f = three_clips();
    f.append("m2", 0, 100); // all of m2, at 200..300
    f.append("m1", 100, 150); // at 300..350
    let p = f.editor.project();
    let red: Vec<i64> = [0, 100, 150, 200, 300, 350]
        .into_iter()
        .filter(|&e| !crate::ops::edit_has_handles(p, e))
        .collect();
    // 0: c1 starts at m1's first frame; 200: c4 at m2's first; 300: c4 ends
    // at m2's last, though c5 has media before its start. 100, 150 and 350
    // have media on both sides.
    assert_eq!(red, vec![0, 200, 300]);
}

#[test]
fn the_smart_indicator_goes_by_track_one_alone() {
    // D36: the indicator shows Track 1's edit. A higher track's clips there
    // don't change its color, with media past the edit or without.
    let mut f = three_clips();
    f.append("m2", 50, 100); // c4 at 200..250, ending on m2's last frame
    let on_v2 = |f: &mut Fixture, media: &str, a: i64, b: i64, at: i64| {
        f.ok(req(
            "edit.overwrite",
            json!({"media": media, "range": format!("{a}f..{b}f"), "at": at, "track": "V2"}),
        ));
    };
    // Green on Track 1 at 100 and 150: V2 ends on m2's last frame at 100 and
    // starts on m3's first at 150.
    on_v2(&mut f, "m2", 0, 100, 0);
    on_v2(&mut f, "m3", 0, 40, 150);
    // Red on Track 1 at 250 (c4): V2 has media on both sides of it there.
    on_v2(&mut f, "m3", 10, 30, 230);
    on_v2(&mut f, "m3", 50, 70, 250);
    assert_eq!(
        f.track("V2"),
        vec![
            row("c5", 0, 100, 0),
            row("c6", 150, 40, 0),
            row("c7", 230, 20, 10),
            row("c8", 250, 20, 50)
        ]
    );
    let p = f.editor.project();
    let red: Vec<i64> = [100, 150, 250]
        .into_iter()
        .filter(|&e| !crate::ops::edit_has_handles(p, e))
        .collect();
    assert_eq!(red, vec![250], "as on Track 1 alone");
}

#[test]
fn smart_insert_uses_the_nearest_edit() {
    // Smart Insert inserts at the edit point nearest the playhead
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
    // Overwrite replaces material without moving other clips.
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
    // Decision D7: In places the source In; a lone Out backtimes;
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
    // Replaces the clip under the playhead; later clips move by the
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
    // Split at the playhead; several points at once for scripts.
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
    // Decision D2: selected clips under the point are the only ones split
    // there; when none of them is there, everything under it is.
    f.ok(req("edit.split", json!({"at": 70, "clips": ["c1"]})));
    assert_eq!(f.v1().len(), 4);
    f.ok(req("edit.split", json!({"at": 80, "clips": ["c4"]})));
    assert_eq!(f.v1().len(), 5);
}

#[test]
fn delete_ripples_on_track_one_and_lift_leaves_a_gap() {
    // Delete ripples Track 1; decision D9: lift leaves a gap.
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
    // Delete with timeline In/Out removes the range.
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
    // Dropping on an edit point inserts there; Track 1 closes the
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
    // Hovering before dropping overwrites; dropping straight onto a
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
    // Resizing on Track 1 with Ripple On moves later clips.
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
    // Alt (ripple off) trims in place; extending writes over the neighbor.
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
    // Roll moves an edit, keeping the program length; slip changes
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
    // Dragging the audio edit moves only the audio.
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
fn a_clip_heard_only_past_its_picture_keeps_its_audio() {
    // Decision D14. c1's L cut covers all of c2's picture, and c2 is heard
    // only in its own L cut into c3. Deleting c3 resets that cut, which
    // would leave c2 no audio at all: the cut before it resets too, and c2
    // is heard over its picture again. (A property test found it.)
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
fn trim_to_playhead_and_change_duration() {
    // Decision D4; Change Clip Duration.
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
    // Importing into an empty project offers the media's frame rate.
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
fn drop_frame_timecode_is_a_project_setting() {
    // Decision D47: at 29.97 and 59.94 record timecode may count
    // drop-frame; the first video, whose own timecode does, sets it.
    let ntsc = crate::time::Rational::new(30000, 1001).unwrap();
    let mut cam = crate::test_support::media_info(120.0, ntsc, true);
    cam.timecode = Some(crate::model::StartTimecode {
        count: 107_892,
        rate: ntsc,
        drop_frame: true,
    });
    let mut f = Fixture::new(&[]);
    f.env = std::mem::take(&mut f.env).with("/media/cam.mov", cam.clone());
    let mut settings = f.editor.project().settings.clone();
    settings.auto_format = true;
    f.editor = crate::Editor::new(crate::Project::new(settings), Default::default());
    f.ok(req("media.add", json!({"paths": ["/media/cam.mov"]})));
    assert!(f.editor.project().settings.counts_drop_frame());
    f.append("m1", 0, 1800);
    let clip = |f: &mut Fixture, key: &str| {
        f.ok(req("timeline.get", json!({}))).result["tracks"][0]["items"][0][key].clone()
    };
    assert_eq!(clip(&mut f, "end_tc"), "00:01:00;02");
    // A drop-frame start: an hour of the clock is 107892 frames.
    f.ok(req("project.set", json!({"start_tc": "01:00:00;00"})));
    assert_eq!(f.editor.project().settings.start_tc, 107_892);
    assert_eq!(clip(&mut f, "start_tc"), "01:00:00;00");
    // An import into an empty project takes it from its media: the file
    // doesn't say.
    let doc = crate::otio::export(
        f.editor.project(),
        "cam",
        None,
        crate::otio::Locations::Absolute,
    )
    .unwrap();
    let mut g = Fixture::new(&[]);
    g.env = std::mem::take(&mut g.env).with("/media/cam.mov", cam);
    g.ok(req("timeline.import", json!({"otio": doc})));
    let s = &g.editor.project().settings;
    assert!(s.counts_drop_frame() && s.start_tc == 107_892);
    // Off, the same frames show non-drop.
    f.ok(req("project.set", json!({"drop_frame": false})));
    assert_eq!(clip(&mut f, "start_tc"), "00:59:56:12");
    // Not at 25.
    let e = Fixture::new(&[]).err(req("project.set", json!({"drop_frame": true})));
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
}

#[test]
fn the_start_timecode_keeps_its_label() {
    // Deviation X7: a start set as 01:00:00:00 stays so when the rate or
    // drop-frame counting changes, another frame count each time.
    let mut f = Fixture::new(&[]);
    let start = |f: &Fixture| {
        let s = &f.editor.project().settings;
        (s.start_tc, s.timecode(0))
    };
    f.ok(req("project.set", json!({"start_tc": "01:00:00:00"})));
    assert_eq!(start(&f), (90_000, "01:00:00:00".to_string()));
    f.ok(req("project.set", json!({"fps": "30000/1001"})));
    assert_eq!(start(&f), (108_000, "01:00:00:00".to_string()));
    f.ok(req("project.set", json!({"drop_frame": true})));
    assert_eq!(start(&f), (107_892, "01:00:00;00".to_string()));
    // A label the new rate hasn't keeps its time: 29 frames at 29.97 are
    // 24 at 25.
    f.ok(req(
        "project.set",
        json!({"drop_frame": false, "start_tc": "00:00:00:29"}),
    ));
    f.ok(req("project.set", json!({"fps": "25"})));
    assert_eq!(start(&f), (24, "00:00:00:24".to_string()));
    // A timecode, not before zero.
    let e = f.err(req("project.set", json!({"start_tc": "-1s"})));
    assert_eq!(e.code, ErrorCode::Usage, "{e}");
}

#[test]
fn a_portrait_phone_clip_makes_a_portrait_project() {
    // Phones store portrait video sideways with a rotation: the project takes
    // the picture as shown.
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

#[test]
fn source_marks_drive_edits_without_a_range() {
    // §4.4 and decisions D5/D6: source marks are kept per clip; a missing In
    // means the first frame and a missing Out the end of the media.
    let mut f = fixture();
    let r = f.ok(req(
        "media.marks.set",
        json!({"media": "m1", "in": "20f", "out": "60f"}),
    ));
    assert_eq!(r.result, json!({"in": 20, "out": 60}));
    f.ok(req("edit.append", json!({"media": "m1"})));
    f.ok(req("media.marks.set", json!({"media": "m2", "in": 90})));
    f.ok(req("edit.append", json!({"media": "m2"})));
    // An explicit range wins over the marks.
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "0f..5f"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 40, 20),
            row("c2", 40, 10, 90),
            row("c3", 50, 5, 0)
        ]
    );
    // Source times resolve against the media: `end` is its end.
    f.ok(req("media.marks.set", json!({"media": "m1", "out": "end"})));
    let m1 = f.editor.project().media("m1").unwrap().marks.clone();
    assert_eq!((m1.mark_in, m1.mark_out), (Some(20), Some(250)));
    assert_eq!(
        f.ok(req("media.get", json!({"media": "m1"}))).result["marks"],
        json!({"in": 20, "out": 250})
    );
    f.ok(req("media.marks.clear", json!({"media": "m1"})));
    assert!(f.editor.project().media("m1").unwrap().marks.is_empty());
    // Marks stay inside the media and in order.
    for bad in [
        json!({"media": "m2", "in": 150}),
        json!({"media": "m2", "in": 50, "out": 40}),
        json!({"media": "m2", "out": "5s"}),
    ] {
        assert_eq!(
            f.err(req("media.marks.set", bad)).code,
            ErrorCode::InvalidEdit
        );
    }
    assert_eq!(
        f.err(req("media.marks.set", json!({"media": "m2"}))).code,
        ErrorCode::Usage
    );
}

fn tape_order() -> serde_json::Value {
    json!(["m1", "m2", "m3"])
}

#[test]
fn tape_marks_on_one_clip_mirror_source_marks() {
    // Decision D17: a span that stays on one clip is that clip's source marks.
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
fn tape_marks_across_two_clips_stay_on_the_tape() {
    // Decision D17: a span across clips is only tape marks, not one clip's marks.
    let mut f = fixture();
    f.ok(req(
        "tape.marks.set",
        json!({"in": 200, "out": 300, "order": tape_order()}),
    ));
    let p = f.editor.project();
    assert_eq!(p.tape_marks.mark_in, Some(200));
    assert_eq!(p.tape_marks.mark_out, Some(300));
    assert!(p.media("m1").unwrap().marks.is_empty());
    assert!(p.media("m2").unwrap().marks.is_empty());
    let rev = p.rev;
    f.ok(req("edit.append", json!({"tape": true})));
    assert_eq!(f.editor.project().rev, rev + 1, "one undo step");
    assert_eq!(f.v1(), vec![row("c1", 0, 50, 200), row("c2", 50, 50, 0)]);
}

#[test]
fn tape_marks_from_mid_clip_to_mid_clip() {
    let mut f = fixture();
    let r = f.ok(req(
        "tape.marks.set",
        json!({"in": 100, "out": 300, "order": tape_order()}),
    ));
    assert_eq!(
        r.result["pieces"],
        json!([
            {"media": "m1", "in": 100, "out": 250},
            {"media": "m2", "in": 0, "out": 50}
        ])
    );
    let rev = f.editor.project().rev;
    f.ok(req("edit.append", json!({"tape": true})));
    assert_eq!(f.editor.project().rev, rev + 1);
    assert_eq!(f.v1(), vec![row("c1", 0, 150, 100), row("c2", 150, 50, 0)]);
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

/// Tape marks over m1 200..250 and m2 0..50 (100 frames, two pieces) on
/// the three-clip timeline.
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
    // D17 with Place On Top: the span lines up once and goes
    // on one track, its pieces end to end. Piece by piece, the second piece
    // (at 102) would snap to Track 1's edit at 100 and land on V3.
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
    // D17 with Insert: later clips move by the whole span.
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
    // D17 with Ripple Overwrite: the span replaces c2, and what
    // follows moves by the length difference.
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
    // D17 with D7: both timeline marks make a three-point edit, so the span
    // is cut to the 70 marked frames: all of the first piece, 20 of the
    // second.
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
    // The marks count frames along a tape that held m3.
    let mut f = tape_span();
    f.ok(req("media.remove", json!({"media": "m3"})));
    assert!(f.editor.project().tape_marks.is_empty());
}

#[test]
fn copy_cut_and_paste() {
    // Ctrl+C copies, Ctrl+X cuts and ripples, Ctrl+V pastes at the
    // playhead on the original track, overwriting.
    let mut f = three_clips();
    f.ok(req(
        "clip.set",
        json!({"clips": "c2", "gain_db": -6.0, "name": "B"}),
    ));
    let rev = f.editor.project().rev;
    let copied = f.ok(req("edit.copy", json!({"clips": ["c3", "c2"]})));
    let clips = copied.result["clips"].clone();
    assert_eq!(clips[0]["id"], "c2", "copied clips are in timeline order");
    assert_eq!(f.editor.project().rev, rev, "copy changes nothing");

    // Paste at 20: the copies keep their spacing and properties and overwrite
    // c1's tail and c2's head (20..120); nothing moves.
    let r = f.ok(req("edit.paste", json!({"clips": clips, "at": 20})));
    assert_eq!(r.result["clips"], json!(["c4", "c7"]));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 20, 0),
            row("c4", 20, 50, 10),
            row("c7", 70, 50, 100),
            row("c8", 120, 30, 30),
            row("c3", 150, 50, 100),
        ]
    );
    let c4 = f.editor.project().clip("c4").unwrap();
    assert_eq!(c4.audio.as_ref().unwrap().gain_db, -6.0);
    assert_eq!(c4.name.as_deref(), Some("B"));

    // Cut ripples Track 1 and returns the clips.
    let r = f.ok(req("edit.cut", json!({"clips": "c4"})));
    assert_eq!(r.result["clips"][0]["id"], "c4");
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 20, 0),
            row("c7", 20, 50, 100),
            row("c8", 70, 30, 30),
            row("c3", 100, 50, 100),
        ]
    );
    // With Ripple On, a paste past the end starts at the end (like D10).
    let cut = r.result["clips"].clone();
    f.ok(req("edit.paste", json!({"clips": cut, "at": 500})));
    assert_eq!(f.v1().last().unwrap(), &row("c9", 150, 50, 10));

    // Headless paste needs clips and a position; pasted media must exist.
    assert_eq!(
        f.err(req("edit.paste", json!({"at": 0}))).code,
        ErrorCode::Usage
    );
    let mut alien = cut.clone();
    alien[0]["media"] = json!("m9");
    assert_eq!(
        f.err(req("edit.paste", json!({"clips": alien, "at": 0})))
            .code,
        ErrorCode::NotFound
    );
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
    // Spec §7.3: a drop places by position; the timeline In/Out are for the
    // edit buttons and keys.
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
    // c3 (150..200) was replaced by 30 frames; the marks were left alone.
    assert_eq!(f.v1().last().unwrap(), &row("c7", 150, 30, 0));
    let marks = &f.editor.project().marks;
    assert_eq!((marks.mark_in, marks.mark_out), (Some(10), Some(20)));
}

#[test]
fn markers_and_track_toggles() {
    // Edit a marker's name, color and note; track lock,
    // mute and video enable.
    let mut f = three_clips();
    let k = f.ok(req("markers.add", json!({"at": 30, "color": "red"})));
    let id = k.result["marker"].as_str().unwrap().to_string();
    f.ok(req(
        "markers.set",
        json!({"marker": id, "name": "laugh", "color": "mint", "note": "keep"}),
    ));
    let m = &f.editor.project().markers[0];
    assert_eq!(
        (m.name.as_str(), m.color.as_str(), m.note.as_str()),
        ("laugh", "mint", "keep")
    );
    assert_eq!(
        f.err(req("markers.set", json!({"marker": id, "color": "plaid"})))
            .code,
        ErrorCode::InvalidEdit
    );
    f.ok(req("track.set", json!({"track": "V1", "locked": true})));
    assert_eq!(
        f.err(req("edit.split", json!({"at": 30}))).code,
        ErrorCode::InvalidEdit,
        "a locked track can't be edited"
    );
    f.ok(req(
        "track.set",
        json!({"track": "V1", "locked": false, "video_enabled": false, "muted": true}),
    ));
    let plan = crate::render::flatten(f.editor.project(), 0, 200).unwrap();
    assert!(matches!(
        plan.video[..],
        [crate::render::VideoPiece::Black { frames: 200 }]
    ));
    assert_eq!(
        f.err(req("track.set", json!({"track": "A9", "muted": true})))
            .code,
        ErrorCode::NotFound
    );
}

#[test]
fn moving_onto_a_neighbor_overwrites_and_closes_the_gap() {
    // Dragging c2 (100..150) 10 frames right over c3's head, without
    // the pointer reaching c3, overwrites c3's first 10 frames; the 10 frames
    // c2 vacated close, so c2 stays put and c3 is 10 frames shorter.
    let mut f = three_clips();
    f.ok(req(
        "edit.move",
        json!({"clips": "c2", "to": 110, "mode": "overwrite"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c2", 100, 50, 10),
            row("c4", 150, 40, 110)
        ]
    );
    // Hovering over c1's middle and dropping c3 there splits c1; the
    // span c3 left at the end closes (nothing after it).
    let mut f = three_clips();
    f.ok(req(
        "edit.move",
        json!({"clips": "c3", "to": 40, "mode": "overwrite"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 40, 0),
            row("c3", 40, 50, 100),
            row("c5", 90, 10, 90),
            row("c2", 100, 50, 10)
        ]
    );
    // Moving c1 to the right of everything: c1's span closes, c1 goes last.
    let mut f = three_clips();
    f.ok(req(
        "edit.move",
        json!({"clips": "c1", "to": 400, "mode": "overwrite"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c2", 0, 50, 10),
            row("c3", 50, 50, 100),
            row("c1", 100, 100, 0)
        ]
    );
    // With Ripple Off the vacated span stays a gap.
    let mut f = three_clips();
    f.ok(req("project.set", json!({"ripple_on": false})));
    f.ok(req(
        "edit.move",
        json!({"clips": "c2", "to": 110, "mode": "overwrite"}),
    ));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 100, 0),
            row("c2", 110, 50, 10),
            row("c4", 160, 40, 110)
        ]
    );
}

#[test]
fn change_duration_to_the_end_of_the_media() {
    // End extends a clip to the end of its media.
    let mut f = three_clips();
    f.ok(req(
        "edit.change_duration",
        json!({"clips": "c2", "duration": "end"}),
    ));
    // m2 has 100 frames; c2 used 10..60, so it grows by 40 and c3 ripples.
    assert_eq!(f.v1()[1], row("c2", 100, 90, 10));
    assert_eq!(f.v1()[2], row("c3", 190, 50, 100));
}

/// An edit that changes nothing is no undo step and no new revision: the
/// next undo takes back the last real change.
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

/// `edited` says when the project last changed, for people (scripts compare
/// `rev`): every edit, undo and redo sets it; a request that fails or
/// changes nothing leaves it.
#[test]
fn changes_record_when_they_were_made() {
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    };
    let start = now();
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 100);
    let edited = f.editor.project().edited.expect("set by the edit");
    assert!((start..=now()).contains(&edited), "{edited}");
    std::thread::sleep(std::time::Duration::from_millis(2));
    f.ok(req("history.undo", json!({})));
    let undone = f.editor.project().edited.unwrap();
    assert!(undone > edited, "{undone} after {edited}");
    f.err(req("edit.split", json!({"at": 5000})));
    f.ok(req("project.set", json!({"ripple_on": true})));
    assert_eq!(f.editor.project().edited, Some(undone));
}
