//! OTIO interchange tests (deviation X9).

use super::*;
use crate::model::{AudioInfo, MediaInfo};
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
