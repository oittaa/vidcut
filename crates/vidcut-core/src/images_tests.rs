//! Stills and image sequences (M9). Each test cites the manual page or
//! decision it checks. Media: m1 = 10 s of video with audio (25 fps).

use std::path::PathBuf;

use serde_json::json;

use crate::editor::{Editor, History};
use crate::error::ErrorCode;
use crate::model::{Project, Settings};
use crate::render::{Image, VideoPiece, flatten};
use crate::test_support::{Fixture, media_info, row, still_info};
use crate::time::Rational;

fn req(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ "method": method, "params": params })
}

/// A fixture with a 4000×3000 still at `/media/photo.png`, added as m2.
fn with_still() -> Fixture {
    let mut f = Fixture::new(&[(10.0, true)]);
    f.env = std::mem::take(&mut f.env).with("/media/photo.png", still_info(4000, 3000));
    f.ok(req("media.add", json!({"paths": ["/media/photo.png"]})));
    f
}

#[test]
fn a_still_lasts_the_still_duration_and_as_long_as_an_edit_makes_it() {
    // p. 114: edited without a range a still lasts the standard still
    // duration (5 s); p. 1194: it can be made up to 17 h 40 min long.
    let mut f = with_still();
    let p = f.editor.project();
    let m = p.media("m2").unwrap();
    assert!(m.is_still());
    assert_eq!(p.media_frames(m), 125, "5 s at 25 fps");
    assert_eq!(p.media_limit(m), 63_600 * 25);
    f.ok(req("edit.append", json!({"media": "m2"})));
    f.ok(req(
        "edit.append",
        json!({"media": "m2", "range": "0f..+1m"}),
    ));
    assert_eq!(f.v1(), vec![row("c1", 0, 125, 0), row("c2", 125, 1500, 0)]);
    // A trim makes it longer still; past the limit it stops.
    f.ok(req(
        "edit.trim",
        json!({"clip": "c1", "edge": "tail", "by": 250}),
    ));
    assert_eq!(f.v1()[0], row("c1", 0, 375, 0));
    let e = f.err(req(
        "edit.append",
        json!({"media": "m2", "range": format!("0f..{}f", 63_600 * 25 + 1)}),
    ));
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
}

#[test]
fn the_still_duration_is_a_project_setting() {
    // Decision D38: `project.set still_duration`, here 3 s.
    let mut f = with_still();
    f.ok(req("project.set", json!({"still_duration": "3s"})));
    f.ok(req("edit.append", json!({"media": "m2"})));
    assert_eq!(f.v1(), vec![row("c1", 0, 75, 0)]);
    let e = f.err(req("project.set", json!({"still_duration": "0f"})));
    assert_eq!(e.code, ErrorCode::InvalidEdit, "{e}");
}

#[test]
fn a_still_leaves_the_project_format_to_video() {
    // Decision D37: the first video sets the format; a photo has no frame
    // rate, and its size is a photo's.
    let mut f = Fixture::new(&[]);
    f.editor = Editor::new(Project::new(Settings::default()), History::default());
    f.env = std::mem::take(&mut f.env)
        .with("/media/photo.png", still_info(4000, 3000))
        .with(
            "/media/cam.mp4",
            media_info(10.0, Rational::new(25, 1).unwrap(), true),
        );
    f.ok(req("media.add", json!({"paths": ["/media/photo.png"]})));
    let s = &f.editor.project().settings;
    assert!(s.auto_format);
    assert_eq!((s.width, s.fps), (1920, Rational::new(30, 1).unwrap()));
    f.ok(req("media.add", json!({"paths": ["/media/cam.mp4"]})));
    let s = &f.editor.project().settings;
    assert!(!s.auto_format);
    assert_eq!(s.fps, Rational::new(25, 1).unwrap());
}

