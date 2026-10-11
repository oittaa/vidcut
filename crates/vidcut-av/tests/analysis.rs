//! Media measurements (M7) on generated fixtures: the beeps in the audio
//! levels, scene scores and black shares of shots, and the loudness of a
//! tone whose level BS.1770 defines.

use std::path::Path;

use vidcut_av::analysis;
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture, Sound, VideoCodec};
use vidcut_core::render::{AudioPiece, RenderPlan};
use vidcut_core::time::Rational;

fn spec(frames: u32) -> FixtureSpec {
    FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), frames)
}

#[test]
fn levels_show_the_beeps_and_nothing_else() {
    // A 10 ms beep at the start of every second, silence between.
    let path = fixture::ensure(&spec(90)).unwrap();
    let l = analysis::audio_levels(&path, 0, 100).unwrap();
    assert_eq!(l.per_second, 100);
    assert!((299..=302).contains(&l.peaks.len()), "{}", l.peaks.len());
    let at = |secs: f64| ((secs - l.start) * 100.0).round() as usize;
    for s in 0..3 {
        let i = at(f64::from(s));
        let peak = l.peaks[i..i + 2].iter().fold(0f32, |m, &p| m.max(p));
        assert!(peak > 0.3, "beep at {s} s: {peak}");
    }
    // Between beeps: below -40 dBFS (AAC leaves a little noise by them).
    let quiet = l.peaks[at(0.1)..at(0.9)]
        .iter()
        .fold(0f32, |m, &p| m.max(p));
    assert!(quiet < 0.01, "{quiet}");
}

#[test]
fn shots_score_at_their_cuts_and_black_reads_black() {
    let path =
        fixture::ensure(&spec(150).picture(Picture::Shots { frames: 30 }).no_audio()).unwrap();
    let s = analysis::picture_stats(&path).unwrap();
    assert_eq!(s.pts.len(), 150);
    for k in [30, 60, 90, 120] {
        assert!(s.score[k] >= 20.0, "cut at {k}: {}", s.score[k]);
    }
    let within = (1..150)
        .filter(|k| k % 30 != 0)
        .map(|k| s.score[k])
        .fold(0f32, f32::max);
    assert!(within < 5.0, "inside shots: {within}");
    // The fourth shot (frames 90..120) is black; the others aren't.
    assert!(s.black[100] > 0.99, "{}", s.black[100]);
    assert!(s.black[10] < 0.01, "{}", s.black[10]);
    assert!((s.pts[1] - s.pts[0] - 1.0 / 30.0).abs() < 1e-3);
}

/// A plan that plays `path`'s audio from its start, `secs` long.
fn plan(path: &Path, secs: i64, gain_db: f64) -> RenderPlan {
    let info = vidcut_av::probe::probe(path).unwrap();
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
            media: "m1".into(),
            path: path.to_path_buf(),
            stream: 0,
            origin,
            source_sample: 0,
            samples: secs * 48_000,
            gain_db,
        }]],
        range: (0, secs * 30),
        name: "tone".into(),
    }
}

#[test]
fn a_tone_at_minus_20_dbfs_measures_minus_20_lufs() {
    // BS.1770: a 1 kHz sine in both channels reads its level in LUFS.
    let path = fixture::ensure(&spec(300).sound(Sound::Tone)).unwrap();
    let l = analysis::loudness(&plan(&path, 10, 0.0)).unwrap();
    assert!((l.integrated + 20.0).abs() < 0.3, "{l:?}");
    assert!((l.true_peak + 20.0).abs() < 0.5, "{l:?}");
    assert!(l.range < 1.0, "a steady tone has no range: {l:?}");
    // Gain moves it one for one.
    let louder = analysis::loudness(&plan(&path, 10, 6.0)).unwrap();
    assert!(
        (louder.integrated - l.integrated - 6.0).abs() < 0.1,
        "{louder:?}"
    );
    // Silence is -inf.
    let quiet = fixture::ensure(&spec(90).sound(Sound::Silence)).unwrap();
    let s = analysis::loudness(&plan(&quiet, 3, 0.0)).unwrap();
    assert_eq!(s.integrated, f64::NEG_INFINITY, "{s:?}");
}
