//! OTIO interchange tests (deviation X9).

use super::*;
use crate::model::{AudioInfo, MediaInfo};
use crate::ops::MAIN;
use crate::test_support::{FakeEnv, Fixture, media_info};

fn req(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

/// Every track with everything an OTIO file can carry, and the media
/// path of each clip.
fn clips(p: &Project) -> Vec<(String, crate::model::Clip, PathBuf)> {
    p.tracks
        .iter()
        .flat_map(|t| {
            t.clips.iter().map(|c| {
                (
                    t.id.clone(),
                    c.clone(),
                    p.media(&c.media).unwrap().path.clone(),
                )
            })
        })
        .collect()
}

/// A timeline with J and L cuts, a gap, an audio-only clip, a disabled,
/// a muted and a renamed clip, gain, markers and marks.
fn busy() -> Fixture {
    let mut f = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
    f.append("m1", 0, 100); // c1 0..100
    f.append("m2", 10, 60); // c2 100..150
    f.append("m1", 100, 150); // c3 150..200
    f.append("m3", 0, 40); // c4 200..240, no audio
    f.append("m2", 0, 50); // c5 240..290
    let ok = |f: &mut Fixture, m: &str, p: Value| {
        f.ok(req(m, p));
    };
    // L cut c1|c2, J cut c2|c3.
    ok(
        &mut f,
        "edit.audio_roll",
        json!({"clip": "c1", "edge": "tail", "by": 5}),
    );
    ok(
        &mut f,
        "edit.audio_roll",
        json!({"clip": "c3", "edge": "head", "by": -7}),
    );
    ok(
        &mut f,
        "clip.set",
        json!({"clips": "c2", "gain_db": -6.0, "name": "interview"}),
    );
    ok(&mut f, "clip.set", json!({"clips": "c3", "enabled": false}));
    ok(&mut f, "clip.set", json!({"clips": "c5", "muted": true}));
    // A gap where c4 was, then audio only in part of it.
    ok(&mut f, "edit.delete", json!({"clips": "c4", "lift": true}));
    ok(
        &mut f,
        "edit.overwrite",
        json!({"media": "m2", "range": "0f..20f", "at": 210, "use_marks": false}),
    );
    ok(
        &mut f,
        "markers.add",
        json!({"at": 120, "name": "look", "color": "mint", "note": "check"}),
    );
    ok(&mut f, "markers.add", json!({"at": 0, "color": "red"}));
    ok(&mut f, "marks.set", json!({"in": 30, "out": 260}));
    f
}

#[test]
fn round_trip_is_frame_exact() {
    let mut f = busy();
    // An audio-only clip in the gap (Audio Only edits make these).
    let p = f.editor.project().clone();
    let gapped = p.clip("c6").unwrap().clone();
    assert_eq!((gapped.start, gapped.duration), (210, 20));
    let mut audio_only = p.clone();
    audio_only
        .track_mut(MAIN)
        .unwrap()
        .clips
        .iter_mut()
        .for_each(|c| {
            if c.id == "c6" {
                c.video = false;
            }
        });
    f.editor = crate::Editor::new(audio_only, Default::default());
    let before = f.editor.project().clone();
    assert!(crate::validate::violations(&before).is_empty());

    // Through text, as a file would be.
    let text =
        serde_json::to_string_pretty(&export(&before, "busy", None, Locations::Absolute).unwrap())
            .unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
    let after = g.editor.project();
    assert_eq!(after.settings.fps, before.settings.fps);
    let strip = |v: Vec<(String, crate::model::Clip, PathBuf)>| -> Vec<_> {
        v.into_iter()
            .map(|(t, mut c, path)| {
                c.id.clear();
                (t, c, path)
            })
            .collect()
    };
    assert_eq!(strip(clips(after)), strip(clips(&before)));
    let marker = |p: &Project| -> Vec<_> {
        p.markers
            .iter()
            .map(|k| (k.at, k.name.clone(), k.color.clone(), k.note.clone()))
            .collect()
    };
    assert_eq!(marker(after), marker(&before));
    assert_eq!(after.marks, before.marks);

    // And the same file again appends a second copy after the end.
    let end = after.end();
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["clips"].as_array().unwrap().len(), 5);
    let p = g.editor.project();
    assert_eq!(p.track(MAIN).unwrap().clips[5].start, end);
    assert_eq!(p.markers.len(), 4);
    assert_eq!(p.media.len(), 3, "media already in the pool are reused");
    // With `replace` it is one copy again.
    g.ok(req(
        "timeline.import",
        json!({ "otio": doc, "replace": true }),
    ));
    assert_eq!(strip(clips(g.editor.project())), strip(clips(&before)));
    // One undo step each.
    g.ok(req("history.undo", json!({})));
    assert_eq!(g.editor.project().track(MAIN).unwrap().clips.len(), 10);
}

