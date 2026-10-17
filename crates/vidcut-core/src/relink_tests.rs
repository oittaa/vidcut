use std::path::PathBuf;

use serde_json::{Value, json};

use crate::test_support::{Fixture, media_info};
use crate::time::Rational;

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

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

#[test]
fn files_named_alike_pick_the_folder_named_like_the_old_one() {
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
