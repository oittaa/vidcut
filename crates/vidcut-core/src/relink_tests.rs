//! Relinking (deviation X1): missing media found by name in folders, each
//! file found checked against what the project knows of it.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::test_support::{Fixture, media_info};
use crate::time::Rational;

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

/// Moves the fake file at `from` to `to`.
fn mv(f: &mut Fixture, from: &str, to: &str) {
    let (from, to) = (PathBuf::from(from), PathBuf::from(to));
    let info = f.env.files.remove(&from).unwrap();
    f.env.files.insert(to.clone(), info);
    if let Some(v) = f.env.fingerprints.remove(&from) {
        f.env.fingerprints.insert(to.clone(), v);
    }
    if let Some(v) = f.env.sizes.remove(&from) {
        f.env.sizes.insert(to, v);
    }
}

fn path_of(f: &Fixture, id: &str) -> String {
    f.editor
        .project()
        .media(id)
        .unwrap()
        .path
        .display()
        .to_string()
}

#[test]
fn a_folder_search_relinks_the_missing_media_that_match() {
    let mut f = Fixture::new(&[(4.0, true), (4.0, true), (4.0, true), (4.0, true)]);
    // m1 moved as it was, m2 into a subfolder; m3 moved and changed; m4
    // is gone.
    mv(&mut f, "/media/1.mp4", "/moved/1.mp4");
    mv(&mut f, "/media/2.mp4", "/moved/day 2/2.mp4");
    mv(&mut f, "/media/3.mp4", "/moved/3.mp4");
    f.env.sizes.insert(PathBuf::from("/moved/3.mp4"), 7);
    f.env.files.remove(&PathBuf::from("/media/4.mp4"));
    let listed = f.ok(req("media.list", json!({}))).result;
    let missing: Vec<&Value> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|m| &m["missing"])
        .collect();
    assert_eq!(missing, [&json!(true); 4]);

    let undo_before = f.editor.history().undo.len();
    let r = f.ok(req("media.relink", json!({ "search": ["/moved"] })));
    assert_eq!(
        r.result["relinked"],
        json!([
            {"media": "m1", "path": "/moved/1.mp4", "same": true},
            {"media": "m2", "path": "/moved/day 2/2.mp4", "same": true},
        ])
    );
    assert_eq!(
        r.result["different"],
        json!([{"media": "m3", "path": "/moved/3.mp4", "why": "7 bytes, not 1"}])
    );
    assert_eq!(
        r.result["not_found"],
        json!([{"media": "m4", "name": "4.mp4"}])
    );
    assert_eq!(path_of(&f, "m1"), "/moved/1.mp4");
    assert_eq!(
        path_of(&f, "m3"),
        "/media/3.mp4",
        "a different file stays out"
    );
    assert_eq!(
        f.editor.history().undo.len(),
        undo_before + 1,
        "one undo step"
    );
    let listed = f.ok(req("media.list", json!({}))).result;
    assert!(listed[0].get("missing").is_none());
    assert_eq!(listed[2]["missing"], true);

    // Meant anyway: a file given by path is relinked, and said to differ.
    let r = f.ok(req(
        "media.relink",
        json!({ "media": "m3", "path": "/moved/3.mp4" }),
    ));
    assert_eq!(
        r.result["relinked"],
        json!([{"media": "m3", "path": "/moved/3.mp4", "same": false, "why": "7 bytes, not 1"}])
    );
    assert_eq!(path_of(&f, "m3"), "/moved/3.mp4");
}

#[test]
fn fingerprints_tell_files_of_one_size_apart() {
    // Two recordings alike in size and streams: only the fingerprint
    // differs.
    let mut f = Fixture::new(&[]);
    let fps = Rational::new(25, 1).unwrap();
    for (path, fp) in [("/media/take.wav", "aa"), ("/other/take.wav", "bb")] {
        f.env
            .files
            .insert(PathBuf::from(path), media_info(4.0, fps, true));
        f.env.fingerprints.insert(PathBuf::from(path), fp.into());
    }
    f.ok(req("media.add", json!({ "paths": ["/media/take.wav"] })));
    assert_eq!(
        f.editor.project().media[0].fingerprint.as_deref(),
        Some("aa")
    );
    f.env.files.remove(&PathBuf::from("/media/take.wav"));
    let r = f.ok(req("media.relink", json!({ "search": ["/other"] })));
    assert_eq!(
        r.result["different"][0]["why"],
        "other content (fingerprint)"
    );
    // The same content found: relinked.
    f.env
        .fingerprints
        .insert(PathBuf::from("/other/take.wav"), "aa".into());
    let r = f.ok(req("media.relink", json!({ "search": ["/other"] })));
    assert_eq!(r.result["relinked"][0]["path"], "/other/take.wav");
}

#[test]
fn files_named_alike_pick_the_folder_named_like_the_old_one() {
    // Every camera card has a C0001.MP4.
    let mut f = Fixture::new(&[]);
    let fps = Rational::new(25, 1).unwrap();
    f.env
        .files
        .insert("/shoot/B/C0001.MP4".into(), media_info(4.0, fps, true));
    f.ok(req("media.add", json!({ "paths": ["/shoot/B/C0001.MP4"] })));
    mv(&mut f, "/shoot/B/C0001.MP4", "/disk/shoot/B/C0001.MP4");
    f.env
        .files
        .insert("/disk/shoot/A/C0001.MP4".into(), media_info(4.0, fps, true));
    let r = f.ok(req("media.relink", json!({ "search": ["/disk"] })));
    assert_eq!(r.result["relinked"][0]["path"], "/disk/shoot/B/C0001.MP4");
}

#[test]
fn the_same_file_keeps_its_proxy_and_a_dry_run_changes_nothing() {
    let mut f = Fixture::new(&[(4.0, true)]);
    f.ok(req("proxy.generate", json!({ "media": "m1" })));
    mv(&mut f, "/media/1.mp4", "/moved/1.mp4");
    let rev = f.editor.project().rev;
    let r = f.ok(json!({
        "method": "media.relink", "dry_run": true, "params": { "search": ["/moved"] }
    }));
    assert_eq!(r.result["relinked"][0]["media"], "m1");
    assert_eq!(f.editor.project().rev, rev);
    assert_eq!(path_of(&f, "m1"), "/media/1.mp4");
    f.ok(req("media.relink", json!({ "search": ["/moved"] })));
    let m = &f.editor.project().media[0];
    assert!(
        m.fresh_proxy().is_some(),
        "the proxy was made from this file"
    );
}

#[test]
fn a_path_takes_one_item_and_search_or_path_is_needed() {
    let mut f = Fixture::new(&[(4.0, true), (4.0, true)]);
    let e = f.err(req(
        "media.relink",
        json!({ "media": ["m1", "m2"], "path": "/media/1.mp4" }),
    ));
    assert!(e.message.contains("one media id"), "{}", e.message);
    let e = f.err(req("media.relink", json!({ "media": "m1" })));
    assert!(e.message.contains("either"), "{}", e.message);
    // Nothing missing, nothing to do.
    let r = f.ok(req("media.relink", json!({ "search": ["/media"] })));
    assert_eq!(
        r.result,
        json!({ "relinked": [], "different": [], "not_found": [] })
    );
}
