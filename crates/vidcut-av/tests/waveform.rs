//! Waveform peaks: the beep fixture's 10 ms tone at each second shows up in
//! the right buckets, and nothing else does.

use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, Spoil, VideoCodec};
use vidcut_av::waveform::waveform;

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
    let w = waveform(&path, 0, 100, 2).unwrap();
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
    let left = w.channel_peak(0, 0.0, 0.01);
    let right = w.channel_peak(1, 0.0, 0.01);
    assert!(
        left > 0.3 && right > 0.3,
        "both channels beep: L {left} R {right}"
    );
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
    let audio = vidcut_av::probe::probe(&path, None).unwrap().audio[0].index;
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-waveform-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let lossy = dir.join("lossy.mkv");
    fixture::spoil(&path, &lossy, audio, 80, Spoil::Drop).unwrap();
    let w = waveform(&lossy, 0, 100, 2).unwrap();
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

/// A 5.1 WAV (FL FR FC LFE SL SR) with a full-scale-half tone in the centre
/// channel only.
fn centre_only_wav(path: &std::path::Path) {
    let (rate, channels, secs) = (48_000u32, 6u16, 1u32);
    let frames = rate * secs;
    let mut data = Vec::with_capacity(frames as usize * usize::from(channels) * 2);
    for i in 0..frames {
        let tone = (f64::from(i) * 2.0 * std::f64::consts::PI * 1000.0 / f64::from(rate)).sin();
        for c in 0..channels {
            let v = if c == 2 { (tone * 16_384.0) as i16 } else { 0 };
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let block = channels * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    wav.extend_from_slice(&block.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).unwrap();
}

#[test]
fn a_centre_channel_shows_on_a_stereo_meter() {
    // The meters show the project's channels, mixed as the export mixes:
    // 5.1 dialogue in the centre is in both of a stereo meter's channels.
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-waveform-51-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    let path = dir.join("centre.wav");
    centre_only_wav(&path);
    let w = waveform(&path, 0, 100, 2).unwrap();
    assert_eq!(w.channels, 2);
    let (left, right) = (w.channel_peak(0, 0.2, 0.8), w.channel_peak(1, 0.2, 0.8));
    // Half scale, mixed in at -3 dB.
    for level in [left, right] {
        assert!((0.3..0.4).contains(&level), "L {left} R {right}");
    }
}
