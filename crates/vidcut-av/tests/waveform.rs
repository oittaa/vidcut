//! Waveform peaks: the beep fixture's 10 ms tone at each second shows up in
//! the right buckets, and nothing else does.

use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, VideoCodec};
use vidcut_av::waveform::{Waveform, waveform};

#[test]
fn beeps_land_in_their_buckets() {
    let path = fixture::ensure(
        &FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::Flac,
            48_000,
            2,
        ),
    )
    .unwrap();
    let w = waveform(&path, 0, 100).unwrap();
    assert!(w.start.abs() < 0.001, "{}", w.start);
    assert!(
        (299..=301).contains(&w.peaks.len()),
        "{} buckets",
        w.peaks.len()
    );
    let loud: Vec<usize> = w
        .peaks
        .iter()
        .enumerate()
        .filter(|(_, (lo, hi))| *hi > 40 && *lo < -40)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(loud, vec![0, 100, 200], "beeps at 0 s, 1 s, 2 s");
    assert_eq!(w.range(0.5, 0.6), Some((0.0, 0.0)));
    let (lo, hi) = w.range(0.0, 0.01).unwrap();
    assert!(lo < -0.3 && hi > 0.3);
    assert_eq!(Waveform::from_bytes(&w.to_bytes()), Some(w));
}
