//! Tests of the edits and queries built on media analysis: Ripple
//! Delete Silence, Detect Scene Cuts, the analysis queries, the Boring
//! Detector and slides. The fake media layer reports made-up levels and
//! pictures. Media: m1 = 250 frames with audio, m2 = 100 frames without
//! audio (25 fps).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::error::ErrorCode;
use crate::test_support::{Fixture, levels, pictures, row};

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

/// m1 talks at 1.0–1.5 s, 3.0–3.2 s and 6.0–7.0 s; its shots change at
/// 2 s and 5 s; it is black from 8 s to 9 s.
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
fn ripple_delete_silence_cuts_out_the_silences() {
    // The red stripes are cut out and the rest closes up.
    let mut f = fixture();
    f.append("m1", 0, 250);
    let params = json!({"clips": "c1", "pre_head": "5f", "post_tail": "5f", "min": "0.7s"});
    // A dry run only reports the parts: leading silence up to 5 frames before
    // the sound, 5 frames kept after and before each sound, trailing silence.
    let parts = |r: &crate::Response| -> Vec<(i64, i64)> {
        r.result["removed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["start"].as_i64().unwrap(), p["end"].as_i64().unwrap()))
            .collect()
    };
    let mut dry_params = params.clone();
    dry_params["dry_run"] = json!(true);
    let dry = f.ok(req("edit.ripple_delete_silence", dry_params));
    assert_eq!(parts(&dry), vec![(0, 20), (43, 70), (85, 145), (180, 250)]);
    assert_eq!(f.v1().len(), 1, "a dry run changes nothing");
    let undo_before = f.editor.history().undo.len();
    let r = f.ok(req("edit.ripple_delete_silence", params));
    assert_eq!(parts(&r), parts(&dry));
    assert_eq!(r.result["frames"], 177);
    let kept: Vec<(i64, i64, i64)> = f.v1().iter().map(|c| (c.1, c.2, c.3)).collect();
    assert_eq!(kept, vec![(0, 23, 20), (23, 15, 70), (38, 35, 145)]);
    assert_eq!(
        f.editor.history().undo.len(),
        undo_before + 1,
        "one undo step"
    );
    f.ok(req("history.undo", json!({})));
    assert_eq!(f.v1(), vec![row("c1", 0, 250, 0)]);
}

#[test]
fn ripple_delete_silence_honors_the_threshold_and_minimum() {
    let mut f = fixture();
    f.append("m1", 0, 250);
    // Nothing is quiet enough below -70 dBFS (the room tone is -60 dBFS).
    let r = f.ok(req(
        "edit.ripple_delete_silence",
        json!({"clips": "c1", "threshold_db": -70}),
    ));
    assert_eq!(r.result["frames"], 0);
    assert_eq!(f.v1().len(), 1);
    // A 3 s minimum leaves the 1.5 s gap and the 1 s lead-in.
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
    // The name says so: unlike Delete, it closes the gaps regardless.
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
    // Clips cut back into their shots.
    let mut f = fixture();
    f.append("m1", 25, 250);
    let r = f.ok(req("edit.detect_scene_cuts", json!({"clips": "c1"})));
    let frames: Vec<i64> = r.result["cuts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["frame"].as_i64().unwrap())
        .collect();
    // Shots at source 2 s and 5 s, the clip starting 1 s in.
    assert_eq!(frames, vec![25, 100]);
    let starts: Vec<(i64, i64)> = f.v1().iter().map(|c| (c.1, c.3)).collect();
    assert_eq!(starts, vec![(0, 25), (25, 50), (100, 125)]);
    // A high threshold finds nothing.
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
fn analysis_queries_report_media_and_clips() {
    let mut f = fixture();
    f.append("m1", 50, 250);
    // Media positions are source frames; clip positions timeline frames.
    let r = f.ok(req("analyze.silence", json!({"media": "m1", "min": "2s"})));
    let ranges = &r.result[0]["ranges"];
    assert_eq!(r.result[0]["media"], "m1");
    assert_eq!(
        (&ranges[0]["start"], &ranges[0]["end"], &ranges[1]["start"]),
        (&json!(80), &json!(150), &json!(175))
    );
    let r = f.ok(req("analyze.silence", json!({"clips": "c1", "min": "2s"})));
    assert_eq!(r.result[0]["clip"], "c1");
    assert_eq!(r.result[0]["track"], "V1");
    assert_eq!(r.result[0]["ranges"][0]["start"], 30, "source 80 - 50");
    // Every clip with pictures, by default.
    let r = f.ok(req("analyze.scenes", json!({})));
    let cuts: Vec<i64> = r.result[0]["cuts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["frame"].as_i64().unwrap())
        .collect();
    assert_eq!(
        cuts,
        vec![75],
        "the shot at 5 s; the one at 2 s is before the clip"
    );
    let r = f.ok(req("analyze.black", json!({"media": "m1"})));
    assert_eq!(r.result[0]["ranges"][0]["start"], 200);
    assert_eq!(r.result[0]["ranges"][0]["end"], 225);
    let e = f.err(req("analyze.scenes", json!({"media": "m1", "clips": "c1"})));
    assert_eq!(e.code, ErrorCode::Usage);
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
    // Longer than "boring clips" or shorter than "jump cuts".
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

#[test]
fn a_slide_moves_a_clip_between_its_neighbors() {
    // The clip keeps its media; the neighbors give and take.
    let mut f = fixture();
    f.append("m1", 0, 50);
    f.append("m1", 100, 150);
    f.append("m1", 200, 250);
    // B-roll over c2's frame 120 follows it.
    let b = f.ok(req(
        "edit.overwrite",
        json!({"media": "m2", "range": "0f..10f", "at": 70, "track": "V2"}),
    ));
    let broll = b.result["clip"].as_str().unwrap().to_string();
    f.ok(req("edit.slide", json!({"clip": "c2", "by": 10})));
    assert_eq!(
        f.v1(),
        vec![
            row("c1", 0, 60, 0),
            row("c2", 60, 50, 100),
            row("c3", 110, 40, 210)
        ]
    );
    assert_eq!(f.track("V2"), vec![row(&broll, 80, 10, 0)]);
    assert_eq!(f.editor.project().end(), 150, "the program length stays");
    // The first clip has nothing before it; media runs out.
    let e = f.err(req("edit.slide", json!({"clip": "c1", "by": 5})));
    assert_eq!(e.code, ErrorCode::InvalidEdit);
    let e = f.err(req("edit.slide", json!({"clip": "c2", "by": -70})));
    assert_eq!(e.code, ErrorCode::InvalidEdit);
}
