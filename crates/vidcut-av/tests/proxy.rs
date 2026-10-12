//! A proxy keeps the source frame's barcode and presentation timestamp,
//! with each encoder proxies can be made with (decision D41).

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::export::encode::Availability;
use vidcut_av::ffmpeg;
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
use vidcut_av::proxy::{ProxyEncoder, encoders};
use vidcut_core::preferences::ProxyCodec;

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

/// The software encoders: x264, and SVT-AV1 when this FFmpeg has it.
fn software() -> Vec<ProxyEncoder> {
    let none = Availability::default();
    let mut out = encoders(ProxyCodec::H264, &none);
    out.extend(encoders(ProxyCodec::Av1, &none));
    out
}

/// Each frame's barcode and timestamp match the source's.
fn matches_source(enc: &ProxyEncoder) {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 24);
    let Some(source) = fixture::ensure(&spec).map_err(skip).ok() else {
        return;
    };
    let offset = FixtureSpec::new(VideoCodec::H264, Container::MpegTs, 320, 180, (30, 1), 12)
        .start_offset_ms(1400);
    let Some(shifted) = fixture::ensure(&offset).map_err(skip).ok() else {
        return;
    };
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    for (i, path) in [source, shifted].iter().enumerate() {
        let dest = dir.join(format!("proxy-{i}.mkv"));
        let written = vidcut_av::proxy::write(
            &Input::File(path.clone()),
            320,
            180,
            &dest,
            None,
            std::slice::from_ref(enc),
        )
        .unwrap_or_else(|e| panic!("{} with {}: {e}", path.display(), enc.name));
        assert_eq!(written.encoder, *enc);
        assert_eq!((written.width, written.height), (320, 180));
        let src = frames(&Input::File(path.clone()));
        let got = frames(&Input::File(dest));
        assert_eq!(got.len(), src.len(), "{} {}", enc.name, path.display());
        for (n, ((want_code, want_t), (got_code, got_t))) in src.iter().zip(&got).enumerate() {
            assert_eq!(
                got_code,
                want_code,
                "{} {} frame {n}",
                enc.name,
                path.display()
            );
            // Matroska stores milliseconds. The half-frame bias absorbs that.
            assert!(
                (got_t - want_t).abs() <= 1.5e-3,
                "{} {} frame {n}: proxy at {got_t}, source at {want_t}",
                enc.name,
                path.display()
            );
        }
    }
}

/// Keyframes every 60 frames as it is set up, not where the source's are,
/// and every frame coded in the order it shows (nothing ahead of a seek).
fn keyframes_and_order(enc: &ProxyEncoder) {
    // Keyframes every 30 frames, and B-frames.
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 150);
    let Some(source) = fixture::ensure(&spec).map_err(skip).ok() else {
        return;
    };
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-keys-")
        .tempdir()
        .unwrap();
    let dest = tmp.path().join("proxy.mkv");
    vidcut_av::proxy::write(
        &Input::File(source),
        320,
        180,
        &dest,
        None,
        std::slice::from_ref(enc),
    )
    .unwrap();
    let mut input = ffmpeg::format::input(&dest).unwrap();
    let video = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .unwrap()
        .index();
    let packets: Vec<(bool, i64)> = input
        .packets()
        .filter(|(s, _)| s.index() == video)
        .map(|(_, p)| (p.is_key(), p.pts().unwrap_or(-1)))
        .collect();
    let keys: Vec<usize> = (0..packets.len()).filter(|&n| packets[n].0).collect();
    assert_eq!(keys, [0, 60, 120], "{}", enc.name);
    assert!(
        packets.windows(2).all(|w| w[0].1 < w[1].1),
        "{}: frames coded out of order",
        enc.name
    );
}

#[test]
fn a_proxy_matches_source_frames_and_timestamps() {
    for enc in software() {
        matches_source(&enc);
    }
}

#[test]
fn a_proxy_keeps_its_own_keyframes_in_showing_order() {
    for enc in software() {
        keyframes_and_order(&enc);
    }
}

/// With a working hardware AV1 encoder, the same: automatic picks it.
#[cfg(feature = "gpu-tests")]
#[test]
fn hardware_av1_proxies_are_exact_too() {
    let available = vidcut_av::export::encode::detect();
    let auto = encoders(ProxyCodec::Auto, &available);
    let hardware: Vec<&ProxyEncoder> = auto.iter().filter(|e| e.codec == "av1").collect();
    assert!(!hardware.is_empty(), "no hardware AV1 encoder: {auto:?}");
    for enc in hardware {
        matches_source(enc);
        keyframes_and_order(enc);
    }
}