#[test]
fn numbered_images_make_one_clip_at_a_rate() {
    // p. 366: a numbered sequence is one clip; decision D39: added with
    // `sequence`, at the project's rate unless `fps` says otherwise.
    let mut f = Fixture::new(&[(10.0, true)]);
    f.env = std::mem::take(&mut f.env)
        .with_sequence("/renders", "frame_%04d.png", 100, 50)
        .with_sequence("/scans", "s%d.exr", 1, 25);
    let r = f.ok(req(
        "media.add",
        json!({"paths": ["/renders/frame_0100.png", "/renders/frame_0120.png"], "sequence": true}),
    ));
    assert_eq!(
        r.result["media"],
        json!(["m2"]),
        "two of its files, one clip"
    );
    let r = f.ok(req(
        "media.add",
        json!({"paths": ["/scans/s7.exr"], "sequence": true, "fps": "50"}),
    ));
    assert_eq!(r.result["media"], json!(["m3"]));
    let p = f.editor.project();
    let m2 = p.media("m2").unwrap();
    assert_eq!(m2.name, "frame_[0100-0149].png");
    assert_eq!(m2.path, PathBuf::from("/renders/frame_0100.png"));
    assert_eq!(
        p.media_frames(m2),
        50,
        "50 pictures at the project's 25 fps"
    );
    let m3 = p.media("m3").unwrap();
    assert_eq!(
        p.media_frames(m3),
        12,
        "25 pictures at 50 fps: half a second"
    );
    let e = f.err(req(
        "media.add",
        json!({"paths": ["/renders/frame_0100.png"], "fps": "50"}),
    ));
    assert_eq!(e.code, ErrorCode::Usage, "{e}");
}

#[test]
fn plans_say_how_pictures_are_read() {
    // The export and the preview read stills and sequences their own way.
    let mut f = with_still();
    f.env = std::mem::take(&mut f.env).with_sequence("/renders", "frame_%04d.png", 100, 50);
    f.ok(req(
        "media.add",
        json!({"paths": ["/renders/frame_0100.png"], "sequence": true}),
    ));
    f.append("m1", 0, 10);
    f.append("m2", 0, 10);
    f.append("m3", 5, 10);
    let plan = flatten(f.editor.project(), 0, 25).unwrap();
    let images: Vec<Option<Image>> = plan
        .video
        .iter()
        .map(|v| match v {
            VideoPiece::Source { image, .. } => image.clone(),
            VideoPiece::Black { .. } => None,
        })
        .collect();
    assert_eq!(
        images,
        vec![
            None,
            Some(Image::Still),
            Some(Image::Sequence {
                pattern: "/renders/frame_%04d.png".into(),
                first: 100,
                count: 50,
                rate: Rational::new(25, 1).unwrap(),
            }),
        ]
    );
}

#[test]
fn stills_and_sequences_travel_through_otio() {
    // Deviation X9: a sequence as OTIO's ImageSequenceReference, a still as
    // a file with no range of its own; both come back as they were.
    let mut f = with_still();
    f.env = std::mem::take(&mut f.env).with_sequence("/renders", "frame_%04d.png", 100, 50);
    f.ok(req(
        "media.add",
        json!({"paths": ["/renders/frame_0100.png"], "sequence": true, "fps": "50"}),
    ));
    f.ok(req(
        "edit.append",
        json!({"media": "m2", "range": "0f..+2s"}),
    ));
    f.ok(req(
        "edit.append",
        json!({"media": "m3", "range": "4f..+10f"}),
    ));
    let before = f.editor.project().clone();
    let doc = crate::otio::export(&before, "images", None).unwrap();
    let clips = doc["tracks"]["children"][0]["children"].as_array().unwrap();
    let still = &clips[0]["media_references"]["DEFAULT_MEDIA"];
    assert_eq!(still["target_url"], "file:///media/photo.png");
    assert_eq!(still["available_range"], serde_json::Value::Null);
    let seq = &clips[1]["media_references"]["DEFAULT_MEDIA"];
    assert_eq!(seq["OTIO_SCHEMA"], "ImageSequenceReference.1");
    assert_eq!(
        (
            &seq["target_url_base"],
            &seq["name_prefix"],
            &seq["name_suffix"],
            &seq["start_frame"],
            &seq["frame_zero_padding"],
            &seq["rate"],
        ),
        (
            &json!("file:///renders/"),
            &json!("frame_"),
            &json!(".png"),
            &json!(100),
            &json!(4),
            &json!(50.0),
        )
    );
    // Into an empty project with the same files.
    let mut g = Fixture::new(&[]);
    g.env = std::mem::take(&mut f.env);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
    let p = g.editor.project();
    let kinds: Vec<_> = p
        .track("V1")
        .unwrap()
        .clips
        .iter()
        .map(|c| {
            let m = p.media(&c.media).unwrap();
            (m.is_still(), m.sequence.clone(), c.duration, c.source_in)
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            (true, None, 50, 0),
            (
                false,
                Some(crate::model::ImageSequence {
                    pattern: "frame_%04d.png".into(),
                    first: 100,
                    count: 50,
                }),
                10,
                4
            ),
        ]
    );
    let seq = p.media(&p.track("V1").unwrap().clips[1].media).unwrap();
    assert_eq!(
        seq.info.video.as_ref().unwrap().fps,
        Rational::new(50, 1).unwrap()
    );
}

