use serde_json::{Value, json};

use crate::model::Project;
use crate::test_support::Fixture;

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn state(f: &Fixture) -> Project {
    let mut p = f.editor.project().clone();
    p.rev = 0;
    p.edited = None;
    p
}

#[test]
fn undo_and_redo_go_back_through_every_state() {
    let mut f = Fixture::new(&[(10.0, true), (10.0, true), (8.0, false)]);
    let edits = [
        req(
            "project.set",
            json!({"ripple_on": true, "still_duration": "3s"}),
        ),
        req("bins.add", json!({"path": "Interviews"})),
        req("media.move", json!({"media": "m2", "bin": "Interviews"})),
        req("media.set", json!({"media": "m1", "color": "teal"})),
        req("edit.append", json!({"media": "m1", "range": "0f..50f"})),
        req("edit.append", json!({"media": "m2", "range": "25f..75f"})),
        req("edit.append", json!({"media": "m3", "range": "0f..40f"})),
        req("edit.split", json!({"at": "20f"})),
        req(
            "edit.insert",
            json!({"media": "m2", "range": "0f..10f", "at": "60f"}),
        ),
        req(
            "edit.trim",
            json!({"clip": "c2", "edge": "tail", "by": "-5f"}),
        ),
        req(
            "markers.add",
            json!({"at": "30f", "name": "look", "color": "rose"}),
        ),
        req(
            "clip.set",
            json!({"clips": "c1", "gain_db": -3.0, "color": "pink"}),
        ),
        req("edit.change_speed", json!({"clips": "c3", "speed": 2.0})),
        req("edit.delete", json!({"clips": "c1"})),
        req(
            "track.set",
            json!({"track": "V1", "name": "Picture", "locked": true}),
        ),
    ];
    let mut states = vec![state(&f)];
    for edit in edits {
        f.ok(edit);
        states.push(state(&f));
    }
    for (i, want) in states.iter().enumerate().rev().skip(1) {
        f.ok(req("history.undo", json!({})));
        assert_eq!(&state(&f), want, "undone to state {i}");
    }
    for (i, want) in states.iter().enumerate().skip(1) {
        f.ok(req("history.redo", json!({})));
        assert_eq!(&state(&f), want, "redone to state {i}");
    }
}

fn trimmed() -> Fixture {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 50);
    f.append("m1", 100, 150);
    f
}

fn steps(f: &Fixture) -> crate::steps::Steps {
    f.editor
        .history_steps(&mut crate::steps::StepsCache::default())
        .unwrap()
}

#[test]
fn a_trims_label_names_the_clip_the_amount_and_the_place() {
    let mut f = trimmed();
    f.ok(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "tail", "by": -12}),
    ));
    assert_eq!(
        steps(&f).undo().unwrap().label,
        "Trim the end of 1.mp4 by -12 frames at 00:00:02:00 on V1"
    );
}

#[test]
fn a_split_names_the_clip_it_cuts() {
    let mut f = trimmed();
    f.ok(req("edit.split", json!({"at": 60})));
    assert_eq!(
        steps(&f).undo().unwrap().label,
        "Split 1.mp4 at 00:00:02:10 on V1"
    );
}

#[test]
fn a_split_at_a_timecode_names_the_clip_it_cuts() {
    let mut f = trimmed();
    f.ok(req("edit.split", json!({"at": "00:00:02:10"})));
    assert_eq!(
        steps(&f).undo().unwrap().label,
        "Split 1.mp4 at 00:00:02:10 on V1"
    );
}

#[test]
fn a_split_at_the_playhead_names_the_clip_and_the_time() {
    let mut f = trimmed();
    let split = crate::request::Envelope::from_json(req("edit.split", json!({"at": "playhead"})));
    f.editor.apply(split.unwrap(), &f.env, Some(60)).unwrap();
    assert_eq!(
        steps(&f).undo().unwrap().label,
        "Split 1.mp4 at 00:00:02:10 on V1"
    );
}

