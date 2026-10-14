//! OTIO interchange tests (deviation X9).

use super::*;
use crate::model::{AudioInfo, Effects, MediaInfo};
use crate::test_support::{FakeEnv, Fixture, media_info};

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

/// A file as editors write one: `Clip.1`, source ranges in the media's
/// timecode relative to its available range, float rates, a transition,
/// music on the audio track.
fn editor_style() -> Value {
    let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": v});
    let r = |s: f64, d: f64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": t(s), "duration": t(d)});
    let reference = |file: &str, available: Option<Value>| {
        let mut v = json!({"OTIO_SCHEMA": "ExternalReference.1", "target_url": format!("file:///media/{file}")});
        if let Some(a) = available {
            v["available_range"] = a;
        }
        v
    };
    let clip = |name: &str, range: Value, reference: Value| json!({"OTIO_SCHEMA": "Clip.1", "name": name, "source_range": range, "media_reference": reference});
    let mut a002 = clip("A002", r(10.0, 24.0), reference("A002.mov", None));
    a002["enabled"] = json!(false);
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": "Cut",
        "global_start_time": t(86400.0),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "children": [
                {"OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "Video 1", "children": [
                    clip("A001", r(86448.0, 48.0), reference("A001.mov", Some(r(86400.0, 1000.0)))),
                    {"OTIO_SCHEMA": "Gap.1", "source_range": r(0.0, 12.0)},
                    {"OTIO_SCHEMA": "Transition.1", "name": "Cross Dissolve",
                     "in_offset": t(6.0), "out_offset": t(6.0)},
                    a002,
                    {"OTIO_SCHEMA": "Clip.1", "name": "Title", "source_range": r(0.0, 24.0),
                     "media_reference": {"OTIO_SCHEMA": "GeneratorReference.1"}}
                ]},
                {"OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "Video 2", "children": []},
                {"OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "Audio 1", "children": [
                    clip("A001", r(86448.0, 54.0), reference("A001.mov", Some(r(86400.0, 1000.0)))),
                    clip("Music", r(0.0, 30.0), reference("music.wav", None))
                ]}
            ],
            "markers": [{"OTIO_SCHEMA": "Marker.2", "name": "look", "color": "RED", "comment": "fix",
                "marked_range": r(30.0, 0.0)}]
        }
    })
}

#[test]
fn files_without_audio_tracks_bring_media_audio() {
    let mut doc = editor_style();
    let tracks = doc["tracks"]["children"].as_array_mut().unwrap();
    tracks.retain(|t| t["kind"] == "Video");
    let fps = Rational::new(24, 1).unwrap();
    let mut f = Fixture::new(&[]);
    f.env = FakeEnv::default()
        .with("/media/A001.mov", media_info(1000.0 / 24.0, fps, true))
        .with("/media/A002.mov", media_info(10.0, fps, false));
    f.ok(req("timeline.import", json!({ "otio": doc })));
    let p = f.editor.project();
    assert_eq!(p.clip("c1").unwrap().audio_range(), Some((0, 48)));
    assert!(p.clip("c2").unwrap().audio.is_none(), "A002 has no audio");
    let _ = (AudioInfo::clone, MediaInfo::clone);
}

#[test]
fn a_clip_color_comes_back_from_the_file() {
    // Spec §3: the color a clip shows travels as OTIO's item color.
    let mut f = Fixture::new(&[(4.0, true)]);
    f.append("m1", 0, 50);
    f.ok(req("media.set", json!({"media": "m1", "color": "teal"})));
    let doc = export(f.editor.project(), "t", None, Locations::Absolute).unwrap();
    f.ok(req(
        "timeline.import",
        json!({"otio": doc, "replace": true}),
    ));
    let c = &f.editor.project().tracks[0].clips[0];
    assert_eq!(c.color.as_deref(), Some("teal"));
}

#[test]
fn a_marker_past_the_last_clip_keeps_its_length() {
    // Spec D40: a marker keeps its length, and the whole timeline takes it.
    let mut f = Fixture::new(&[(4.0, true)]);
    f.append("m1", 0, 50);
    f.ok(req("markers.add", json!({"at": "40f", "duration": "30f"})));
    let doc = export(f.editor.project(), "t", None, Locations::Absolute).unwrap();
    let range = &doc["tracks"]["markers"][0]["marked_range"];
    assert_eq!(range["duration"]["value"], json!(30.0));
}

#[test]
fn another_editors_color_becomes_the_nearest_clip_color() {
    let item =
        json!({"color": {"OTIO_SCHEMA": "Color.1", "r": 0.1, "g": 0.6, "b": 0.55, "a": 1.0}});
    assert_eq!(clip_color(&item).as_deref(), Some("teal"));
}

#[test]
fn rates_become_exact() {
    let r = |n, d| Rational::new(n, d).unwrap();
    assert_eq!(rational(24.0).unwrap(), r(24, 1));
    assert_eq!(rational(29.97002997002997).unwrap(), r(30000, 1001));
    assert_eq!(rational(29.97).unwrap(), r(30000, 1001));
    assert_eq!(rational(23.976).unwrap(), r(24000, 1001));
    assert_eq!(rational(59.94).unwrap(), r(60000, 1001));
    assert_eq!(rational(25.0).unwrap(), r(25, 1));
    assert_eq!(rational(12.5).unwrap(), r(25, 2));
    // Ten hours of 23.976 timecode doesn't drift.
    assert_eq!(frames(863_136.0, 23.976, r(24000, 1001)), 863_136);
}

