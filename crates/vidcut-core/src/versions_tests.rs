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
    let m = &history(&again)["marks"][0];
    assert_eq!(
        (m["step"].clone(), m["id"].clone(), m["name"].clone()),
        (json!(1), json!(1), json!("Before b"))
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

#[test]
fn reverting_a_move_keeps_a_clip_added_after_it() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 100);
    f.append("m1", 100, 200);
    f.ok(json!({ "method": "edit.move", "params": { "clips": "c1", "to": "200f" } }));
    f.append("m1", 0, 50);
    f.ok(json!({ "method": "history.revert", "params": { "step": 4 } }));
    let ids: Vec<String> = f.v1().into_iter().map(|r| r.0).collect();
    assert_eq!(ids, ["c1", "c2", "c3"]);
}

#[test]
fn reverting_a_ripple_a_later_move_changed_is_refused() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 50);
    f.append("m1", 50, 100);
    f.append("m1", 100, 150);
    f.ok(json!({ "method": "edit.delete", "params": { "clips": "c1" } }));
    f.ok(json!({ "method": "project.set", "params": { "ripple_on": false } }));
    f.ok(json!({ "method": "edit.move", "params": { "clips": "c3", "to": "300f", "mode": "overwrite" } }));
    let e = f.err(json!({ "method": "history.revert", "params": { "step": 5 } }));
    assert_eq!(e.code, crate::error::ErrorCode::Conflict);
}

#[test]
fn a_revert_that_no_longer_fits_names_the_step_in_its_way() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 50);
    f.append("m1", 50, 100);
    f.append("m1", 100, 150);
    f.ok(json!({ "method": "edit.delete", "params": { "clips": "c3", "lift": true } }));
    f.ok(json!({ "method": "edit.delete", "params": { "clips": "c1", "lift": true } }));
    f.ok(json!({ "method": "edit.delete", "params": { "clips": "c2", "lift": true } }));
    let e = f.err(json!({ "method": "history.revert", "params": { "step": 5 } }));
    assert_eq!(
        e.message,
        "can't revert step 5: step 6 (\u{201c}Delete 1.mp4 at 00:00:00:00 on V1, leaving a gap\u{201d}) \
         changed what is around it since"
    );
}

#[test]
fn reverting_with_no_steps_says_so() {
    let mut f = Fixture::new(&[]);
    let e = f.err(json!({ "method": "history.revert", "params": { "step": 1 } }));
    assert_eq!(e.message, "there are no steps to revert");
}

#[test]
fn restoring_a_version_inside_another_brings_both_back() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a", "b", "c"]);
    run(&s, "history.undo", json!({ "steps": 1 }));
    bins(&s, &["d"]);
    run(&s, "history.undo", json!({ "steps": 2 }));
    bins(&s, &["e"]);
    run(&s, "history.restore", json!({ "branch": 3 }));
    assert_eq!(s.load().unwrap().project().bins, ["a", "b", "c"]);
}

#[test]
fn since_a_step_counts_the_bins_added() {
    let mut f = Fixture::new(&[]);
    f.ok(json!({ "method": "bins.add", "params": { "path": "a" } }));
    f.ok(json!({ "method": "bins.add", "params": { "path": "b" } }));
    let r = f.ok(json!({ "method": "history.get", "params": { "since": 1 } }));
    assert_eq!(r.result["since"]["text"], "1 bin added");
}

#[test]
fn since_a_step_says_the_in_and_out_marks_changed() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 50);
    f.ok(json!({ "method": "marks.set", "params": { "in": "10f", "out": "20f" } }));
    let r = f.ok(json!({ "method": "history.get", "params": { "since": 2 } }));
    assert_eq!(r.result["since"]["text"], "In and Out marks changed");
}

#[test]
fn since_a_step_counts_a_renamed_media_file_as_changed() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.ok(json!({ "method": "media.rename", "params": { "media": "m1", "name": "Take 2" } }));
    let r = f.ok(json!({ "method": "history.get", "params": { "since": 1 } }));
    assert_eq!(r.result["since"]["text"], "1 media file changed");
}

#[test]
fn since_a_step_to_redo_says_what_redoing_brings() {
    let mut f = Fixture::new(&[]);
    f.ok(json!({ "method": "bins.add", "params": { "path": "a" } }));
    f.ok(json!({ "method": "history.undo", "params": {} }));
    let r = f.ok(json!({ "method": "history.get", "params": { "since": 1 } }));
    let s = &r.result["since"];
    assert_eq!(
        (s["text"].clone(), s["redo"].clone()),
        (json!("1 bin added"), json!(true))
    );
}

#[test]
fn restoring_a_version_inside_another_keeps_what_it_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a", "b", "c"]);
    run(&s, "history.undo", json!({ "steps": 1 }));
    bins(&s, &["d"]);
    run(&s, "history.undo", json!({ "steps": 2 }));
    bins(&s, &["e"]);
    run(&s, "history.restore", json!({ "branch": 3 }));
    let mut kept: Vec<Vec<String>> = history(&s)["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| labels(&b["steps"]))
        .collect();
    kept.sort();
    assert_eq!(
        kept,
        [vec!["New bin d".to_string()], vec!["New bin e".to_string()]]
    );
}

#[test]
fn since_a_step_of_a_version_says_what_going_there_brings() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    bins(&s, &["a", "b"]);
    run(&s, "history.undo", json!({ "steps": 1 }));
    bins(&s, &["c"]);
    let r = run(&s, "history.get", json!({ "since": 1, "branch": 2 }));
    assert_eq!(r["since"]["text"], "1 bin added, 1 deleted");
}
