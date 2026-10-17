use std::path::Path;

use vidcut_av::analysis;
use vidcut_av::fixture::{self, Container, FixtureSpec, Sound, Spoil, VideoCodec};
use vidcut_core::render::{AudioPiece, RenderPlan};
use vidcut_core::time::Rational;

fn spec(frames: u32) -> FixtureSpec {
    FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), frames)
}

#[test]
fn levels_keep_time_across_a_lost_packet() {
    let path = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ))
    .unwrap();
    let audio = vidcut_av::probe::probe(&path, None).unwrap().audio[0].index;
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-levels-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let lossy = dir.join("lossy.mkv");
    fixture::spoil(&path, &lossy, audio, 80, Spoil::Drop).unwrap();
    let l = analysis::audio_levels(&lossy, 0, 1000).unwrap();
    let at = |secs: f64| ((secs - l.start) * 1000.0).round() as usize;
    for s in 0..3 {
        let i = at(f64::from(s));
        let peak = l.peaks[i..i + 2].iter().fold(0f32, |m, &p| m.max(p));
        assert!(peak > 0.3, "beep at {s} s: {peak}");
    }
}

fn plan(path: &Path, secs: i64, gain_db: f64) -> RenderPlan {
    let info = vidcut_av::probe::probe(path, None).unwrap();
    let origin = info.video.and_then(|v| v.first_frame_time).unwrap_or(0.0);
    let fps = Rational::new(30, 1).unwrap();
    RenderPlan {
        fps,
        width: 320,
        height: 180,
        sample_rate: 48_000,
        channels: 2,
        frames: secs * 30,
        video: vec![],
        audio: vec![vec![AudioPiece::Source {
            speed: 1.0,
            media: "m1".into(),
            path: path.to_path_buf(),
            stream: 0,
            origin,
            source_sample: 0,
            samples: secs * 48_000,
            gain_db,
            fade: None,
        }]],
        range: (0, secs * 30),
        name: "tone".into(),
    }
}

#[test]
fn a_tone_at_minus_20_dbfs_measures_minus_20_lufs() {
    let path = fixture::ensure(&spec(300).sound(Sound::Tone)).unwrap();
    let l = analysis::loudness(&plan(&path, 10, 0.0)).unwrap();
    assert!((l.integrated + 20.0).abs() < 0.3, "{l:?}");
    assert!((l.true_peak + 20.0).abs() < 0.5, "{l:?}");
    assert!(l.range < 1.0, "a steady tone has no range: {l:?}");
    let louder = analysis::loudness(&plan(&path, 10, 6.0)).unwrap();
    assert!(
        (louder.integrated - l.integrated - 6.0).abs() < 0.1,
        "{louder:?}"
    );
    let quiet = fixture::ensure(&spec(90).sound(Sound::Silence)).unwrap();
    let s = analysis::loudness(&plan(&quiet, 3, 0.0)).unwrap();
    assert_eq!(s.integrated, f64::NEG_INFINITY, "{s:?}");
}