#[test]
fn ranges_and_rate_conversion() {
    let f = busy();
    let p = f.editor.project();
    // 100..200 holds c2 (with c1's L-cut audio before it) and c3.
    let doc = export(p, "part", Some((100, 200)), Locations::Absolute).unwrap();
    let t = import(&doc, None).unwrap();
    assert_eq!(
        t.clips
            .iter()
            .map(|c| (c.start, c.duration, c.source_in, c.audio))
            .collect::<Vec<_>>(),
        vec![(0, 50, 10, Some((5, -7))), (50, 50, 100, Some((-7, 0)))]
    );
    assert_eq!(t.start_tc, 100);
    assert_eq!(t.markers.len(), 1);
    assert_eq!(
        t.marks,
        Marks {
            mark_in: None,
            mark_out: None
        }
    );

    // Into a 50 fps project: positions double, with a warning.
    let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
    g.ok(req("project.set", json!({"fps": "50"})));
    g.append("m3", 0, 10);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    // c1's L-cut audio (100..105) came along without c1's video, over c2.
    let w = r.result["warnings"].to_string();
    assert!(
        w.contains("1 clip(s) that overlapped") && w.contains("converted"),
        "{w}"
    );
    let v1 = g.v1();
    assert_eq!(v1[1].1..v1[1].1 + v1[1].2, 10..110);
    assert_eq!(v1[1].3, 20);
}

