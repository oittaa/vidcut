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
