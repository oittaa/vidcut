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
                // A generator has no media.
                let path = p.media(&c.media).map_or(PathBuf::new(), |m| m.path.clone());
                (t.id.clone(), c.clone(), path)
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
fn reads_editor_style_files() {
    let t = import(&editor_style(), None).unwrap();
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
    // The title: a generator clip, without media.
    assert_eq!(v1.len(), 3);
    assert_eq!((v1[2].start, v1[2].duration), (84, 24));
    assert!(v1[2].generator.is_some());
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
            duration: 1,
            name: "look".into(),
            note: "fix".into(),
            color: "red".into()
        }]
    );
    let w = t.warnings.join("\n");
    for expected in ["transitions kept", "generators and titles kept"] {
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
    let r = f.ok(req("timeline.import", json!({ "otio": editor_style() })));
    let p = f.editor.project();
    assert_eq!((p.settings.fps, p.settings.start_tc), (fps, 86400));
    assert_eq!(
        f.v1(),
        vec![
            crate::test_support::row("c1", 0, 48, 48),
            crate::test_support::row("c3", 60, 24, 10),
            crate::test_support::row("c4", 84, 24, 0),
        ]
    );
    assert!(p.clip("c4").unwrap().is_generator(), "the title");
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
fn media_urls_and_paths() {
    let fps = Rational::new(25, 1).unwrap();
    // Paths compare by their parts: on Windows `/` and `\` alike.
    let paths = |url: &str, base: Option<&str>| -> Vec<PathBuf> {
        locate(url, base.map(Path::new), fps).unwrap().paths
    };
    let these = |p: &[&str]| -> Vec<PathBuf> { p.iter().map(PathBuf::from).collect() };
    // Percent-encoded file URLs; OTIO's Python library doesn't encode them.
    assert_eq!(
        paths(
            "file:///home/me/My%20Clips/take%20%231%20%28%C3%A4%29.mp4",
            None
        ),
        these(&[
            "/home/me/My Clips/take #1 (ä).mp4",
            "/home/me/My%20Clips/take%20%231%20%28%C3%A4%29.mp4"
        ])
    );
    assert_eq!(
        paths("file:///a b/c #1.mov", None),
        these(&["/a b/c #1.mov"])
    );
    assert_eq!(
        paths("file://localhost/a/b.mov", None),
        these(&["/a/b.mov"])
    );
    // Plain paths: as they are first.
    assert_eq!(paths("/plain/50%.mov", None), these(&["/plain/50%.mov"]));
    assert_eq!(
        paths("/a/b%20c.mov", None),
        these(&["/a/b%20c.mov", "/a/b c.mov"])
    );
    assert!(locate("file://server/share/a.mov", None, fps).is_none());
    assert!(locate("https://example.com/a.mp4", None, fps).is_none());
    assert!(locate("", None, fps).is_none());
    // Relative to the timeline file (bundles write these).
    assert_eq!(
        paths("media/A001.mov", Some("/bundles/cut")),
        these(&["/bundles/cut/media/A001.mov"])
    );
    // A backslash separates on Windows; elsewhere it may be in a name.
    let backslash: &[&str] = if cfg!(windows) {
        &["/b/media/A001.mov"]
    } else {
        &[r"/b/media\A001.mov", "/b/media/A001.mov"]
    };
    assert_eq!(paths(r"media\A001.mov", Some("/b")), these(backslash));
    assert!(
        paths("media/A001.mov", None).is_empty(),
        "no folder to count from"
    );
    // A project folder moved with its timeline file: the path's last parts
    // next to the file.
    assert_eq!(
        paths("/Volumes/Proj/Media/A001.mov", Some("/home/me/Proj")),
        these(&[
            "/Volumes/Proj/Media/A001.mov",
            "/home/me/Proj/Proj/Media/A001.mov",
            "/home/me/Proj/Media/A001.mov",
            "/home/me/Proj/A001.mov"
        ])
    );
    // Windows paths, as editors on Windows write them: there, the path
    // first; elsewhere only near the file, and by name.
    let w = locate(
        r"E:\subMedia\Mel\Mel q7-camera2.mp4",
        Some(Path::new("/x")),
        fps,
    )
    .unwrap();
    assert_eq!(w.file_name, "Mel q7-camera2.mp4");
    assert_eq!(w.shown, r"E:\subMedia\Mel\Mel q7-camera2.mp4");
    let near = [
        "/x/subMedia/Mel/Mel q7-camera2.mp4",
        "/x/Mel/Mel q7-camera2.mp4",
        "/x/Mel q7-camera2.mp4",
    ];
    let mut want = these(&near);
    if cfg!(windows) {
        want.insert(0, PathBuf::from(r"E:\subMedia\Mel\Mel q7-camera2.mp4"));
    }
    assert_eq!(w.paths, want);
    let w = locate("file:///C:/Users/me/a%20b.mov", None, fps).unwrap();
    assert_eq!(w.file_name, "a b.mov");
    let here: &[&str] = if cfg!(windows) {
        &[r"C:\Users\me\a b.mov", r"C:\Users\me\a%20b.mov"]
    } else {
        &[]
    };
    assert_eq!(w.paths, these(here));
    // A server's file, with no timeline folder: on Windows the server with
    // either slash; elsewhere `//` is the root's, and `\\` only a name.
    let server: &[&str] = if cfg!(windows) {
        &[r"\\server\share\a.mov"]
    } else {
        &["//server/share/a.mov"]
    };
    assert_eq!(paths("//server/share/a.mov", None), these(server));
    let server: &[&str] = if cfg!(windows) {
        &[r"\\server\share\a.mov"]
    } else {
        &[]
    };
    assert_eq!(paths(r"\\server\share\a.mov", None), these(server));
    // An image sequence named by its range.
    let s = locate("/renders/frame_[0001-0120].png", None, fps).unwrap();
    assert_eq!(s.paths, these(&["/renders/frame_0001.png"]));
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
    // B-roll on V2, music on A1, track settings, an audio-only clip on
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
        json!({"track": "V2", "video_enabled": false, "name": "B-roll"}),
    );
    let before = f.editor.project().clone();
    let doc = export(&before, "busy", None, Locations::Absolute).unwrap();
    let names: Vec<&str> = doc["tracks"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    // Named tracks keep their names, V2's audio too; the others are named
    // as editors name theirs, by place in the file.
    assert_eq!(
        names,
        ["Video 1", "B-roll", "Audio 1", "B-roll", "Music", "Audio 4"]
    );

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
fn editor_style_tracks_map_onto_ours() {
    // An editor's file: B-roll with linked audio on Video 2/Audio 2,
    // and music alone on Audio 3.
    let doc = editor_style();
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

#[test]
fn editor_track_names_name_ours() {
    // Names given in another editor name the tracks; its defaults don't.
    let mut doc = editor_style();
    let tracks = doc["tracks"]["children"].as_array_mut().unwrap();
    tracks[0]["name"] = json!("Interview");
    tracks[2]["name"] = json!("Sound");
    let t = import(&doc, None).unwrap();
    let names: Vec<(&str, Option<&str>)> = t
        .tracks
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("V1", Some("Interview")),
            ("V2", None),
            ("A1", Some("Sound"))
        ]
    );
    for name in ["Video 12", "Audio 1", "V2", "A1", "", " "] {
        assert!(default_track_name(name), "{name:?}");
    }
    for name in ["Video", "Audio B", "Voice 1", "A-roll", "VO"] {
        assert!(!default_track_name(name), "{name:?}");
    }
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
fn writes_what_editors_read() {
    // Sources placed by timecode, a clip's video and audio linked by a group
    // number, plain paths, the clip volume and track settings in the editor
    // metadata (deviation X9).
    let f = busy();
    let mut p = f.editor.project().clone();
    p.media[0].info.timecode = Some(crate::model::StartTimecode {
        count: 900_000, // 10:00:00:00
        rate: Rational::new(25, 1).unwrap(),
        drop_frame: false,
    });
    let doc = export(&p, "busy", None, Locations::Absolute).unwrap();
    assert_eq!(
        doc["metadata"]["Resolve_OTIO"]["Resolve OTIO Meta Version"],
        "1.0"
    );
    let v1 = otio_clips(otio_track(&doc, "Video 1"));
    let audio = otio_clips(otio_track(&doc, "Audio 1"));
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
    // c2's -6 dB as the clip volume.
    let volume = &audio[1]["effects"][0]["metadata"]["Resolve_OTIO"];
    assert_eq!(volume["Effect Name"], "Fairlight Clip Volume and Fades");
    assert_eq!(volume["Parameters"][0]["Parameter Value"], -6.0);
    assert_eq!(audio[0]["effects"], json!([]));
    let sound = &otio_track(&doc, "Audio 1")["metadata"]["Resolve_OTIO"];
    assert_eq!(sound["Audio Type"], "Stereo");
    assert_eq!(sound["Locked"], false);
    // Markers: a frame long, the note in the editor metadata too.
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
fn marker_lengths_round_trip() {
    // A marker keeps its length through OTIO, cut at the end of what is
    // exported; markers.add and markers.set take it (spec D40).
    let mut f = busy();
    let r = f.ok(req(
        "markers.add",
        json!({"at": 20, "duration": "25f", "name": "long"}),
    ));
    let long = r.result["marker"].as_str().unwrap().to_string();
    let r = f.ok(req("markers.add", json!({"at": 40, "name": "short"})));
    let short = r.result["marker"].as_str().unwrap().to_string();
    f.ok(req("markers.set", json!({"marker": short, "duration": 3})));
    let e = f.err(req("markers.set", json!({"marker": long, "duration": 0})));
    assert_eq!(e.code, crate::ErrorCode::InvalidEdit, "{e}");
    let p = f.editor.project().clone();
    let length = |id: &str| p.markers.iter().find(|k| k.id == id).unwrap().duration;
    assert_eq!((length(&long), length(&short)), (25, 3));
    let doc = export(&p, "busy", None, Locations::Absolute).unwrap();
    let ranges = |doc: &Value| -> Vec<(f64, f64)> {
        doc["tracks"]["markers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| {
                let r = &k["marked_range"];
                (
                    r["start_time"]["value"].as_f64().unwrap(),
                    r["duration"]["value"].as_f64().unwrap(),
                )
            })
            .collect()
    };
    assert!(ranges(&doc).contains(&(20.0, 25.0)), "{doc}");
    assert!(ranges(&doc).contains(&(40.0, 3.0)), "{doc}");
    // An export ending at 30 cuts the long one there.
    let cut = export(&p, "busy", Some((0, 30)), Locations::Absolute).unwrap();
    assert!(ranges(&cut).contains(&(20.0, 10.0)), "{cut}");
    // Back: the lengths stay.
    let t = import(&doc, None).unwrap();
    let at = |frame: i64| t.markers.iter().find(|k| k.at == frame).unwrap().duration;
    assert_eq!((at(20), at(40)), (25, 3));
    // Another editor's ranged marker keeps its length too.
    let mut other = doc.clone();
    other["tracks"]["markers"] = json!([{"OTIO_SCHEMA": "Marker.2", "name": "range",
        "color": "YELLOW", "comment": "", "metadata": {},
        "marked_range": range(30.0, 12.0, p.settings.fps.as_f64())}]);
    let t = import(&other, None).unwrap();
    assert_eq!((t.markers[0].at, t.markers[0].duration), (30, 12));
}

#[test]
fn stills_and_sequences_round_trip() {
    // A still is available for one frame; a numbered sequence is an
    // ImageSequenceReference.
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
    let v1 = otio_clips(otio_track(&doc, "Video 1"));
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
        (
            &json!(format!("/renders{}", std::path::MAIN_SEPARATOR)),
            &json!("frame_"),
            &json!(".png")
        )
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

/// A timeline as an editor exports one (the shapes of real exports, cut
/// down): `Clip.2` with the editor's effects, Windows paths, a link group
/// number on each clip's video and audio, the stream in a channel map,
/// clip volume, media the editor had offline, a clip marker, timecode source
/// ranges with float noise, no timeline name.
fn editor_export() -> Value {
    let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": v});
    let r = |s: f64, d: f64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": t(s), "duration": t(d)});
    let effect = |meta: Value| json!({"OTIO_SCHEMA": "Effect.1", "name": "", "effect_name": "Resolve Effect", "metadata": {"Resolve_OTIO": meta}});
    let clip = |name: &str, source: Value, reference: Value, editor: Value, effects: Vec<Value>| {
        json!({
            "OTIO_SCHEMA": "Clip.2", "name": name, "source_range": source, "effects": effects,
            "markers": [], "enabled": true, "metadata": {"Resolve_OTIO": editor},
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
fn reads_editor_exports() {
    let t = import(&editor_export(), None).unwrap();
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
    // A Windows path: there, the path; elsewhere found by name only.
    let here = if cfg!(windows) {
        vec![PathBuf::from(r"E:\Shoot\Day 1\A001.mov")]
    } else {
        vec![]
    };
    assert_eq!(v1[0].media.paths, here);
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
        "the editor's track lock"
    );
    assert!(t.tracks.iter().any(|s| s.id == "A1" && s.solo));
    // The clip marker lands where it is in the clip; the note is the
    // editor's.
    assert_eq!(
        t.markers,
        vec![
            ImportedMarker {
                at: 10,
                duration: 1,
                name: "focus".into(),
                note: "soft".into(),
                color: "green".into()
            },
            ImportedMarker {
                at: 60,
                duration: 1,
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
    let e = f.err(req("timeline.import", json!({ "otio": editor_export() })));
    assert!(e.message.contains("E:\\Shoot\\Day 1\\A001.mov"), "{e}");
    let r = f.ok(req(
        "timeline.import",
        json!({ "otio": editor_export(), "search": ["/footage"] }),
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
    let mut doc = editor_style();
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
    // media's timecode: the media's start timecode comes off them.
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
        drop_frame: false,
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

#[test]
fn keeps_clip_effects_in_use() {
    let effect = |meta: Value| json!({"OTIO_SCHEMA": "Effect.1", "name": "", "effect_name": "Resolve Effect", "metadata": {"Resolve_OTIO": meta}});
    // The editor's stack on every clip, untouched: nothing to note.
    let zoom = |on: bool| {
        effect(
            json!({"Effect Name": "Dynamic Zoom", "Enabled": on, "Name": "Dynamic Zoom", "Type": 59,
            "Parameters": [{"Parameter ID": "dynamicZoomScale", "Default Parameter Value": 1.0, "Parameter Value": 1.0,
                "Key Frames": {"0": {"Value": 0.8, "Variant Type": "Double"}, "1000": {"Value": 1.0, "Variant Type": "Double"}},
                "Variant Type": "Double"}]}),
        )
    };
    let untouched = vec![
        effect(
            json!({"Effect Name": "Transform", "Enabled": true, "Name": "Transform", "Parameters": [], "Type": 2}),
        ),
        zoom(false),
        effect(
            json!({"Effect Name": "Fairlight Equaliser Band", "Enabled": true, "Name": "Equalizer Band", "Type": 63,
            "Parameters": [{"Parameter ID": "eq band index", "Default Parameter Value": 1, "Parameter Value": 1, "Variant Type": "Int"}]}),
        ),
    ];
    let with = |video: Vec<Value>, audio: Vec<Value>| {
        let mut doc = editor_export();
        let tracks = doc["tracks"]["children"].as_array_mut().unwrap();
        tracks[0]["children"][0]["effects"] = json!(video);
        tracks[1]["children"][0]["effects"] = json!(audio);
        import(&doc, None).unwrap()
    };
    // The first clip's kept effects, picture and sound.
    let kept = |t: &Imported| -> (Vec<String>, Vec<String>) {
        let names = |l: &[Value]| l.iter().map(effects::name).collect();
        (names(&t.clips[0].effects), names(&t.clips[0].audio_effects))
    };
    let noted = |t: &Imported| -> Vec<String> {
        t.warnings
            .iter()
            .filter(|w| w.contains("clip effect"))
            .cloned()
            .collect()
    };
    let t = with(untouched.clone(), vec![]);
    assert_eq!((kept(&t), noted(&t)), ((vec![], vec![]), vec![]));
    // The editor's clip volume is the gain: nothing to keep.
    let t = import(&editor_export(), None).unwrap();
    assert_eq!((kept(&t), t.clips[0].gain_db), ((vec![], vec![]), -17.0));

    // In use: a setting off its default, Dynamic Zoom switched on, a
    // setting that moves, and another program's effect.
    let zoomed = effect(
        json!({"Effect Name": "Transform", "Enabled": true, "Name": "Transform", "Type": 2,
        "Parameters": [{"Parameter ID": "transformationZoomX", "Default Parameter Value": 1.0, "Parameter Value": 1.25, "Variant Type": "Double"}]}),
    );
    let panned = effect(
        json!({"Effect Name": "Fairlight Clip Pan", "Enabled": true, "Name": "Pan", "Type": 72,
        "Parameters": [{"Parameter ID": "pan", "Default Parameter Value": 0.0, "Parameter Value": 0.0,
            "Key Frames": {"0": {"Value": 0.0}, "24": {"Value": -0.5}}, "Variant Type": "Double"}]}),
    );
    let blur = json!({"OTIO_SCHEMA": "Effect.1", "name": "", "effect_name": "Gaussian Blur", "metadata": {}});
    let t = with(vec![zoomed.clone(), zoom(true), blur], vec![panned]);
    assert_eq!(
        kept(&t),
        (
            vec![
                "Transform".to_string(),
                "Dynamic Zoom".into(),
                "Gaussian Blur".into()
            ],
            vec!["Pan".to_string()]
        )
    );
    assert_eq!(t.clips[0].effects[0], zoomed, "as it came");
    assert_eq!(
        noted(&t),
        [
            "clip effects kept on their clips but not shown or played: Transform, Dynamic Zoom, \
             Gaussian Blur, Pan"
        ]
    );
    // Switched off, a changed setting changes nothing.
    let off = effect(
        json!({"Effect Name": "Cropping", "Enabled": false, "Name": "Cropping", "Type": 3,
        "Parameters": [{"Parameter ID": "cropLeft", "Default Parameter Value": 0.0, "Parameter Value": 0.2}]}),
    );
    let t = with(vec![off], vec![]);
    assert_eq!((kept(&t), noted(&t)), ((vec![], vec![]), vec![]));
}

/// One of the editor's clip effects, switched on.
fn editor_effect(name: &str, shown: &str, parameters: Value) -> Value {
    json!({"OTIO_SCHEMA": "Effect.1", "name": "", "effect_name": "Resolve Effect", "metadata": {"Resolve_OTIO": {
        "Effect Name": name, "Enabled": true, "Name": shown, "Type": 1, "Parameters": parameters}}})
}

/// The effects written on the clips of OTIO track `track`.
fn written(doc: &Value, track: &str) -> Vec<Value> {
    otio_clips(otio_track(doc, track))
        .into_iter()
        .map(|c| c["effects"].clone())
        .collect()
}

/// The keyframe times of an effect's first parameter, as written.
fn key_times(effect: &Value) -> Vec<String> {
    let mut t: Vec<String> = effect["metadata"]["Resolve_OTIO"]["Parameters"][0]["Key Frames"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    t.sort_by_key(|t| t.parse::<i64>().unwrap());
    t
}

/// A parameter's value, as written.
fn param(effect: &Value, id: &str) -> Value {
    effect["metadata"]["Resolve_OTIO"]["Parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["Parameter ID"] == id)
        .map_or(Value::Null, |p| p["Parameter Value"].clone())
}

#[test]
fn clip_effects_are_carried() {
    // A clip from source frame 50, as another editor wrote it: effects in
    // use on its picture and sound, an untouched one besides.
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 50, 150);
    let mut doc = export(f.editor.project(), "fx", None, Locations::Absolute).unwrap();
    let transform = editor_effect(
        "Transform",
        "Transform",
        json!([{"Parameter ID": "transformationZoomX", "Default Parameter Value": 1.0, "Parameter Value": 1.2, "Key Frames": {}}]),
    );
    // Keyframe times count from the clip's first source frame: these are
    // on source frames 0 and 1000.
    let zoom = editor_effect(
        "Dynamic Zoom",
        "Dynamic Zoom",
        json!([{"Parameter ID": "dynamicZoomScale", "Default Parameter Value": 1.0, "Parameter Value": 1.0,
            "Key Frames": {"-50": {"Value": 0.8}, "950": {"Value": 1.0}}}]),
    );
    let faders = editor_effect(
        "Video Faders",
        "Video Faders",
        json!([{"Parameter ID": "videoFaderIn", "Default Parameter Value": 0.0, "Parameter Value": 10.0},
               {"Parameter ID": "videoFaderOut", "Default Parameter Value": 0.0, "Parameter Value": 5.0}]),
    );
    let untouched = editor_effect(
        "Cropping",
        "Cropping",
        json!([{"Parameter ID": "cropLeft", "Default Parameter Value": 0.0, "Parameter Value": 0.0}]),
    );
    let volume = editor_effect(
        "Fairlight Clip Volume and Fades",
        "Volume",
        json!([{"Parameter ID": "volume", "Default Parameter Value": 0.0, "Parameter Value": -8.0, "Key Frames": {}},
               {"Parameter ID": "faderIn", "Default Parameter Value": 0.0, "Parameter Value": 25.0},
               {"Parameter ID": "faderOut", "Default Parameter Value": 0.0, "Parameter Value": 0.0}]),
    );
    let pan = editor_effect(
        "Fairlight Clip Pan",
        "Pan",
        json!([{"Parameter ID": "pan", "Default Parameter Value": 0.0, "Parameter Value": -50.0}]),
    );
    for (track, list) in [
        ("Video 1", json!([transform, zoom, untouched, faders])),
        ("Audio 1", json!([volume, pan])),
    ] {
        let items = doc["tracks"]["children"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["name"] == track)
            .unwrap()["children"]
            .as_array_mut()
            .unwrap();
        items[0]["effects"] = list;
        items[0]["metadata"].as_object_mut().unwrap().remove(VIDCUT);
    }

    let mut g = Fixture::new(&[(10.0, true)]);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(
        r.result["warnings"],
        json!([
            "clip effects kept on their clips but not shown or played: Transform, Dynamic Zoom, \
             Video Faders, Volume, Pan"
        ])
    );
    let c = g.editor.project().clip("c1").unwrap().clone();
    let picture = c.effects.clone().unwrap();
    assert_eq!(
        (picture.from, picture.list),
        (50, vec![transform.clone(), zoom.clone(), faders.clone()])
    );
    let au = c.audio.clone().unwrap();
    let sound = au.effects.unwrap();
    assert_eq!(
        (au.gain_db, sound.from, sound.list),
        (-8.0, 50, vec![volume.clone(), pan.clone()])
    );
    // By name in the timeline.
    let r = g.ok(req("timeline.get", json!({})));
    let clip = &r.result["tracks"][0]["items"][0];
    assert_eq!(
        (&clip["effects"], &clip["audio"]["effects"]),
        (
            &json!(["Transform", "Dynamic Zoom", "Video Faders"]),
            &json!(["Volume", "Pan"])
        ),
        "{clip}"
    );
    // Written back as they came.
    let out = export(g.editor.project(), "fx", None, Locations::Absolute).unwrap();
    assert_eq!(
        (written(&out, "Video 1"), written(&out, "Audio 1")),
        (
            vec![json!([transform, zoom, faders])],
            vec![json!([volume, pan])]
        )
    );

    // A trimmed head: the keyframes stay on their source frames.
    g.ok(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "head", "by": 10}),
    ));
    assert_eq!(g.v1(), vec![crate::test_support::row("c1", 0, 90, 60)]);
    let out = export(g.editor.project(), "fx", None, Locations::Absolute).unwrap();
    let v = written(&out, "Video 1");
    assert_eq!(key_times(&v[0][1]), ["-60", "940"]);
    let fades = |e: &Value| (param(e, "videoFaderIn"), param(e, "videoFaderOut"));
    assert_eq!(fades(&v[0][2]), (json!(10.0), json!(5.0)));

    // A split: the fade-in stays on the first part, the fade-out on the
    // last. A sound fade cut away leaves the gain as a clip volume.
    g.ok(req("edit.split", json!({"at": 40})));
    g.ok(req("clip.set", json!({"clips": "c1", "gain_db": -3.0})));
    let out = export(g.editor.project(), "fx", None, Locations::Absolute).unwrap();
    let (v, a) = (written(&out, "Video 1"), written(&out, "Audio 1"));
    assert_eq!(fades(&v[0][2]), (json!(10.0), json!(0.0)));
    assert_eq!(fades(&v[1][2]), (json!(0.0), json!(5.0)));
    assert_eq!(
        key_times(&v[1][1]),
        ["-100", "900"],
        "from source frame 100"
    );
    assert_eq!(
        (param(&a[0][0], "volume"), param(&a[0][0], "faderIn")),
        (json!(-3.0), json!(25.0)),
        "the gain, and the fade kept"
    );
    assert_eq!(a[1], json!([effects::volume(-8.0), pan]));
    let p = g.editor.project();
    let right = p.clip("c2").unwrap().audio.as_ref().unwrap();
    assert_eq!(right.effects.as_ref().unwrap().names(), ["Pan"]);

    // Through vidcut's own file, the same.
    let before = p.clone();
    let mut h = Fixture::new(&[(10.0, true)]);
    let r = h.ok(req("timeline.import", json!({ "otio": out })));
    assert!(
        r.result["warnings"][0]
            .as_str()
            .unwrap()
            .starts_with("clip effects kept"),
        "{}",
        r.result
    );
    let again = export(h.editor.project(), "fx", None, Locations::Absolute).unwrap();
    assert_eq!(
        again,
        export(&before, "fx", None, Locations::Absolute).unwrap()
    );

    // Into a 50 fps project: keyframe times and fade lengths double.
    let mut k = Fixture::new(&[(10.0, true)]);
    k.ok(req("project.set", json!({"fps": "50"})));
    k.append("m1", 0, 10);
    k.ok(req("timeline.import", json!({ "otio": doc })));
    let c = k.editor.project().clip("c2").unwrap().clone();
    let e = c.effects.unwrap();
    assert_eq!(e.from, 100);
    assert_eq!(key_times(&e.list[1]), ["-100", "1900"]);
    assert_eq!(fades(&e.list[2]), (json!(20.0), json!(10.0)));
    let sound = c.audio.unwrap().effects.unwrap();
    assert_eq!(param(&sound.list[0], "faderIn"), json!(50.0));
}

/// A transition as an editor writes one: `before` and `after` frames of
/// the cut.
fn otio_transition(name: &str, kind: &str, before: f64, after: f64) -> Value {
    let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": v});
    json!({"OTIO_SCHEMA": "Transition.1", "name": name, "transition_type": kind,
        "in_offset": t(before), "out_offset": t(after),
        "metadata": {"Resolve_OTIO": {"Effects": {"Effect Name": name, "Enabled": true, "Name": name,
            "Parameters": [], "Type": 9}, "Transition Type": name}}})
}

/// The transitions written on OTIO track `track`: (name, before, after,
/// what is before it, what after), sides by clip name or `gap`.
fn written_transitions(doc: &Value, track: &str) -> Vec<(String, f64, f64, String, String)> {
    let items = otio_track(doc, track)["children"].as_array().unwrap();
    let side = |i: Option<&Value>| match i {
        Some(v) if is_clip(v) => v["name"].as_str().unwrap().to_string(),
        Some(v) if schema(v).starts_with("Gap") => "gap".to_string(),
        _ => "nothing".to_string(),
    };
    items
        .iter()
        .enumerate()
        .filter(|(_, v)| schema(v).starts_with("Transition"))
        .map(|(i, v)| {
            (
                v["name"].as_str().unwrap().to_string(),
                v["in_offset"]["value"].as_f64().unwrap(),
                v["out_offset"]["value"].as_f64().unwrap(),
                side(i.checked_sub(1).map(|i| &items[i])),
                side(items.get(i + 1)),
            )
        })
        .collect()
}

#[test]
fn transitions_are_carried() {
    // a 0..100 | b 100..200, a gap, c 220..320; each from source frame 50.
    let mut f = Fixture::new(&[(10.0, true), (10.0, true), (10.0, true)]);
    f.append("m1", 50, 150);
    f.append("m2", 50, 150);
    f.append("m3", 0, 20);
    f.append("m3", 50, 150);
    f.ok(req("edit.delete", json!({"clips": "c3", "lift": true})));
    let mut doc = export(f.editor.project(), "tr", None, Locations::Absolute).unwrap();
    // As an editor writes them: a dissolve and a sound crossfade between a
    // and b, a fade from the gap into c and one out of c into a gap.
    let dissolve = otio_transition("Cross Dissolve", "SMPTE_Dissolve", 6.0, 6.0);
    let crossfade = otio_transition("Cross Fade +3 dB", "Custom_Transition", 5.0, 5.0);
    let fade_in = otio_transition("Fade In", "SMPTE_Dissolve", 0.0, 10.0);
    let fade_out = otio_transition("Fade Out", "SMPTE_Dissolve", 10.0, 0.0);
    let gap10 = gap(10, 25.0);
    for (track, at, list) in [
        ("Video 1", 1, vec![dissolve.clone()]),
        ("Audio 1", 1, vec![crossfade.clone()]),
        ("Video 1", 4, vec![fade_in.clone()]),
        ("Video 1", 6, vec![fade_out.clone(), gap10]),
    ] {
        let items = doc["tracks"]["children"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["name"] == track)
            .unwrap()["children"]
            .as_array_mut()
            .unwrap();
        for (k, x) in list.into_iter().enumerate() {
            items.insert(at + k, x);
        }
    }
    let expected = |doc: &Value| {
        (
            written_transitions(doc, "Video 1"),
            written_transitions(doc, "Audio 1"),
        )
    };
    let sides = |n: &str, b: f64, a: f64, from: &str, to: &str| {
        (n.to_string(), b, a, from.to_string(), to.to_string())
    };
    assert_eq!(
        expected(&doc),
        (
            vec![
                sides("Cross Dissolve", 6.0, 6.0, "1.mp4", "2.mp4"),
                sides("Fade In", 0.0, 10.0, "gap", "3.mp4"),
                sides("Fade Out", 10.0, 0.0, "3.mp4", "gap"),
            ],
            vec![sides("Cross Fade +3 dB", 5.0, 5.0, "1.mp4", "2.mp4")]
        ),
        "the file as made"
    );

    let mut g = Fixture::new(&[(10.0, true), (10.0, true), (10.0, true)]);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(
        r.result["warnings"],
        json!([
            "transitions kept on their edit points but shown as cuts: Cross Dissolve, \
             Fade In, Fade Out, Cross Fade +3 dB"
        ])
    );
    let v1 = g.editor.project().track(MAIN).unwrap().clone();
    let on = |v1: &crate::model::Track| -> Vec<_> {
        v1.transitions
            .iter()
            .map(|x| (x.sound, x.from.clone(), x.to.clone(), x.before, x.after))
            .collect()
    };
    let id = |s: &str| Some(s.to_string());
    assert_eq!(
        on(&v1),
        vec![
            (false, id("c1"), id("c2"), 6, 6),
            (false, None, id("c3"), 0, 10),
            (false, id("c3"), None, 10, 0),
            (true, id("c1"), id("c2"), 5, 5),
        ]
    );
    assert_eq!(v1.transitions[0].otio, dissolve, "as it came");
    let r = g.ok(req("timeline.get", json!({})));
    let listed: Vec<(String, i64)> = r.result["tracks"][0]["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["name"].as_str().unwrap().to_string(),
                x["at"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("Cross Dissolve".to_string(), 100),
            ("Fade In".into(), 220),
            ("Fade Out".into(), 320),
            ("Cross Fade +3 dB".into(), 100)
        ]
    );
    // Written back where they were.
    let out = export(g.editor.project(), "tr", None, Locations::Absolute).unwrap();
    assert_eq!(expected(&out), expected(&doc));

    // A split of the clip before a cut hands its transition to the part
    // that ends there.
    g.ok(req("edit.split", json!({"at": 50})));
    let v1 = g.editor.project().track(MAIN).unwrap().clone();
    assert_eq!(on(&v1)[0], (false, id("c4"), id("c2"), 6, 6));
    let out = export(g.editor.project(), "tr", None, Locations::Absolute).unwrap();
    assert_eq!(expected(&out), expected(&doc));

    // A clip with too little media before its first frame for the
    // transitions into it: dropped, and the edit says so.
    let r = g.ok(req(
        "edit.trim",
        json!({"clip": "c2", "edge": "head", "by": -48}),
    ));
    assert_eq!(
        r.warnings,
        ["2 transition(s) from another editor no longer fit their edit points and were dropped"]
    );
    let out = export(g.editor.project(), "tr", None, Locations::Absolute).unwrap();
    assert_eq!(
        expected(&out),
        (
            vec![
                sides("Fade In", 0.0, 10.0, "gap", "3.mp4"),
                sides("Fade Out", 10.0, 0.0, "3.mp4", "gap"),
            ],
            vec![]
        )
    );
    // Undone, they are back.
    g.ok(req("history.undo", json!({})));
    assert_eq!(g.editor.project().track(MAIN).unwrap().transitions.len(), 4);
    // A clip moved up against the one before it: the fade from the gap
    // goes, the one into the gap after it stays.
    let r = g.ok(req(
        "edit.move",
        json!({"clips": "c3", "to": 200, "mode": "overwrite"}),
    ));
    assert_eq!(
        r.warnings,
        ["1 transition(s) from another editor no longer fit their edit points and were dropped"],
        "{:?}",
        g.editor.project().track(MAIN).unwrap()
    );

    // An export starting at a cut has nothing before it to fade from.
    g.ok(req("history.undo", json!({})));
    let part = export(
        g.editor.project(),
        "tr",
        Some((100, 330)),
        Locations::Absolute,
    )
    .unwrap();
    assert_eq!(
        written_transitions(&part, "Video 1"),
        vec![
            sides("Fade In", 0.0, 10.0, "gap", "3.mp4"),
            sides("Fade Out", 10.0, 0.0, "3.mp4", "gap"),
        ]
    );
}

/// A generator clip as an editor writes one, `frames` long.
fn otio_generator(name: &str, kind: &str, frames: f64) -> Value {
    let t = |v: f64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 25.0, "value": v});
    json!({"OTIO_SCHEMA": "Clip.2", "name": name,
        "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": t(0.0), "duration": t(frames)},
        "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "GeneratorReference.1", "name": name,
            "generator_kind": kind, "available_range": null, "available_image_bounds": null,
            "parameters": {"Resolve_OTIO": [{"Effect Name": name, "Enabled": true, "Name": "Generator",
                "Parameters": [{"Parameter ID": "color", "Parameter Value": "#000000", "Variant Type": "Color"}]}]},
            "metadata": {"Resolve_OTIO": {"Generator Type": kind}}}},
        "active_media_reference_key": "DEFAULT_MEDIA", "effects": [], "markers": [], "enabled": true,
        "metadata": {"Resolve_OTIO": {}}})
}

/// The generator clips written on OTIO track `track`: (where, length,
/// name, reference).
fn written_generators(doc: &Value, track: &str) -> Vec<(i64, i64, String, Value)> {
    let mut at = 0;
    let mut out = Vec::new();
    for item in otio_track(doc, track)["children"].as_array().unwrap() {
        if schema(item).starts_with("Transition") {
            continue;
        }
        let len = item["source_range"]["duration"]["value"].as_f64().unwrap() as i64;
        let reference = media_reference_of(item);
        if schema(reference).starts_with("GeneratorReference") {
            let name = item["name"].as_str().unwrap().to_string();
            out.push((at, len, name, reference.clone()));
        }
        at += len;
    }
    out
}

#[test]
fn generators_are_carried() {
    // As an editor writes them: a clip 0..100, a solid color 100..150 and a
    // title 150..175 after it, an empty title, and a solid color over the
    // clip on a second track (60..80).
    let mut f = Fixture::new(&[(10.0, true)]);
    f.append("m1", 0, 100);
    let mut doc = export(f.editor.project(), "gen", None, Locations::Absolute).unwrap();
    let solid = otio_generator("Solid Color", "Solid Color", 50.0);
    let title = otio_generator("Text", "Rich", 25.0);
    let mut empty = otio_generator("Text+", "", 25.0);
    empty["media_references"]["DEFAULT_MEDIA"] =
        json!({"OTIO_SCHEMA": "MissingReference.1", "name": "", "metadata": {}});
    let tracks = doc["tracks"]["children"].as_array_mut().unwrap();
    let v1 = tracks[0]["children"].as_array_mut().unwrap();
    v1.extend([solid.clone(), title.clone(), empty]);
    let mut v2 = tracks[0].clone();
    v2["name"] = json!("Video 2");
    v2["metadata"] = json!({});
    v2["children"] = json!([
        gap(60, 25.0),
        otio_generator("Solid Color", "Solid Color", 20.0)
    ]);
    tracks.insert(1, v2);

    let mut g = Fixture::new(&[(10.0, true)]);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(
        r.result["warnings"],
        json!([
            "clip \"Text+\" has no media file (MissingReference.1); left as a gap",
            "generators and titles kept as blocks but played as gaps: Solid Color, Text"
        ])
    );
    let p = g.editor.project().clone();
    assert!(crate::validate::violations(&p).is_empty());
    let shape = |p: &Project, track: &str| -> Vec<(i64, i64, String, Option<Value>)> {
        p.track(track)
            .unwrap()
            .clips
            .iter()
            .map(|c| {
                let reference = c.generator.as_ref().map(|g| g.reference.clone());
                (c.start, c.end(), c.media.clone(), reference)
            })
            .collect()
    };
    let reference = |v: &Value| Some(v["media_references"]["DEFAULT_MEDIA"].clone());
    assert_eq!(
        shape(&p, "V1"),
        vec![
            (0, 100, "m1".to_string(), None),
            (100, 150, String::new(), reference(&solid)),
            (150, 175, String::new(), reference(&title)),
        ]
    );
    assert_eq!(shape(&p, "V2").len(), 1);
    // By kind in the timeline.
    let r = g.ok(req("timeline.get", json!({})));
    let kinds: Vec<&Value> = r.result["tracks"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i.get("generator"))
        .collect();
    assert_eq!(kinds, [&json!("Solid Color"), &json!("Rich")]);
    // They play as gaps: the clip under the solid color shows through.
    let plan = crate::render::flatten(&p, 0, 175).unwrap();
    let pieces: Vec<(bool, i64)> = plan
        .video
        .iter()
        .map(|v| match v {
            crate::render::VideoPiece::Source { frames, .. } => (true, *frames),
            crate::render::VideoPiece::Black { frames } => (false, *frames),
        })
        .collect();
    assert_eq!(pieces, [(true, 100), (false, 75)]);
    // Written back as they came.
    let out = export(&p, "gen", None, Locations::Absolute).unwrap();
    let solid_ref = reference(&solid).unwrap();
    let title_ref = reference(&title).unwrap();
    assert_eq!(
        written_generators(&out, "Video 1"),
        vec![
            (100, 50, "Solid Color".to_string(), solid_ref.clone()),
            (150, 25, "Text".to_string(), title_ref.clone()),
        ]
    );
    assert_eq!(written_generators(&out, "Video 2")[0].0, 60);
    assert!(
        crate::otio::used_media(&p, None) == ["m1"],
        "no media of their own"
    );

    // Any length: a trim past where it ended, extending to the media's end
    // leaving it as it is, a split into two, a copy pasted after it (at
    // Track 1's end, with Ripple on).
    let title_id = p.track("V1").unwrap().clips[2].id.clone();
    let solid_id = p.track("V1").unwrap().clips[1].id.clone();
    g.ok(req(
        "edit.trim",
        json!({"clip": title_id, "edge": "tail", "by": 500}),
    ));
    g.ok(req(
        "edit.change_duration",
        json!({"clips": [title_id.clone()], "duration": "end"}),
    ));
    g.ok(req("edit.split", json!({"at": 120, "clips": [solid_id]})));
    let copied = g.ok(req("edit.copy", json!({"clips": [title_id]})));
    g.ok(req(
        "edit.paste",
        json!({"clips": copied.result["clips"], "at": 2000}),
    ));
    let p = g.editor.project().clone();
    assert!(crate::validate::violations(&p).is_empty());
    let v1: Vec<(i64, i64, bool)> = p
        .track("V1")
        .unwrap()
        .clips
        .iter()
        .map(|c| (c.start, c.end(), c.is_generator()))
        .collect();
    assert_eq!(
        v1,
        [
            (0, 100, false),
            (100, 120, true),
            (120, 150, true),
            (150, 675, true),
            (675, 1200, true)
        ]
    );
    let out = export(&p, "gen", None, Locations::Absolute).unwrap();
    let written = written_generators(&out, "Video 1");
    assert_eq!(
        written
            .iter()
            .map(|(at, len, _, r)| (*at, *len, r == &title_ref))
            .collect::<Vec<_>>(),
        [
            (100, 20, false),
            (120, 30, false),
            (150, 525, true),
            (675, 525, true)
        ]
    );
    // Nothing to measure in one.
    let e = g.err(req(
        "analyze.silence",
        json!({"clips": [p.track("V1").unwrap().clips[1].id]}),
    ));
    assert!(
        e.message.contains("is a generator from another editor"),
        "{e}"
    );
}

#[test]
fn numbers_read_exactly() {
    // What another editor wrote is written back as read: each number the
    // nearest double to the file's (serde_json's float_roundtrip), as the
    // standard library reads it. These are an editor's, 17 digits long.
    for text in [
        "0.037037037037037038",
        "0.052083333333333336",
        "-16.999998092651367",
    ] {
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v.as_f64(), text.parse::<f64>().ok(), "{text}");
    }
}
