//! Waveform peaks: the beep fixture's 10 ms tone at each second shows up in
//! the right buckets, and nothing else does.

use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, Spoil, VideoCodec};
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

#[test]
fn a_lost_packet_keeps_the_time() {
    // An AAC packet before the 2 s beep is missing (21 ms): the beep stays
    // in its bucket.
    let path = fixture::ensure(&FixtureSpec::new(
        VideoCodec::H264,
        Container::Mkv,
        320,
        180,
        (30, 1),
        90,
    ))
    .unwrap();
    let audio = vidcut_av::probe::probe(&path).unwrap().audio[0].index;
    let dir = std::env::temp_dir().join(format!("vidcut-waveform-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lossy = dir.join("lossy.mkv");
    fixture::spoil(&path, &lossy, audio, 80, Spoil::Drop).unwrap();
    let w = waveform(&lossy, 0, 100).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let loud: Vec<usize> = w
        .peaks
        .iter()
        .enumerate()
        .filter(|(_, (lo, hi))| *hi > 40 && *lo < -40)
        .map(|(i, _)| i)
        .collect();
    // Buckets count from the first sample, before 0 s here (AAC priming).
    let at = |secs: f64| ((secs - w.start) * 100.0).floor() as usize;
    assert_eq!(
        loud.iter().find(|&&i| i > at(1.5)),
        Some(&at(2.0)),
        "beeps at {loud:?}"
    );
}