#[test]
fn automatic_is_x264_without_hardware_av1() {
    let none = Availability::default();
    let auto = encoders(ProxyCodec::Auto, &none);
    assert_eq!(
        auto.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["libx264"]
    );
    assert_eq!(auto[0].label(), "H.264, software (x264)");
    let av1 = encoders(ProxyCodec::Av1, &none);
    if let Some(svt) = av1.first() {
        assert_eq!(svt.label(), "AV1, software (SVT-AV1)");
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
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-seq-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    for enc in software() {
        let dest = dir.join(format!("seq-{}.mkv", enc.name));
        let written = match vidcut_av::proxy::write(
            &input,
            320,
            180,
            &dest,
            None,
            std::slice::from_ref(&enc),
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        assert_eq!(written.frames, 8);
        let got = frames(&Input::File(dest));
        assert_eq!(got.len(), 8, "{}", enc.name);
        for (k, (code, t)) in got.iter().enumerate() {
            assert_eq!(*code, Some(k as u32), "{} picture {k}", enc.name);
            let want = k as f64 / 25.0;
            assert!(
                (t - want).abs() <= 1.5e-3,
                "{} picture {k} at {t}, want {want}",
                enc.name
            );
        }
    }
}

/// With NVENC, sources the GPU shows as the CPU would are decoded on NVDEC
/// and scaled there, frames never leaving it: each proxy frame is still
/// the source frame with its timestamp (interlaced ones deinterlaced one
/// frame per frame, as the CPU reader does; BT.601 ones converted by
/// libplacebo). Turned sources, interlaced ones in BT.601, and ones NVDEC
/// can't decode, are made on the CPU, still with NVENC.
#[cfg(feature = "gpu-tests")]
#[test]
fn nvenc_proxies_decode_on_the_gpu_and_stay_exact() {
    let available = vidcut_av::export::encode::detect();
    let nvenc: Vec<ProxyEncoder> = encoders(ProxyCodec::Av1, &available)
        .into_iter()
        .filter(|e| e.name == "av1_nvenc")
        .collect();
    assert!(!nvenc.is_empty(), "no NVENC here");
    let hd = |codec, container| FixtureSpec::new(codec, container, 1280, 720, (30, 1), 45);
    let sd = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 45);
    let cases = [
        (
            "H.264 with B-frames",
            hd(VideoCodec::H264, Container::Mp4),
            true,
        ),
        ("HEVC 10-bit", hd(VideoCodec::Hevc10, Container::Mkv), true),
        ("VP9", hd(VideoCodec::Vp9, Container::Mkv), true),
        // FFmpeg decodes AV1 with libdav1d unless told otherwise.
        ("AV1 10-bit", hd(VideoCodec::Av1, Container::Mp4), true),
        (
            "MPEG-TS starting at 1.4 s",
            hd(VideoCodec::H264, Container::MpegTs).start_offset_ms(1400),
            true,
        ),
        (
            "interlaced",
            hd(VideoCodec::H264, Container::Mp4).interlaced(true),
            true,
        ),
        // Untagged SD is BT.601: libplacebo converts it first.
        ("BT.601", sd.clone(), true),
        ("interlaced BT.601", sd.interlaced(true), false),
        (
            "turned",
            hd(VideoCodec::H264, Container::Mp4).rotation(90),
            false,
        ),
        ("ProRes", hd(VideoCodec::ProRes422Hq, Container::Mov), false),
    ];
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-gpu-")
        .tempdir()
        .unwrap();
    for (n, (what, spec, gpu)) in cases.into_iter().enumerate() {
        let source = fixture::ensure(&spec).unwrap();
        let dest = tmp.path().join(format!("gpu-{n}.mkv"));
        let turned = spec.rotation % 180 == 90;
        let (w, h) = if turned { (360, 640) } else { (640, 360) };
        let written =
            vidcut_av::proxy::write(&Input::File(source.clone()), w, h, &dest, None, &nvenc)
                .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(written.on_gpu, gpu, "{what}: on the GPU");
        assert_eq!(written.encoder.name, "av1_nvenc", "{what}");
        let src = frames(&Input::File(source));
        let got = frames(&Input::File(dest));
        assert_eq!(got.len(), src.len(), "{what}: frames");
        for (k, ((want_code, want_t), (got_code, got_t))) in src.iter().zip(&got).enumerate() {
            // A turned picture's barcode reads across, not along.
            if !turned {
                assert_eq!(got_code, want_code, "{what}: frame {k}");
            }
            assert!(
                (got_t - want_t).abs() <= 1.5e-3,
                "{what}: frame {k} at {got_t}, source at {want_t}"
            );
        }
    }
}