/// OpenTimelineIO's `RationalTime.to_frames(rate)`: rescale to `rate`, then
/// truncate toward zero. `ImageSequenceReference` counts pictures that way,
/// passing its own `rate`.
fn otio_to_frames(value: f64, value_rate: f64, rate: f64) -> i64 {
    let scaled = if value_rate == rate {
        value
    } else {
        value * rate / value_rate
    };
    scaled.trunc() as i64
}

#[test]
fn sequence_available_range_counts_pictures_at_the_sequence_rate() {
    // 100 pictures at 24 fps in a 25 fps project. Their timeline length is
    // 104 frames at 25 fps; stored as the available range, `to_frames(24)`
    // truncates to 99 pictures (one picture can come out as none). The
    // range has to be the picture count at the sequence rate. A clip's
    // source range stays the timeline length at the project rate, and
    // reimport still takes the picture count from the directory.
    let mut f = Fixture::new(&[]);
    f.env = std::mem::take(&mut f.env).with_sequence("/renders", "frame_%04d.png", 1, 100);
    f.ok(req(
        "media.add",
        json!({"paths": ["/renders/frame_0001.png"], "sequence": true, "fps": "24"}),
    ));
    f.ok(req(
        "edit.append",
        json!({"media": "m1", "range": "0f..+10f"}),
    ));
    let before = f.editor.project().clone();
    let clip_len = before.track("V1").unwrap().clips[0].duration;
    let doc = crate::otio::export(&before, "seq", None).unwrap();
    let clip = &doc["tracks"]["children"][0]["children"][0];
    let seq = &clip["media_references"]["DEFAULT_MEDIA"];
    assert_eq!(seq["OTIO_SCHEMA"], "ImageSequenceReference.1");
    let sequence_rate = seq["rate"].as_f64().unwrap();
    assert_eq!(sequence_rate, 24.0);
    let duration = &seq["available_range"]["duration"];
    assert_eq!(
        otio_to_frames(
            duration["value"].as_f64().unwrap(),
            duration["rate"].as_f64().unwrap(),
            sequence_rate,
        ),
        100
    );
    let source = &clip["source_range"]["duration"];
    assert_eq!(
        (source["value"].as_f64(), source["rate"].as_f64()),
        (Some(clip_len as f64), Some(before.settings.fps.as_f64()))
    );

    let mut g = Fixture::new(&[]);
    g.env = std::mem::take(&mut f.env);
    let r = g.ok(req("timeline.import", json!({ "otio": doc })));
    assert_eq!(r.result["warnings"], json!([]), "{}", r.result);
    let p = g.editor.project();
    let c = &p.track("V1").unwrap().clips[0];
    let m = p.media(&c.media).unwrap();
    assert_eq!(c.duration, clip_len);
    assert_eq!(m.sequence.as_ref().map(|s| s.count), Some(100));
    assert_eq!(
        m.info.video.as_ref().unwrap().fps,
        Rational::new(24, 1).unwrap()
    );
}
