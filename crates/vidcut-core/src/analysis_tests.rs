use std::path::PathBuf;

use serde_json::{Value, json};

use crate::error::ErrorCode;
use crate::test_support::{Fixture, levels, pictures};

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn fixture() -> Fixture {
    let mut f = Fixture::new(&[(10.0, true), (4.0, false)]);
    let m1 = PathBuf::from("/media/1.mp4");
    f.env.levels.insert(
        m1.clone(),
        levels(10.0, &[(1.0, 1.5), (3.0, 3.2), (6.0, 7.0)]),
    );
    f.env
        .pictures
        .insert(m1, pictures(10.0, 25.0, &[2.0, 5.0], &[(8.0, 9.0)]));
    f
}

#[test]
fn ripple_delete_silence_honors_the_threshold_and_minimum() {
    let mut f = fixture();
    f.append("m1", 0, 250);
    let r = f.ok(req(
        "edit.ripple_delete_silence",
        json!({"clips": "c1", "threshold_db": -70}),
    ));
    assert_eq!(r.result["frames"], 0);
    assert_eq!(f.v1().len(), 1);
    let r = f.ok(req(
        "edit.ripple_delete_silence",
        json!({"clips": "c1", "min": "3s", "pre_head": 0, "post_tail": 0}),
    ));
    let removed = r.result["removed"].as_array().unwrap();
    assert_eq!(removed.len(), 1, "{removed:?}");
    assert_eq!(
        (&removed[0]["start"], &removed[0]["end"]),
        (&json!(175), &json!(250))
    );
}

#[test]
fn ripple_delete_silence_needs_sound() {
    let mut f = fixture();
    f.append("m2", 0, 50);
    let e = f.err(req("edit.ripple_delete_silence", json!({"clips": "c1"})));
    assert_eq!(e.code, ErrorCode::InvalidEdit);
}

#[test]
fn ripple_delete_silence_ripples_even_with_ripple_off() {
    let mut f = fixture();
    f.append("m1", 0, 250);
    f.ok(req("project.set", json!({"ripple_on": false})));
    f.ok(req(
        "edit.ripple_delete_silence",
        json!({"clips": "c1", "pre_head": "5f", "post_tail": "5f", "min": "0.7s"}),
    ));
    assert_eq!(f.editor.project().end(), 73);
}

#[test]
fn detect_scene_cuts_splits_where_shots_change() {
    let mut f = fixture();
    f.append("m1", 25, 250);
    let r = f.ok(req("edit.detect_scene_cuts", json!({"clips": "c1"})));
    let frames: Vec<i64> = r.result["cuts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["frame"].as_i64().unwrap())
        .collect();
    assert_eq!(frames, vec![25, 100]);
    let starts: Vec<(i64, i64)> = f.v1().iter().map(|c| (c.1, c.3)).collect();
    assert_eq!(starts, vec![(0, 25), (25, 50), (100, 125)]);
    let mut g = fixture();
    g.append("m1", 0, 250);
    let r = g.ok(req(
        "edit.detect_scene_cuts",
        json!({"clips": "c1", "threshold": 90}),
    ));
    assert_eq!(r.result["cuts"], json!([]));
    assert_eq!(g.v1().len(), 1);
}

#[test]
fn loudness_measures_the_mix() {
    let mut f = fixture();
    f.append("m1", 0, 100);
    f.ok(req("clip.set", json!({"clips": "c1", "gain_db": 6.0})));
    let r = f.ok(req("analyze.loudness", json!({})));
    assert_eq!(r.result["integrated"], -14.0);
    assert_eq!(
        (&r.result["from"], &r.result["to"]),
        (&json!(0), &json!(100))
    );
    let r = f.ok(req("analyze.loudness", json!({"range": "10f..20f"})));
    assert_eq!(r.result["from"], 10);
}

#[test]
fn the_boring_detector_flags_long_and_short_clips() {
    let mut f = Fixture::new(&[(60.0, true)]);
    f.append("m1", 0, 900);
    f.append("m1", 900, 905);
    f.append("m1", 905, 1000);
    let r = f.ok(req("timeline.boring", json!({})));
    assert_eq!(r.result["boring"][0]["clip"], "c1", "36 s > 30 s");
    assert_eq!(r.result["jump"][0]["clip"], "c2", "5 frames < 10");
    assert_eq!(r.result["boring"].as_array().unwrap().len(), 1);
    let r = f.ok(req(
        "timeline.boring",
        json!({"boring": "2s", "jump": "2f"}),
    ));
    assert_eq!(r.result["boring"].as_array().unwrap().len(), 2);
    assert_eq!(r.result["jump"], json!([]));
}