#[test]
fn a_ripple_delete_takes_place_where_the_clip_was() {
    let mut f = trimmed();
    f.ok(req("edit.delete", json!({"clips": "c1"})));
    let step = steps(&f).undo().cloned().unwrap();
    assert_eq!(
        serde_json::to_value(&step.spans).unwrap(),
        json!([{"track": "V1", "from": 0, "to": 50}])
    );
}

#[test]
fn a_moved_clip_is_where_its_step_happened() {
    let mut f = trimmed();
    f.ok(req(
        "edit.move",
        json!({"clips": "c2", "to": 200, "mode": "overwrite"}),
    ));
    assert_eq!(steps(&f).undo().unwrap().clips, ["c2"]);
}

#[test]
fn a_step_records_who_made_it() {
    let mut f = trimmed();
    f.ok(json!({
        "method": "bins.add",
        "params": {"path": "a"},
        "client": {"kind": "mcp", "name": "agent"},
    }));
    assert_eq!(
        serde_json::to_value(&steps(&f).undo().unwrap().by).unwrap(),
        json!({"kind": "mcp", "name": "agent"})
    );
}

#[test]
fn an_apply_lists_its_commands() {
    let mut f = trimmed();
    f.ok(req(
        "apply",
        json!({"commands": [
            {"method": "bins.add", "params": {"path": "a"}},
            {"method": "bins.add", "params": {"path": "b"}},
        ]}),
    ));
    let step = steps(&f).undo().cloned().unwrap();
    assert_eq!(
        (step.label, step.parts),
        (
            "New bin a and 1 more".to_string(),
            vec!["New bin a".to_string(), "New bin b".into()]
        )
    );
}

#[test]
fn going_to_a_step_undoes_back_to_it() {
    let mut f = trimmed();
    f.ok(req("bins.add", json!({"path": "a"})));
    f.ok(req("bins.add", json!({"path": "b"})));
    let r = f.ok(req("history.go", json!({"step": 3})));
    assert_eq!(r.result["undone"], json!(["New bin b", "New bin a"]));
}

#[test]
fn going_to_a_step_past_the_history_fails() {
    let mut f = trimmed();
    let e = f.err(req("history.go", json!({"step": 9})));
    assert_eq!(e.message, "no step 9: the undo history has 3");
}

#[test]
fn the_project_at_an_undone_step_is_as_it_was() {
    let mut f = trimmed();
    f.ok(req("edit.delete", json!({"clips": "c1"})));
    let before = f.editor.project_at(3).unwrap();
    assert_eq!(before.track("V1").unwrap().clips.len(), 2);
}

#[test]
fn history_get_numbers_the_steps_and_says_which_are_done() {
    let mut f = trimmed();
    f.ok(req("history.undo", json!({})));
    let r = f.ok(req("history.get", json!({"from": 2})));
    let listed: Vec<(Value, Value)> = r.result["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["step"].clone(), s["done"].clone()))
        .collect();
    assert_eq!(listed, [(json!(2), json!(true)), (json!(3), json!(false))]);
}

#[test]
fn going_to_a_step_cannot_be_batched() {
    let mut f = trimmed();
    let e = f.err(req(
        "apply",
        json!({"commands": [{"method": "history.go", "params": {"step": 1}}]}),
    ));
    assert_eq!(e.message, "entry 1: history.go cannot be batched");
}

#[test]
fn an_appended_range_is_named_in_timecode() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "1s..3s"}),
    ));
    assert_eq!(
        steps(&f).undo().unwrap().label,
        "Append 1.mp4 00:00:01:00\u{2013}00:00:03:00"
    );
}

#[test]
fn a_time_in_seconds_is_named_in_timecode() {
    let mut f = trimmed();
    f.ok(req("markers.add", json!({"at": "2s"})));
    assert_eq!(steps(&f).undo().unwrap().label, "Marker at 00:00:02:00");
}

#[test]
fn an_applys_parts_leave_out_its_queries() {
    let mut f = trimmed();
    f.ok(req(
        "apply",
        json!({"commands": [
            {"method": "project.get", "params": {}},
            {"method": "bins.add", "params": {"path": "a"}},
        ]}),
    ));
    assert_eq!(steps(&f).undo().unwrap().parts, ["New bin a"]);
}