/// Picture effects with a fade-in and a fade-out, in frames.
fn faded(fade_in: f64, fade_out: f64) -> Option<Effects> {
    let fades = json!([
        {"Parameter ID": "videoFaderIn", "Parameter Value": fade_in},
        {"Parameter ID": "videoFaderOut", "Parameter Value": fade_out},
    ]);
    Some(Effects {
        from: 0,
        list: vec![json!({
            "OTIO_SCHEMA": "Effect.1",
            "metadata": { EDITOR: { "Effect Name": "Transform", "Enabled": true, "Parameters": fades } },
        })],
    })
}

#[test]
fn joined_parts_get_both_fades_back() {
    // Decision D49: the first part's fade-in and the second's fade-out.
    let whole = faded(10.0, 20.0);
    let (mut first, mut second) = (whole.clone(), whole.clone());
    effects::cut(&mut first, false, true);
    effects::cut(&mut second, true, false);
    assert_eq!(effects::join(&first, &second), Some(whole));
}

#[test]
fn a_fade_where_parts_meet_keeps_them_apart() {
    // Decision D49: joining would lose the fade-out in the middle.
    assert_eq!(effects::join(&faded(0.0, 20.0), &None), None);
}

/// `f`'s project written as an OTIO file of its whole timeline and opened
/// again, with no media files to probe.
fn reopened(f: &Fixture) -> Project {
    let doc = export(f.editor.project(), "t", None, Locations::Absolute).unwrap();
    let opened = crate::editor::open_otio(&doc, None, &FakeEnv::default()).unwrap();
    opened.expect("vidcut's own file").0
}

/// The project as an opened file has it: its revision and time start anew.
fn as_saved(p: &Project) -> Project {
    let mut p = p.clone();
    p.rev = 0;
    p.edited = None;
    p
}

#[test]
fn the_pool_comes_back_from_an_otio_file() {
    let mut f = Fixture::new(&[(10.0, true), (10.0, true), (8.0, false)]);
    f.ok(req("bins.add", json!({"path": "Interviews"})));
    f.ok(req(
        "media.move",
        json!({"media": "m2", "bin": "Interviews"}),
    ));
    f.ok(req("media.set", json!({"media": "m1", "color": "teal"})));
    f.ok(req(
        "media.marks.set",
        json!({"media": "m3", "in": "10f", "out": "40f"}),
    ));
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "0f..50f"}),
    ));
    assert_eq!(as_saved(&reopened(&f)), as_saved(f.editor.project()));
}

#[test]
fn the_settings_come_back_from_an_otio_file() {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.ok(req(
        "project.set",
        json!({"fps": "30000/1001", "drop_frame": true, "width": 1280, "height": 720,
               "sample_rate": 44100, "channels": 6, "ripple_on": false,
               "start_tc": "01:00:00;00", "still_duration": "3s", "use_proxies": true}),
    ));
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "0f..50f"}),
    ));
    assert_eq!(as_saved(&reopened(&f)), as_saved(f.editor.project()));
}

#[test]
fn ids_come_back_from_an_otio_file() {
    let mut f = Fixture::new(&[(10.0, true)]);
    for range in ["0f..20f", "20f..40f", "40f..60f"] {
        f.ok(req("edit.append", json!({"media": "m1", "range": range})));
    }
    f.ok(req("edit.delete", json!({"clips": "c2", "lift": true})));
    f.ok(req("markers.add", json!({"at": "5f", "name": "a"})));
    f.ok(req("markers.add", json!({"at": "45f", "name": "b"})));
    f.ok(req("markers.remove", json!({"marker": "k1"})));
    assert_eq!(as_saved(&reopened(&f)), as_saved(f.editor.project()));
}

#[test]
fn tape_marks_come_back_from_an_otio_file() {
    let mut f = Fixture::new(&[(10.0, true), (10.0, true)]);
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "0f..50f"}),
    ));
    f.ok(req(
        "tape.marks.set",
        json!({"in": "10f", "out": "30f", "order": ["m1", "m2"]}),
    ));
    assert_eq!(as_saved(&reopened(&f)), as_saved(f.editor.project()));
}

#[test]
fn a_timeline_comes_back_from_an_otio_file() {
    let mut f = Fixture::new(&[(10.0, true), (10.0, true), (8.0, false), (6.0, true)]);
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "0f..50f"}),
    ));
    f.ok(req(
        "edit.append",
        json!({"media": "m2", "range": "25f..75f"}),
    ));
    f.ok(req(
        "edit.append",
        json!({"media": "m3", "range": "0f..40f"}),
    ));
    f.ok(req("edit.split", json!({"at": "20f"})));
    f.ok(req(
        "edit.append",
        json!({"media": "m4", "range": "0f..30f", "audio_only": true, "track": "A1"}),
    ));
    f.ok(req(
        "track.set",
        json!({"track": "A1", "volume_db": -6.0, "solo": true}),
    ));
    f.ok(req(
        "clip.set",
        json!({"clips": "c1", "gain_db": -3.0, "color": "pink", "name": "First"}),
    ));
    f.ok(req(
        "edit.change_speed",
        json!({"clips": "c3", "speed": 2.0}),
    ));
    f.ok(req(
        "markers.add",
        json!({"at": "20f", "name": "look", "color": "rose", "note": "fix", "duration": "5f"}),
    ));
    f.ok(req("marks.set", json!({"in": "5f", "out": "80f"})));
    f.ok(req(
        "track.set",
        json!({"track": "V1", "name": "Picture", "locked": true}),
    ));
    assert_eq!(as_saved(&reopened(&f)), as_saved(f.editor.project()));
}
