//! Fixture matrix: the generator writes what we expect, and the decoder returns
//! exactly the requested frame after any seek.

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, VideoCodec};
use vidcut_av::probe;

fn matrix() -> Vec<FixtureSpec> {
    use Container::*;
    use VideoCodec::*;
    vec![
        FixtureSpec::new(H264, Mp4, 320, 180, (30000, 1001), 90),
        FixtureSpec::new(H264, Mp4, 320, 180, (25, 1), 75),
        FixtureSpec::new(H264, Mp4, 320, 180, (60, 1), 120).gop(60),
        FixtureSpec::new(H264, Mp4, 320, 180, (30, 1), 90).no_audio(),
        FixtureSpec::new(H264, Mp4, 320, 180, (30, 1), 90).vfr(),
        FixtureSpec::new(H264, MpegTs, 320, 180, (30, 1), 90).start_offset_ms(1400),
        FixtureSpec::new(Hevc10, Mkv, 320, 180, (30, 1), 90),
        FixtureSpec::new(ProRes422Hq, Mov, 320, 180, (25, 1), 50),
        FixtureSpec::new(Vp9, Webm, 320, 180, (30, 1), 60),
        FixtureSpec::new(Ffv1, Mkv, 320, 180, (24000, 1001), 48).with_audio(
            AudioCodec::Flac,
            44_100,
            1,
        ),
    ]
}

fn secs(pts: i64, tb: vidcut_av::ffmpeg::Rational) -> f64 {
    pts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
}

#[test]
fn fixtures_decode_frame_exact() {
    for spec in matrix() {
        let path = fixture::ensure(&spec).unwrap_or_else(|e| panic!("{}: {e}", spec.file_name()));
        let name = spec.file_name();

        let info = probe::probe(&path, None).unwrap();
        let v = info.video.as_ref().expect("video stream");
        assert_eq!((v.width, v.height), (spec.width, spec.height), "{name}");
        assert_eq!(
            info.audio.len(),
            usize::from(spec.audio.is_some()),
            "{name}"
        );
        if let (Some(a), Some(want)) = (info.audio.first(), spec.audio) {
            assert_eq!(
                (a.sample_rate, a.channels),
                (want.rate, want.channels),
                "{name}"
            );
        }

        // Sequential decode: every frame present, in order, with expected timing.
        let mut reader = VideoReader::open(&path).unwrap();
        let tb = reader.time_base();
        let mut first_pts = None;
        let mut k = 0u32;
        while let Some(frame) = reader.next_frame().unwrap() {
            let pts = frame.pts().expect("pts");
            let o = *first_pts.get_or_insert(pts);
            assert_eq!(
                read_barcode(&frame).unwrap(),
                Some(k),
                "{name}: barcode of frame {k}"
            );
            let got = secs(pts - o, tb);
            let want = spec.frame_time(k);
            // Containers quantize timestamps (Matroska to 1 ms), so allow one tick.
            let tolerance = secs(1, tb).max(0.5e-3) + 1e-9;
            assert!(
                (got - want).abs() <= tolerance,
                "{name}: frame {k} at {got}, want {want}"
            );
            k += 1;
        }
        assert_eq!(k, spec.frames, "{name}: frame count");
        let o = first_pts.unwrap();

        // Exact seeks, forward and backward, around keyframes and the ends.
        let last = spec.frames - 1;
        let mut targets = vec![
            last,
            0,
            1,
            spec.gop.saturating_sub(1),
            spec.gop,
            spec.gop + 1,
            last / 2,
            2,
            last - 1,
        ];
        targets.extend((0..spec.frames).step_by(7).rev());
        let half_frame = 0.5 * f64::from(spec.fps.1) / f64::from(spec.fps.0);
        let to_ticks =
            |s: f64| (s * f64::from(tb.denominator()) / f64::from(tb.numerator())).round() as i64;
        for &k in targets.iter().filter(|&&k| k <= last) {
            // The time map's rule: the first frame with pts >= o + t - b, where b
            // is half a frame. For VFR, aim half-way into the gap before frame k.
            let t = spec.frame_time(k);
            let aim = match (spec.vfr, k) {
                (true, 1..) => (t + spec.frame_time(k - 1)) / 2.0,
                _ => t - half_frame,
            };
            let target = o + to_ticks(aim);
            reader.seek(target).unwrap();
            let frame = reader
                .next_frame()
                .unwrap()
                .unwrap_or_else(|| panic!("{name}: no frame after seek to {k}"));
            assert_eq!(
                read_barcode(&frame).unwrap(),
                Some(k),
                "{name}: seek to frame {k} (target pts {target})"
            );
        }
    }
}