#[test]
fn missing_media_is_an_error_unless_found_by_name() {
    let f = busy();
    let doc = export(f.editor.project(), "busy", None, Locations::Absolute).unwrap();
    let text = doc.to_string().replace("\"/media/", "\"/gone/");
    let doc: Value = serde_json::from_str(&text).unwrap();
    let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
    let e = g.err(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(e.code, crate::ErrorCode::Media);
    assert!(e.message.contains("/gone/1.mp4"), "{}", e.message);
    g.ok(req(
        "timeline.import",
        // Subfolders are searched too.
        json!({ "otio": doc, "search": ["/"] }),
    ));
    assert_eq!(g.v1().len(), 5);
    let e = g.err(req("timeline.import", json!({})));
    assert_eq!(e.code, crate::ErrorCode::Usage);
}

/// A file as Resolve writes it: `Clip.1`, source ranges in the media's
/// timecode relative to its available range, float rates, a transition,
/// music on the audio track.
fn resolve_style() -> Value {
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
fn reads_resolve_style_files() {
    let t = import(&resolve_style(), None).unwrap();
    assert_eq!(t.fps, Rational::new(24, 1).unwrap());
    assert_eq!(t.start_tc, 86400);
    let v1: Vec<&ImportedClip> = t.clips.iter().filter(|c| c.track == "V1").collect();
    let a = v1[0];
    assert_eq!((a.start, a.duration, a.source_in), (0, 48, 48));
    // The audio runs 6 frames past the video: an L cut.
    assert_eq!(a.audio, Some((0, 6)));
    let b = v1[1];
    assert_eq!(
        (b.start, b.duration, b.source_in, b.enabled),
        (60, 24, 10, false)
    );
    assert_eq!(b.audio, None);
    assert_eq!(v1.len(), 2, "the title has no media file");
    // The music, alone on its audio track, gets an audio track (A1).
    let music: Vec<&ImportedClip> = t.clips.iter().filter(|c| c.track == "A1").collect();
    assert_eq!(
        (music[0].start, music[0].duration, music[0].video),
        (54, 30, false)
    );
    assert_eq!(
        t.markers,
        vec![ImportedMarker {
            at: 30,
            name: "look".into(),
            note: "fix".into(),
            color: "red".into()
        }]
    );
    let w = t.warnings.join("\n");
    for expected in ["transitions", "GeneratorReference"] {
        assert!(w.contains(expected), "{expected:?} in {w}");
    }
    assert!(!w.contains("overlapped"), "nothing else was left out: {w}");

    // Through the editor: an empty 25 fps project becomes 24 fps from 01:00:00:00.
    let fps = Rational::new(24, 1).unwrap();
    let mut music = media_info(60.0, fps, true);
    music.video = None;
    let mut f = Fixture::new(&[]);
    f.env = FakeEnv::default()
        .with("/media/A001.mov", media_info(1000.0 / 24.0, fps, true))
        .with("/media/A002.mov", media_info(10.0, fps, false))
        .with("/media/music.wav", music);
    let r = f.ok(req("timeline.import", json!({ "otio": resolve_style() })));
    let p = f.editor.project();
    assert_eq!((p.settings.fps, p.settings.start_tc), (fps, 86400));
    assert_eq!(
        f.v1(),
        vec![
            crate::test_support::row("c1", 0, 48, 48),
            crate::test_support::row("c3", 60, 24, 10),
        ]
    );
    assert_eq!(
        f.track("A1"),
        vec![crate::test_support::row("c2", 54, 30, 0)]
    );
    assert_eq!(f.track_ids(), ["V1", "V2", "A1"], "Video 2 comes too");
    let c1 = p.clip("c1").unwrap();
    assert_eq!(c1.audio_range(), Some((0, 54)));
    assert!(p.clip("c3").unwrap().audio.is_none() && !p.clip("c3").unwrap().enabled);
    assert_eq!(r.result["media"], json!(["m1", "m2", "m3"]));
    assert_eq!(p.markers[0].color, "red");
}

#[test]
fn imported_gains_stay_within_range() {
    // A file from elsewhere may carry any gain: the import sets it to
    // the nearest of -60..24 dB (D12) and says so.
    let f = busy();
    let mut doc = export(f.editor.project(), "busy", None, Locations::Absolute).unwrap();
    let mut raised = 0;
    fn raise(v: &mut Value, raised: &mut usize) {
        match v {
            Value::Object(m) => {
                if let Some(g) = m.get_mut("gain_db") {
                    *g = json!(1000.0);
                    *raised += 1;
                }
                m.values_mut().for_each(|v| raise(v, raised));
            }
            Value::Array(a) => a.iter_mut().for_each(|v| raise(v, raised)),
            _ => {}
        }
    }
    raise(&mut doc, &mut raised);
    assert!(raised > 0);
    let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    let warnings = r.result["warnings"].to_string();
    assert!(warnings.contains("-60..24 dB"), "{warnings}");
    let p = g.editor.project();
    let gains: Vec<f64> = p
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter_map(|c| c.audio.as_ref().map(|a| a.gain_db))
        .collect();
    assert!(gains.contains(&24.0), "{gains:?}");
    assert!(
        gains.iter().all(|g| (-60.0..=24.0).contains(g)),
        "{gains:?}"
    );
}

#[test]
fn files_without_audio_tracks_bring_media_audio() {
    let mut doc = resolve_style();
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
fn media_urls_and_paths() {
    let fps = Rational::new(25, 1).unwrap();
    let paths = |url: &str, base: Option<&str>| -> Vec<String> {
        locate(url, base.map(Path::new), fps)
            .unwrap()
            .paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };
    // Percent-encoded file URLs; OTIO's Python library doesn't encode them.
    assert_eq!(
        paths(
            "file:///home/me/My%20Clips/take%20%231%20%28%C3%A4%29.mp4",
            None
        ),
        [
            "/home/me/My Clips/take #1 (ä).mp4",
            "/home/me/My%20Clips/take%20%231%20%28%C3%A4%29.mp4"
        ]
    );
    assert_eq!(paths("file:///a b/c #1.mov", None), ["/a b/c #1.mov"]);
    assert_eq!(paths("file://localhost/a/b.mov", None), ["/a/b.mov"]);
    // Plain paths as Resolve writes them: as they are first.
    assert_eq!(paths("/plain/50%.mov", None), ["/plain/50%.mov"]);
    assert_eq!(paths("/a/b%20c.mov", None), ["/a/b%20c.mov", "/a/b c.mov"]);
    assert!(locate("file://server/share/a.mov", None, fps).is_none());
    assert!(locate("https://example.com/a.mp4", None, fps).is_none());
    assert!(locate("", None, fps).is_none());
    // Relative to the timeline file (bundles write these).
    assert_eq!(
        paths("media/A001.mov", Some("/bundles/cut")),
        ["/bundles/cut/media/A001.mov"]
    );
    assert_eq!(
        paths("media\\A001.mov", Some("/b")),
        ["/b/media\\A001.mov", "/b/media/A001.mov"]
    );
    assert!(
        paths("media/A001.mov", None).is_empty(),
        "no folder to count from"
    );
    // A project folder moved with its timeline file: the path's last parts
    // next to the file.
    assert_eq!(
        paths("/Volumes/Proj/Media/A001.mov", Some("/home/me/Proj")),
        [
            "/Volumes/Proj/Media/A001.mov",
            "/home/me/Proj/Proj/Media/A001.mov",
            "/home/me/Proj/Media/A001.mov",
            "/home/me/Proj/A001.mov"
        ]
    );
    // Windows paths, as Resolve on Windows writes them: only near the file
    // here, and by name.
    let w = locate(
        "E:\\subMedia\\Mel\\Mel q7-camera2.mp4",
        Some(Path::new("/x")),
        fps,
    )
    .unwrap();
    assert_eq!(w.file_name, "Mel q7-camera2.mp4");
    assert_eq!(w.shown, "E:\\subMedia\\Mel\\Mel q7-camera2.mp4");
    assert_eq!(
        w.paths,
        [
            "/x/subMedia/Mel/Mel q7-camera2.mp4",
            "/x/Mel/Mel q7-camera2.mp4",
            "/x/Mel q7-camera2.mp4"
        ]
        .map(PathBuf::from)
    );
    let w = locate("file:///C:/Users/me/a%20b.mov", None, fps).unwrap();
    assert_eq!((w.file_name.as_str(), w.paths.len()), ("a b.mov", 0));
    // An image sequence named as Resolve shows one.
    let s = locate("/renders/frame_[0001-0120].png", None, fps).unwrap();
    assert_eq!(s.paths, [PathBuf::from("/renders/frame_0001.png")]);
    assert_eq!(
        (s.file_name.as_str(), s.sequence),
        ("frame_0001.png", Some(fps))
    );
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

#[test]
fn several_tracks_round_trip() {
    // M6: B-roll on V2, music on A1, track settings, an audio-only clip on
    // V2 and a muted, soloed, faded track.
    let mut f = busy();
    let music = f.add_music("music.wav", 20.0);
    let ok = |f: &mut Fixture, m: &str, p: Value| {
        f.ok(req(m, p));
    };
    ok(&mut f, "marks.clear", json!({}));
    ok(
        &mut f,
        "edit.overwrite",
        json!({"media": "m3", "range": "0f..30f", "at": 20, "track": "V2"}),
    );
    ok(
        &mut f,
        "edit.overwrite",
        json!({"media": "m1", "range": "0f..15f", "at": 60, "track": "V2", "audio_only": true}),
    );
    ok(
        &mut f,
        "edit.overwrite",
        json!({"media": music, "range": "0f..200f", "at": 10}),
    );
    ok(
        &mut f,
        "edit.overwrite",
        json!({"media": music, "range": "300f..350f", "at": 230, "track": "A2"}),
    );
    ok(
        &mut f,
        "track.set",
        json!({"track": "A1", "volume_db": -9.0, "name": "Music"}),
    );
    ok(
        &mut f,
        "track.set",
        json!({"track": "A2", "muted": true, "solo": true}),
    );
    ok(
        &mut f,
        "track.set",
        json!({"track": "V2", "video_enabled": false}),
    );
    let before = f.editor.project().clone();
    let doc = export(&before, "busy", None, Locations::Absolute).unwrap();
    let names: Vec<&str> = doc["tracks"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["V1", "V2", "V1 audio", "V2 audio", "A1", "A2"]);

    let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
    g.add_music("music.wav", 20.0);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
    let after = g.editor.project();
    let strip = |v: Vec<(String, crate::model::Clip, PathBuf)>| -> Vec<_> {
        v.into_iter()
            .map(|(t, mut c, path)| {
                c.id.clear();
                (t, c, path)
            })
            .collect()
    };
    assert_eq!(strip(clips(after)), strip(clips(&before)));
    let settings = |p: &Project| -> Vec<_> {
        p.tracks
            .iter()
            .map(|t| {
                (
                    t.id.clone(),
                    t.muted,
                    t.solo,
                    t.volume_db,
                    t.video_enabled,
                    t.name.clone(),
                )
            })
            .collect()
    };
    assert_eq!(settings(after), settings(&before));
}

#[test]
fn resolve_style_tracks_map_onto_ours() {
    // A Resolve-style file: B-roll with linked audio on Video 2/Audio 2,
    // and music alone on Audio 3.
    let doc = resolve_style();
    let mut doc = doc;
    let tracks = doc["tracks"]["children"].as_array_mut().unwrap();
    // Video 2 gets a clip of A002 over A001, with its audio on a new
    // Audio 2 track; Audio 1's Music clip moves to Audio 3.
    let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": v});
    let r = |s: f64, d: f64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": t(s), "duration": t(d)});
    let a002 = json!({"OTIO_SCHEMA": "Clip.1", "name": "A002", "source_range": r(0.0, 12.0),
        "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///media/A002.mov"}});
    let gap = json!({"OTIO_SCHEMA": "Gap.1", "source_range": r(0.0, 10.0)});
    tracks[1]["children"] = json!([gap.clone(), a002.clone()]);
    let music = tracks[2]["children"].as_array_mut().unwrap().pop().unwrap();
    tracks.push(json!({"OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "Audio 2", "children": [gap, a002]}));
    tracks.push(
        json!({"OTIO_SCHEMA": "Track.1", "kind": "Audio", "name": "Audio 3", "enabled": false,
        "children": [{"OTIO_SCHEMA": "Gap.1", "source_range": r(0.0, 54.0)}, music]}),
    );
    let t = import(&doc, None).unwrap();
    // (start, duration, video, audio head/tail) of the clips on a track.
    type Placed = (i64, i64, bool, Option<(i64, i64)>);
    let on = |id: &str| -> Vec<Placed> {
        t.clips
            .iter()
            .filter(|c| c.track == id)
            .map(|c| (c.start, c.duration, c.video, c.audio))
            .collect()
    };
    assert_eq!(
        on("V2"),
        vec![(10, 12, true, Some((0, 0)))],
        "linked audio paired across tracks"
    );
    assert_eq!(
        on("A1"),
        vec![(54, 30, false, Some((0, 0)))],
        "music on its own track"
    );
    assert!(
        t.tracks.iter().any(|s| s.id == "A1" && s.muted),
        "disabled track: muted"
    );
    assert!(
        !t.warnings.iter().any(|w| w.contains("audio clip")),
        "{:?}",
        t.warnings
    );
}

/// The clip ids and media paths stripped: what an import must reproduce.
fn shape(p: &Project) -> Vec<(String, crate::model::Clip, PathBuf)> {
    clips(p)
        .into_iter()
        .map(|(t, mut c, path)| {
            c.id.clear();
            (t, c, path)
        })
        .collect()
}

/// The OTIO tracks of `doc` by name.
fn otio_track<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc["tracks"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no track {name}"))
}

fn otio_clips(track: &Value) -> Vec<&Value> {
    track["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| is_clip(c))
        .collect()
}

#[test]
fn writes_what_resolve_reads() {
    // Resolve places sources by timecode, links a clip's video and audio by
    // a group number, reads plain paths, its own clip volume and track
    // settings (deviation X9).
    let f = busy();
    let mut p = f.editor.project().clone();
    p.media[0].info.timecode = Some(crate::model::StartTimecode {
        count: 900_000, // 10:00:00:00
        rate: Rational::new(25, 1).unwrap(),
    });
    let doc = export(&p, "busy", None, Locations::Absolute).unwrap();
    assert_eq!(
        doc["metadata"]["Resolve_OTIO"]["Resolve OTIO Meta Version"],
        "1.0"
    );
    let v1 = otio_clips(otio_track(&doc, "V1"));
    let audio = otio_clips(otio_track(&doc, "V1 audio"));
    let reference = &v1[0]["media_references"]["DEFAULT_MEDIA"];
    assert_eq!(reference["target_url"], "/media/1.mp4", "a plain path");
    assert_eq!(
        reference["available_range"]["start_time"]["value"],
        900_000.0
    );
    // c1 shows m1 from frame 0, c3 from frame 100: from 10:00:00:00 on.
    assert_eq!(v1[0]["source_range"]["start_time"]["value"], 900_000.0);
    assert_eq!(v1[2]["source_range"]["start_time"]["value"], 900_100.0);
    assert_eq!(
        v1[1]["source_range"]["start_time"]["value"], 10.0,
        "m2 has none"
    );
    // Video and audio of a clip share a link group; the audio names its
    // stream's channels.
    let group = |c: &Value| c["metadata"]["Resolve_OTIO"]["Link Group ID"].as_i64();
    assert_eq!(group(v1[0]), Some(1));
    assert_eq!(group(audio[0]), Some(1));
    assert_eq!(group(v1[1]), group(audio[1]));
    assert_ne!(group(v1[0]), group(v1[1]));
    assert_eq!(
        audio[0]["metadata"]["Resolve_OTIO"]["Channels"],
        json!([
            {"Source Channel ID": 0, "Source Track ID": 0},
            {"Source Channel ID": 1, "Source Track ID": 0}
        ])
    );
    // c2's -6 dB as Resolve's clip volume.
    let volume = &audio[1]["effects"][0]["metadata"]["Resolve_OTIO"];
    assert_eq!(volume["Effect Name"], "Fairlight Clip Volume and Fades");
    assert_eq!(volume["Parameters"][0]["Parameter Value"], -6.0);
    assert_eq!(audio[0]["effects"], json!([]));
    let sound = &otio_track(&doc, "V1 audio")["metadata"]["Resolve_OTIO"];
    assert_eq!(sound["Audio Type"], "Stereo");
    assert_eq!(sound["Locked"], false);
    // Markers: a frame long, the note where Resolve keeps it too.
    let marker = &doc["tracks"]["markers"][1];
    assert_eq!(marker["marked_range"]["duration"]["value"], 1.0);
    assert_eq!(marker["comment"], "check");
    assert_eq!(marker["metadata"]["Resolve_OTIO"]["Note"], "check");
    assert!(!doc.to_string().contains("file://"));

    // And back, frame for frame: source ranges count from the timecode.
    let mut g = Fixture::new(&[(10.0, true), (4.0, true), (8.0, false)]);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
    assert_eq!(shape(g.editor.project()), shape(&p));
}

#[test]
fn stills_and_sequences_round_trip() {
    // A still is available for one frame, as Resolve writes it; a numbered
    // sequence is an ImageSequenceReference (M9).
    let mut f = Fixture::new(&[(10.0, true)]);
    f.env = std::mem::take(&mut f.env)
        .with(
            "/media/photo.png",
            crate::test_support::still_info(4000, 3000),
        )
        .with_sequence("/renders", "frame_%04d.png", 100, 50);
    f.ok(req("media.add", json!({"paths": ["/media/photo.png"]})));
    f.ok(req(
        "media.add",
        json!({"paths": ["/renders/frame_0100.png"], "sequence": true, "fps": "50"}),
    ));
    f.append("m2", 0, 60);
    f.append("m3", 4, 20);
    // A still trimmed at the head keeps a source position.
    f.ok(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "head", "by": 10}),
    ));
    let before = f.editor.project().clone();
    assert_eq!(before.clip("c1").unwrap().source_in, 10);
    let doc = export(&before, "pictures", None, Locations::Absolute).unwrap();
    let v1 = otio_clips(otio_track(&doc, "V1"));
    let still = &v1[0];
    assert_eq!(still["source_range"]["start_time"]["value"], 0.0);
    assert_eq!(
        still["media_references"]["DEFAULT_MEDIA"]["available_range"]["duration"]["value"],
        1.0
    );
    let seq = &v1[1]["media_references"]["DEFAULT_MEDIA"];
    assert_eq!(seq["OTIO_SCHEMA"], "ImageSequenceReference.1");
    assert_eq!(
        (
            &seq["target_url_base"],
            &seq["name_prefix"],
            &seq["name_suffix"]
        ),
        (&json!("/renders/"), &json!("frame_"), &json!(".png"))
    );
    assert_eq!(
        (
            &seq["start_frame"],
            &seq["frame_zero_padding"],
            &seq["rate"]
        ),
        (&json!(100), &json!(4), &json!(50.0))
    );

    let mut g = Fixture::new(&[(10.0, true)]);
    g.env = std::mem::take(&mut g.env)
        .with(
            "/media/photo.png",
            crate::test_support::still_info(4000, 3000),
        )
        .with_sequence("/renders", "frame_%04d.png", 100, 50);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
    let after = g.editor.project();
    assert_eq!(shape(after), shape(&before));
    let m = after.media(&after.clip("c2").unwrap().media).unwrap();
    assert_eq!(m.sequence, before.media("m3").unwrap().sequence);
    assert_eq!(
        m.info.video.as_ref().unwrap().fps,
        Rational::new(50, 1).unwrap()
    );
}

/// A timeline as DaVinci Resolve exports one (the shapes of real exports,
/// cut down): `Clip.2` with Resolve's effects, Windows paths, a link group
/// number on each clip's video and audio, the stream in a channel map,
/// clip volume, media Resolve had offline, a clip marker, timecode source
/// ranges with float noise, no timeline name.
fn resolve_export() -> Value {
    let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": v});
    let r = |s: f64, d: f64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": t(s), "duration": t(d)});
    let effect = |meta: Value| json!({"OTIO_SCHEMA": "Effect.1", "name": "", "effect_name": "Resolve Effect", "metadata": {"Resolve_OTIO": meta}});
    let clip = |name: &str,
                source: Value,
                reference: Value,
                resolve: Value,
                effects: Vec<Value>| {
        json!({
            "OTIO_SCHEMA": "Clip.2", "name": name, "source_range": source, "effects": effects,
            "markers": [], "enabled": true, "metadata": {"Resolve_OTIO": resolve},
            "media_references": {"DEFAULT_MEDIA": reference}, "active_media_reference_key": "DEFAULT_MEDIA"
        })
    };
    let external = |url: &str, available: Value| json!({"OTIO_SCHEMA": "ExternalReference.1", "metadata": {}, "name": "", "target_url": url, "available_range": available, "available_image_bounds": null});
    let a001 = external("E:\\Shoot\\Day 1\\A001.mov", r(86_400.000_000_01, 2400.0));
    let channels = json!([{"Source Channel ID": 0, "Source Track ID": 1}, {"Source Channel ID": 1, "Source Track ID": 1}]);
    let volume = effect(
        json!({"Effect Name": "Fairlight Clip Volume and Fades", "Enabled": true, "Name": "Volume", "Type": 62,
        "Parameters": [{"Parameter ID": "volume", "Parameter Value": -16.999_998_092_651_367, "Key Frames": {}, "Variant Type": "Double"},
                       {"Parameter ID": "faderIn", "Parameter Value": 0.0, "Variant Type": "Double"}]}),
    );
    let transform = effect(
        json!({"Effect Name": "Transform", "Enabled": true, "Name": "Transform", "Parameters": [], "Type": 2}),
    );
    let mut v_a001 = clip(
        "A001.mov",
        r(86_448.0, 48.0),
        a001.clone(),
        json!({"Link Group ID": 1}),
        vec![transform.clone()],
    );
    v_a001["markers"] = json!([{"OTIO_SCHEMA": "Marker.2", "name": "focus", "color": "GREEN", "comment": "",
        "metadata": {"Resolve_OTIO": {"Keywords": [], "Note": "soft"}}, "marked_range": r(86_458.0, 1.0)}]);
    let a_a001 = clip(
        "A001.mov",
        r(86_448.0, 48.0),
        a001.clone(),
        json!({"Link Group ID": 1, "Channels": channels}),
        vec![volume],
    );
    // The same media, in sync but in another link group: not this clip's audio.
    let other = clip(
        "A001.mov",
        r(86_496.0, 24.0),
        a001,
        json!({"Link Group ID": 7, "Channels": channels}),
        vec![],
    );
    let offline = json!({"OTIO_SCHEMA": "MissingReference.1", "name": "B002.mov",
        "metadata": {"Resolve_OTIO": {"Media Url": "/Volumes/Shoot/B002.mov"}}, "available_range": r(0.0, 240.0)});
    let mut speed = clip(
        "B002.mov",
        r(0.0, 24.0),
        offline,
        json!({}),
        vec![transform],
    );
    speed["effects"].as_array_mut().unwrap().push(json!({"OTIO_SCHEMA": "LinearTimeWarp.1", "name": "", "effect_name": "", "time_scalar": 2.0}));
    let gap = |d: f64| json!({"OTIO_SCHEMA": "Gap.1", "name": "", "source_range": r(0.0, d), "effects": [], "markers": [], "enabled": true, "metadata": {}});
    json!({
        "OTIO_SCHEMA": "Timeline.1", "name": "", "global_start_time": t(86_400.0),
        "metadata": {"Resolve_OTIO": {"Resolve OTIO Meta Version": "1.0"}},
        "tracks": {"OTIO_SCHEMA": "Stack.1", "name": "", "metadata": {}, "markers": [], "source_range": null, "effects": [], "enabled": true,
            "children": [
                {"OTIO_SCHEMA": "Track.1", "name": "Video 1", "kind": "Video", "metadata": {"Resolve_OTIO": {"Locked": true}},
                 "markers": [{"OTIO_SCHEMA": "Marker.2", "name": "track", "color": "RED", "comment": "", "metadata": {}, "marked_range": r(60.0, 1.0)}],
                 "enabled": true, "source_range": null, "effects": [], "children": [v_a001, speed]},
                {"OTIO_SCHEMA": "Track.1", "name": "Audio 1", "kind": "Audio", "metadata": {"Resolve_OTIO": {"Audio Type": "Stereo", "Locked": false, "SoloOn": true}},
                 "markers": [], "enabled": true, "source_range": null, "effects": [], "children": [a_a001, other, gap(10.0)]}
            ]}
    })
}

#[test]
fn reads_resolve_exports() {
    let t = import(&resolve_export(), None).unwrap();
    assert_eq!((t.fps, t.start_tc), (Rational::new(24, 1).unwrap(), 86_400));
    let on =
        |id: &str| -> Vec<&ImportedClip> { t.clips.iter().filter(|c| c.track == id).collect() };
    let v1 = on("V1");
    // Source positions from the available range (the media's timecode).
    assert_eq!((v1[0].start, v1[0].duration, v1[0].source_in), (0, 48, 48));
    assert_eq!(v1[0].audio, Some((0, 0)), "its link group's audio");
    assert_eq!(
        (v1[0].stream, v1[0].gain_db),
        (1, -17.0),
        "the channel map's stream, the clip volume"
    );
    assert_eq!(v1[0].media.file_name, "A001.mov");
    assert!(
        v1[0].media.paths.is_empty(),
        "a Windows path: found by name only"
    );
    // The offline media's URL.
    assert_eq!(
        v1[1].media.paths,
        [PathBuf::from("/Volumes/Shoot/B002.mov")]
    );
    // In sync, but another link group: audio of its own.
    let a1 = on("A1");
    assert_eq!((a1[0].start, a1[0].video, a1[0].stream), (48, false, 1));
    assert!(
        t.tracks.iter().any(|s| s.id == "V1" && s.locked),
        "Resolve's track lock"
    );
    assert!(t.tracks.iter().any(|s| s.id == "A1" && s.solo));
    // The clip marker lands where it is in the clip; the note is Resolve's.
    assert_eq!(
        t.markers,
        vec![
            ImportedMarker {
                at: 10,
                name: "focus".into(),
                note: "soft".into(),
                color: "green".into()
            },
            ImportedMarker {
                at: 60,
                name: "track".into(),
                note: String::new(),
                color: "red".into()
            },
        ]
    );
    let w = t.warnings.join("\n");
    assert!(w.contains("speed changes"), "{w}");

    // Through the editor: found by name, named after the file.
    let fps = Rational::new(24, 1).unwrap();
    let mut two = media_info(100.0, fps, true);
    two.audio.push(two.audio[0].clone());
    two.audio[1].index = 1;
    let mut f = Fixture::new(&[]);
    f.env = FakeEnv::default()
        .with("/footage/day1/A001.mov", two)
        .with("/Volumes/Shoot/B002.mov", media_info(10.0, fps, false));
    let e = f.err(req("timeline.import", json!({ "otio": resolve_export() })));
    assert!(e.message.contains("E:\\Shoot\\Day 1\\A001.mov"), "{e}");
    let r = f.ok(req(
        "timeline.import",
        json!({ "otio": resolve_export(), "search": ["/footage"] }),
    ));
    assert_eq!(r.result["media"], json!(["m1", "m2"]));
    let p = f.editor.project();
    let c1 = p.clip("c1").unwrap();
    assert_eq!(
        c1.audio.as_ref().map(|a| (a.stream, a.gain_db)),
        Some((1, -17.0))
    );
    assert_eq!(p.markers.len(), 2);
}

#[test]
fn relative_paths_count_from_the_file() {
    // Bundles name their media relative to themselves (`.otiod` folders,
    // unpacked `.otioz`), and a project folder may move with its timeline.
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-otio-rel-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let bundle = dir.join("cut.otiod");
    std::fs::create_dir_all(&bundle).unwrap();
    let fps = Rational::new(25, 1).unwrap();
    let mut doc = resolve_style();
    let urls = [
        "media/A001.mov",
        "/Volumes/Proj/A002.mov",
        "media%2Fmusic.wav",
    ];
    let mut n = 0;
    fn set_urls(v: &mut Value, urls: &[&str], n: &mut usize) {
        match v {
            Value::Object(m) => {
                if let Some(u) = m.get_mut("target_url") {
                    let name = u.as_str().unwrap().rsplit('/').next().unwrap().to_string();
                    *u = json!(
                        urls.iter()
                            .find(|x| x.ends_with(&name))
                            .copied()
                            .unwrap_or("")
                    );
                    *n += 1;
                }
                m.values_mut().for_each(|v| set_urls(v, urls, n));
            }
            Value::Array(a) => a.iter_mut().for_each(|v| set_urls(v, urls, n)),
            _ => {}
        }
    }
    set_urls(&mut doc, &urls, &mut n);
    assert_eq!(n, 4);
    std::fs::write(bundle.join("content.otio"), doc.to_string()).unwrap();
    let mut music = media_info(60.0, fps, true);
    music.video = None;
    let mut f = Fixture::new(&[]);
    f.env = FakeEnv::default()
        .with(
            bundle.join("media/A001.mov").to_str().unwrap(),
            media_info(1000.0 / 24.0, fps, true),
        )
        .with(
            bundle.join("A002.mov").to_str().unwrap(),
            media_info(10.0, fps, false),
        )
        .with(bundle.join("media/music.wav").to_str().unwrap(), music);
    // The folder itself, as OpenTimelineIO's adapters name an .otiod.
    let r = f.ok(req("timeline.import", json!({ "path": bundle })));
    assert_eq!(
        r.result["media"].as_array().unwrap().len(),
        3,
        "{}",
        r.result
    );
    assert_eq!(r.result["name"], "Cut");
    let p = f.editor.project();
    assert!(p.media.iter().any(|m| m.path == bundle.join("A002.mov")));
    // Or its content.otio.
    let mut g = Fixture::new(&[]);
    g.env = std::mem::take(&mut f.env);
    g.ok(req(
        "timeline.import",
        json!({ "path": bundle.join("content.otio") }),
    ));
    assert_eq!(shape(g.editor.project()), shape(f.editor.project()));
    // An .otioz needs the vidcut program.
    let e = g.err(req(
        "timeline.import",
        json!({ "path": dir.join("cut.otioz") }),
    ));
    assert_eq!(e.code, crate::ErrorCode::Usage);
}

#[test]
fn source_timecode_without_available_ranges() {
    // A writer that leaves out available ranges still counts sources in the
    // media's timecode, as Resolve does: the media's start timecode comes
    // off them.
    let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": v});
    let r = |s: f64, d: f64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": t(s), "duration": t(d)});
    let clip = |file: &str, s: f64| {
        json!({"OTIO_SCHEMA": "Clip.1", "name": file, "source_range": r(s, 20.0),
        "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": format!("/media/{file}")}})
    };
    let doc = json!({"OTIO_SCHEMA": "Timeline.1", "name": "tc", "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [
        {"OTIO_SCHEMA": "Track.1", "kind": "Video", "children": [clip("cam.mov", 900_010.0), clip("plain.mp4", 30.0)]},
        {"OTIO_SCHEMA": "Track.1", "kind": "Audio", "children": [clip("cam.mov", 900_010.0)]}
    ]}});
    let fps = Rational::new(25, 1).unwrap();
    let mut cam = media_info(60.0, fps, true);
    cam.timecode = Some(crate::model::StartTimecode {
        count: 900_000,
        rate: fps,
    });
    let mut f = Fixture::new(&[]);
    f.env = FakeEnv::default()
        .with("/media/cam.mov", cam)
        .with("/media/plain.mp4", media_info(60.0, fps, true));
    let r = f.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
    assert_eq!(
        f.v1(),
        vec![
            crate::test_support::row("c1", 0, 20, 10),
            crate::test_support::row("c2", 20, 20, 30)
        ]
    );
    assert_eq!(
        f.editor.project().clip("c1").unwrap().audio_range(),
        Some((0, 20)),
        "its own audio"
    );
}
