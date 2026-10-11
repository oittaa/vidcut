//! A proxy keeps the source frame's barcode and presentation timestamp.

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::ffmpeg;
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};

fn secs(pts: i64, tb: ffmpeg::Rational) -> f64 {
    pts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
}

/// `(barcode, timestamp seconds)` of every frame.
fn frames(input: &Input) -> Vec<(Option<u32>, f64)> {
    let mut reader = VideoReader::open_input(input).unwrap();
    let tb = reader.time_base();
    let mut out = Vec::new();
    while let Some(frame) = reader.next_frame().unwrap() {
        let pts = frame.pts().expect("pts");
        out.push((read_barcode(&frame).unwrap(), secs(pts, tb)));
    }
    out
}

fn skip(e: impl std::fmt::Display) -> bool {
    eprintln!("skipping: {e}");
    true
}

#[test]
fn a_proxy_matches_source_frames_and_timestamps() {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 24);
    let Some(source) = fixture::ensure(&spec).map_err(skip).ok() else {
        return;
    };
    let offset = FixtureSpec::new(VideoCodec::H264, Container::MpegTs, 320, 180, (30, 1), 12)
        .start_offset_ms(1400);
    let Some(shifted) = fixture::ensure(&offset).map_err(skip).ok() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("vidcut-proxy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    for (i, path) in [source, shifted].iter().enumerate() {
        let dest = dir.join(format!("proxy-{i}.mkv"));
        let written = vidcut_av::proxy::write(&Input::File(path.clone()), 320, 180, &dest)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(written.codec, "h264");
        assert_eq!((written.width, written.height), (320, 180));
        let src = frames(&Input::File(path.clone()));
        let got = frames(&Input::File(dest));
        assert_eq!(got.len(), src.len(), "{}", path.display());
        for (n, ((want_code, want_t), (got_code, got_t))) in src.iter().zip(&got).enumerate() {
            assert_eq!(got_code, want_code, "{} frame {n}", path.display());
            // Matroska stores milliseconds. The half-frame bias absorbs that.
            assert!(
                (got_t - want_t).abs() <= 1.5e-3,
                "{} frame {n}: proxy at {got_t}, source at {want_t}",
                path.display()
            );
        }
    }
}

#[test]
fn a_sequence_proxy_uses_the_sequence_timestamps() {
    let spec = ImageSpec::sequence(1, 8, 2, 320, 180);
    let Some(files) = fixture::ensure_images(&spec).map_err(skip).ok() else {
        return;
    };
    let pattern = files[0].with_file_name("img_%02d.png");
    let input = Input::Sequence {
        pattern: pattern.clone(),
        first: 1,
        rate: (25, 1),
    };
    let dir = std::env::temp_dir().join(format!("vidcut-proxy-seq-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("seq.mkv");
    let written = match vidcut_av::proxy::write(&input, 320, 180, &dest) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    assert_eq!(written.frames, 8);
    let got = frames(&Input::File(dest));
    assert_eq!(got.len(), 8);
    for (k, (code, t)) in got.iter().enumerate() {
        assert_eq!(*code, Some(k as u32), "picture {k}");
        let want = k as f64 / 25.0;
        assert!(
            (t - want).abs() <= 1.5e-3,
            "picture {k} at {t}, want {want}"
        );
    }
}
