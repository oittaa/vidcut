//! Preview tail patches: finding the frames mpv's EDL would drop before a
//! keyframe with leading pictures, and rendering exact stand-ins.

use std::path::PathBuf;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::tails::{lost_before, render_patch};
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

fn fixture(w: u32, h: u32) -> PathBuf {
    fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mp4,
        w,
        h,
        (30, 1),
        300,
    ))
    .unwrap()
}

#[test]
fn finds_frames_decoded_after_the_next_keyframe() {
    // In this encode the keyframe at frame 90 is decoded before frames 88
    // and 89 (x264 chose B-frames there); a cut before 90 loses them in mpv.
    let open_gop = fixture(640, 360);
    let end = |frame: f64| frame / 30.0 - 0.5 / 30.0; // EDL segment end before `frame`
    let lost = lost_before(&open_gop, end(90.0)).unwrap();
    assert!(
        lost.is_some_and(|t| (t - 88.0 / 30.0).abs() < 1e-6),
        "{lost:?}"
    );
    // Cutting earlier (before 88) or mid-GOP loses nothing.
    assert_eq!(lost_before(&open_gop, end(88.0)).unwrap(), None);
    assert_eq!(lost_before(&open_gop, end(75.0)).unwrap(), None);
    // At the end of the file there is no keyframe after the cut.
    assert_eq!(lost_before(&open_gop, end(300.0)).unwrap(), None);
}

#[test]
fn patches_are_exact_lossless_frames_from_zero() {
    let src = fixture(640, 360);
    let fps = Rational::new(30, 1).unwrap();
    let piece = VideoPiece::Source {
        media: "m1".into(),
        path: src,
        origin: 0.0,
        source_frame: 87,
        frames: 3,
        image: None,
    };
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-tails-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let out = dir.join("patch.mkv");
    render_patch(&piece, fps, 640, 360, &out).unwrap();
    let mut r = VideoReader::open(&out).unwrap();
    let tb = r.time_base();
    let mut got = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        let secs = f.pts().unwrap() as f64 * f64::from(tb);
        got.push((read_barcode(&f).unwrap(), (secs * 30.0).round() as i64));
    }
    assert_eq!(got, vec![(Some(87), 0), (Some(88), 1), (Some(89), 2)]);
}
