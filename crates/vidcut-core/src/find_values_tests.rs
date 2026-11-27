use serde_json::{Value, json};

use crate::model::{FindValues, Subject};
use crate::motion::Join;
use crate::test_support::Fixture;

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn with_tracker() -> Fixture {
    let mut f = Fixture::new(&[(4.0, false)]);
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "0f..50f"}),
    ));
    f.ok(req("comp.add", json!({"clip": "c1", "tool": "tracker"})));
    f
}

fn kept(f: &Fixture) -> Option<FindValues> {
    crate::ops::motion::find_values(f.editor.project(), "c1", "Tracker1")
}

#[test]
fn a_value_of_the_request_goes_over_the_kept_value() {
    let asked = FindValues {
        start: Some(0.6),
        ..FindValues::default()
    };
    let kept = FindValues {
        subject: Some(Subject::Person),
        start: Some(0.8),
        keep: Some(0.3),
        ..FindValues::default()
    };
    let both = FindValues {
        subject: Some(Subject::Person),
        start: Some(0.6),
        keep: Some(0.3),
        ..FindValues::default()
    };
    assert_eq!(asked.over(kept), both);
}

#[test]
fn find_values_change_only_the_rules_that_they_give() {
    let values = FindValues {
        keep: Some(0.25),
        wait: Some(2.0),
        ..FindValues::default()
    };
    let rules = Join {
        start: 0.7,
        keep: 0.25,
        overlap: 0.2,
        wait_secs: 2.0,
        shortest_secs: 0.5,
        steady: 0.5,
    };
    assert_eq!(Join::of(Subject::Face).with(&values), rules);
}

#[test]
fn a_keep_over_the_usual_start_of_the_subject_is_a_problem() {
    let values = FindValues {
        subject: Some(Subject::Person),
        keep: Some(0.6),
        ..FindValues::default()
    };
    assert_eq!(values.problem().as_deref(), Some("keep is at most start"));
}

#[test]
fn a_size_out_of_range_is_a_problem() {
    let values = FindValues {
        size: Some(100),
        ..FindValues::default()
    };
    assert_eq!(
        values.problem().as_deref(),
        Some("size is from 320 to 3840 pixels")
    );
}

#[test]
fn a_tracker_keeps_its_find_values_until_undo() {
    let mut f = with_tracker();
    let set =
        json!({"clip": "c1", "node": "Tracker1", "find": {"subject": "person", "start": 0.6}});
    f.ok(req("comp.set", set));
    let values = FindValues {
        subject: Some(Subject::Person),
        start: Some(0.6),
        ..FindValues::default()
    };
    assert_eq!(kept(&f), Some(values));
    f.ok(req("history.undo", json!({})));
    assert_eq!(kept(&f), None);
}

#[test]
fn empty_find_values_are_the_usual_values() {
    let mut f = with_tracker();
    let set = json!({"clip": "c1", "node": "Tracker1", "find": {"wait": 2.0}});
    f.ok(req("comp.set", set));
    f.ok(req(
        "comp.set",
        json!({"clip": "c1", "node": "Tracker1", "find": {}}),
    ));
    assert_eq!(kept(&f), None);
}

#[test]
fn a_tracker_refuses_find_values_with_a_problem() {
    let mut f = with_tracker();
    let set = json!({"clip": "c1", "node": "Tracker1", "find": {"start": 0.3, "keep": 0.5}});
    let e = f.err(req("comp.set", set));
    assert_eq!(
        e.to_string(),
        "a tracker's find values: keep is at most start"
    );
}

#[test]
fn a_find_takes_the_subject_that_the_tracker_keeps() {
    let mut f = with_tracker();
    let set = json!({"clip": "c1", "node": "Tracker1", "find": {"subject": "person"}});
    f.ok(req("comp.set", set));
    let found = f.ok(req(
        "motion.find",
        json!({"clip": "c1", "node": "Tracker1"}),
    ));
    assert_eq!(found.result["subject"], json!("person"));
}

#[test]
fn a_subject_in_the_request_goes_over_the_kept_subject() {
    let mut f = with_tracker();
    let set = json!({"clip": "c1", "node": "Tracker1", "find": {"subject": "person"}});
    f.ok(req("comp.set", set));
    let find = json!({"clip": "c1", "node": "Tracker1", "subject": "text"});
    let found = f.ok(req("motion.find", find));
    assert_eq!(found.result["subject"], json!("text"));
}