#[test]
fn probe_reports_first_frame_and_duration() {
    for spec in matrix() {
        let path = fixture::ensure(&spec).unwrap();
        let v = probe::probe(&path, None).unwrap().video.unwrap();
        let o = v.first_frame_time.expect("first frame");
        let want_o = f64::from(spec.start_offset_ms) / 1000.0;
        assert!(
            (o - want_o).abs() < 1e-3,
            "{}: o = {o}, want {want_o}",
            spec.file_name()
        );
        let d = v.duration_secs.expect("duration");
        assert!(
            (d - spec.duration()).abs() < 2e-3,
            "{}: duration {d}, want {}",
            spec.file_name(),
            spec.duration()
        );
    }
}

#[test]
fn probe_finds_where_primed_audio_starts() {
    // AAC (in MP4 an edit list, in Matroska a codec delay) and Opus (WebM,
    // pre-skip) put priming samples before the sound. Matroska's codec delay
    // is skipped by some FFmpeg versions and decoded by others; either way
    // the sound of an audio-only file starts at 0, its first beep included.
    use Container::*;
    for container in [Mp4, Mkv, Webm] {
        let spec =
            FixtureSpec::new(VideoCodec::H264, container, 320, 180, (30, 1), 60).audio_only();
        let path = fixture::ensure(&spec).unwrap();
        let a = probe::probe(&path, None).unwrap().audio.remove(0);
        let first = a.first_secs.expect("start");
        // Matroska and WebM count milliseconds.
        assert!(
            first.abs() < 1.5e-3,
            "{}: sound starts at {first} s (start {:?}, padding {})",
            spec.file_name(),
            a.start_secs,
            a.padding
        );
    }
}

#[test]
fn probe_reads_start_timecodes() {
    // A camera's start timecode, from a QuickTime/MP4 `tmcd` track
    // (deviation X9: timeline interchange counts sources from it). The
    // timecode track doesn't disturb decoding.
    use Container::*;
    use VideoCodec::*;
    for (spec, tc) in [
        (
            FixtureSpec::new(H264, Mov, 320, 180, (25, 1), 50).timecode("10:00:00:00"),
            "10:00:00:00",
        ),
        (
            FixtureSpec::new(H264, Mp4, 320, 180, (25, 1), 50).timecode("10:00:00:00"),
            "10:00:00:00",
        ),
        (
            FixtureSpec::new(ProRes422Hq, Mov, 320, 180, (30000, 1001), 30).timecode("01:00:00;00"),
            "01:00:00;00",
        ),
    ] {
        let path = fixture::ensure(&spec).unwrap();
        let info = probe::probe(&path, None).unwrap();
        assert_eq!(info.timecode.as_deref(), Some(tc), "{}", path.display());
        assert_eq!(probe::clock(&path).unwrap(), (Some(tc.to_string()), None));
        let mut reader = VideoReader::open(&path).unwrap();
        let mut k = 0;
        while let Some(frame) = reader.next_frame().unwrap() {
            assert_eq!(read_barcode(&frame).unwrap(), Some(k), "{}", path.display());
            k += 1;
        }
        assert_eq!(k, spec.frames, "{}", path.display());
    }
    let plain = fixture::ensure(&FixtureSpec::new(H264, Mp4, 320, 180, (25, 1), 50)).unwrap();
    assert_eq!(probe::probe(&plain, None).unwrap().timecode, None);
}

/// x264 through FFmpeg makes open GOPs with B-frames (the keyframes after
/// the first aren't IDR) and IDR keyframes without. MPEG-TS holds the
/// NAL units after start codes.
#[test]
fn open_gops_are_told_from_closed_ones() {
    use vidcut_av::decode::{Input, h264_open_gop};
    let spec =
        |container| FixtureSpec::new(VideoCodec::H264, container, 320, 180, (30, 1), 300).gop(120);
    let open = |s: FixtureSpec| h264_open_gop(&Input::File(fixture::ensure(&s).unwrap()));
    assert!(open(spec(Container::Mp4)));
    assert!(!open(spec(Container::Mp4).bframes(0)));
    assert!(open(spec(Container::MpegTs)));
}
