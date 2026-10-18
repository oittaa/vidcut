use std::path::Path;

use serde_json::{Value, json};

use crate::model::{Project, Settings};
use crate::request::Envelope;
use crate::store::Store;
use crate::test_support::{FakeEnv, Fixture, row};

fn store(dir: &Path) -> Store {
    let path = dir.join("p.vidcut");
    let project = Project::new(Settings {
        auto_format: false,
        ..Settings::default()
    });
    std::fs::write(&path, serde_json::to_string(&project).unwrap()).unwrap();
    Store::open_in(&path, &dir.join("state")).unwrap()
}

fn run(store: &Store, method: &str, params: Value) -> Value {
    let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
    let (r, _) = store
        .transact(|ed| ed.apply(e, &FakeEnv::default(), None))
        .unwrap_or_else(|e| panic!("{method}: {e}"));
    r.result
}

fn bins(store: &Store, names: &[&str]) {
    for n in names {
        run(store, "bins.add", json!({ "path": n }));
    }
}

fn history(store: &Store) -> Value {
    run(store, "history.get", json!({}))
}

fn labels(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s["label"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_change_after_an_undo_keeps_the_undone_steps_as_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a", "b", "c"]);
    run(&s, "history.undo", json!({ "steps": 2 }));
    bins(&s, &["d"]);
    let b = &history(&s)["branches"][0];
    assert_eq!(
        (b["after"].clone(), labels(&b["steps"])),
        (json!(1), vec!["New bin b".to_string(), "New bin c".into()])
    );
}

#[test]
fn restoring_a_branch_brings_its_steps_back() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a", "b", "c"]);
    run(&s, "history.undo", json!({ "steps": 2 }));
    bins(&s, &["d"]);
    let id = history(&s)["branches"][0]["id"].clone();
    run(&s, "history.restore", json!({ "branch": id }));
    assert_eq!(s.load().unwrap().project().bins, ["a", "b", "c"]);
}

#[test]
fn restoring_a_branch_keeps_what_it_replaces_as_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a", "b", "c"]);
    run(&s, "history.undo", json!({ "steps": 2 }));
    bins(&s, &["d"]);
    let id = history(&s)["branches"][0]["id"].clone();
    run(&s, "history.restore", json!({ "branch": id }));
    let h = history(&s);
    assert_eq!(
        (
            labels(&h["branches"][0]["steps"]),
            h["branches"].as_array().unwrap().len()
        ),
        (vec!["New bin d".to_string()], 1)
    );
}

#[test]
fn a_named_step_keeps_its_name_in_a_later_run() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a", "b"]);
    run(&s, "history.mark", json!({ "step": 1, "name": "Before b" }));
    let again = Store::open_in(s.path(), &dir.path().join("state")).unwrap();
    assert_eq!(
        history(&again)["marks"],
        json!([{ "step": 1, "id": 1, "name": "Before b", "time": history(&again)["marks"][0]["time"] }])
    );
}

#[test]
fn a_name_is_not_a_project_change() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a"]);
    let rev = s.load().unwrap().project().rev;
    run(&s, "history.mark", json!({ "name": "First" }));
    assert_eq!(s.load().unwrap().project().rev, rev);
}

#[test]
fn reverting_a_step_undoes_it_alone() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 50);
    f.append("m1", 100, 150);
    f.ok(json!({ "method": "edit.trim", "params": { "clip": "c1", "edge": "tail", "by": -10 } }));
    f.ok(json!({ "method": "markers.add", "params": { "at": 5 } }));
    f.ok(json!({ "method": "history.revert", "params": { "step": 4 } }));
    assert_eq!(
        (f.v1(), f.editor.project().markers.len()),
        (vec![row("c1", 0, 50, 0), row("c2", 50, 50, 100)], 1)
    );
}

#[test]
fn reverting_a_step_a_later_one_changed_is_refused() {
    let mut f = Fixture::new(&[]);
    f.ok(json!({ "method": "bins.add", "params": { "path": "a" } }));
    f.ok(json!({ "method": "bins.rename", "params": { "path": "a", "to": "x" } }));
    let e = f.err(json!({ "method": "history.revert", "params": { "step": 1 } }));
    assert_eq!(
        e.message,
        "can't revert step 1: step 2 (\u{201c}Rename bin a to x\u{201d}) changed bins since"
    );
}

#[test]
fn since_a_step_says_what_changed() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 50);
    f.append("m1", 100, 150);
    f.ok(json!({ "method": "edit.delete", "params": { "clips": "c1" } }));
    let r = f.ok(json!({ "method": "history.get", "params": { "since": 3 } }));
    assert_eq!(r.result["since"]["text"], "1 clip deleted, 1 moved");
}
