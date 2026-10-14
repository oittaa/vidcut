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

/// Each frame's planes, rows packed.
#[cfg(feature = "gpu-tests")]
fn pictures(input: &Input) -> Vec<Vec<Vec<u8>>> {
    let mut reader = VideoReader::open_input(input).unwrap();
    let mut out = Vec::new();
    while let Some(frame) = reader.next_frame().unwrap() {
        let planes = (0..frame.planes())
            .map(|p| {
                let (w, h, stride) = (
                    frame.plane_width(p) as usize,
                    frame.plane_height(p) as usize,
                    frame.stride(p),
                );
                let data = frame.data(p);
                (0..h)
                    .flat_map(|y| &data[y * stride..y * stride + w])
                    .copied()
                    .collect()
            })
            .collect();
        out.push(planes);
    }
    out
}

/// How far `a`'s values are from `b`'s, plane by plane: on average, and the
/// average difference (a shift of levels).
#[cfg(feature = "gpu-tests")]
fn distance(a: &[Vec<u8>], b: &[Vec<u8>]) -> Vec<(f64, f64)> {
    assert_eq!(a.len(), b.len(), "as many planes");
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            assert_eq!(a.len(), b.len(), "planes of one size");
            let n = a.len() as f64;
            let diffs = a.iter().zip(b).map(|(&x, &y)| f64::from(x) - f64::from(y));
            let (abs, sum) = diffs.fold((0.0, 0.0), |(abs, sum), d| (abs + d.abs(), sum + d));
            (abs / n, sum / n)
        })
        .collect()
}

fn skip(e: impl std::fmt::Display) -> bool {
    eprintln!("skipping: {e}");
    true
}

/// The software encoders: x264, SVT-AV1 when this FFmpeg has it, ProRes
/// and DNxHR.
fn software() -> Vec<ProxyEncoder> {
    let none = Availability::default();
    [
        ProxyCodec::H264,
        ProxyCodec::Av1,
        ProxyCodec::Prores,
        ProxyCodec::Dnxhr,
    ]
    .into_iter()
    .flat_map(|c| encoders(c, &none))
    .collect()
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
            vidcut_av::hw::Decoding::Nvdec,
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

/// Keyframes every 60 frames as it is set up, not where the source's are
/// (ProRes and DNxHR: every frame), and every frame coded in the order it
/// shows (nothing ahead of a seek).
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
        vidcut_av::hw::Decoding::Nvdec,
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
    if matches!(enc.codec, "prores" | "dnxhr") {
        assert_eq!(keys.len(), 150, "{}", enc.name);
    } else {
        assert_eq!(keys, [0, 60, 120], "{}", enc.name);
    }
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
    if hardware.is_empty() {
        eprintln!("skipping: no hardware AV1 encoder here: {auto:?}");
        return;
    }
    for enc in hardware {
        matches_source(enc);
        keyframes_and_order(enc);
    }
}

/// On Apple's ProRes hardware (VideoToolbox), the same: ProRes picks it.
#[cfg(feature = "gpu-tests")]
#[test]
fn videotoolbox_prores_proxies_are_exact_too() {
    let available = vidcut_av::export::encode::detect();
    let prores = encoders(ProxyCodec::Prores, &available);
    let Some(apple) = prores.iter().find(|e| e.name == "prores_videotoolbox") else {
        eprintln!("skipping: no ProRes hardware here: {prores:?}");
        return;
    };
    assert_eq!(apple.label(), "ProRes Proxy, Apple hardware (VideoToolbox)");
    matches_source(apple);
    keyframes_and_order(apple);
}

/// DNxHR takes nothing smaller than 256×120, and says so.
#[test]
fn a_dnxhr_proxy_is_at_least_256_by_120() {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 320, 180, (30, 1), 6);
    let Some(source) = fixture::ensure(&spec).map_err(skip).ok() else {
        return;
    };
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-dnxhr-")
        .tempdir()
        .unwrap();
    let dest = tmp.path().join("small.mkv");
    let e = vidcut_av::proxy::write(
        &Input::File(source),
        240,
        136,
        &dest,
        None,
        &encoders(ProxyCodec::Dnxhr, &Availability::default()),
        vidcut_av::hw::Decoding::Cpu,
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "DNxHR proxies are at least 256×120 pixels, and this one would be 240×136: make \
         the proxy size larger or choose another codec"
    );
    assert!(!dest.exists());
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
        let written = vidcut_av::proxy::write(
            &input,
            320,
            180,
            &dest,
            None,
            std::slice::from_ref(&enc),
            vidcut_av::hw::Decoding::Nvdec,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", enc.name));
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

/// With NVENC, video files are decoded on NVDEC and scaled there, frames
/// never leaving it: each proxy frame is still the source frame with its
/// timestamp (interlaced ones deinterlaced one frame per frame, as the CPU
/// reader does; BT.601 and HDR ones converted by libplacebo, which turns
/// turned ones upright). Ones NVDEC can't decode are made on the CPU, still
/// with NVENC.
#[cfg(feature = "gpu-tests")]
#[test]
fn nvenc_proxies_decode_on_the_gpu_and_stay_exact() {
    use vidcut_core::model::Hdr;
    let available = vidcut_av::export::encode::detect();
    let nvenc: Vec<ProxyEncoder> = encoders(ProxyCodec::Av1, &available)
        .into_iter()
        .filter(|e| e.name == "av1_nvenc")
        .collect();
    if nvenc.is_empty() {
        eprintln!("skipping: no NVENC here");
        return;
    }
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
        // Deinterlaced by the reader, then converted by libplacebo.
        ("interlaced BT.601", sd.interlaced(true), true),
        (
            "turned",
            hd(VideoCodec::H264, Container::Mp4).rotation(90),
            true,
        ),
        (
            "turned upside down",
            hd(VideoCodec::H264, Container::Mp4).rotation(180),
            true,
        ),
        (
            "turned 10-bit",
            hd(VideoCodec::Hevc10, Container::Mp4).rotation(90),
            true,
        ),
        // HDR is tone-mapped by libplacebo with the CPU's curve.
        (
            "PQ",
            hd(VideoCodec::Hevc10, Container::Mkv).hdr(Hdr::Pq),
            true,
        ),
        (
            "turned HLG",
            hd(VideoCodec::Hevc10, Container::Mp4)
                .hdr(Hdr::Hlg)
                .rotation(90),
            true,
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
        let turned = spec.rotation != 0 || spec.hdr.is_some();
        let written = vidcut_av::proxy::write(
            &Input::File(source.clone()),
            640,
            360,
            &dest,
            None,
            &nvenc,
            vidcut_av::hw::Decoding::Nvdec,
        )
        .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(written.on_gpu, gpu, "{what}: on the GPU");
        assert_eq!(written.encoder.name, "av1_nvenc", "{what}");
        let src = frames(&Input::File(source));
        let got = frames(&Input::File(dest));
        assert_eq!(got.len(), src.len(), "{what}: frames");
        for (k, ((want_code, want_t), (got_code, got_t))) in src.iter().zip(&got).enumerate() {
            // A turned source's barcode reads only once turned upright, a
            // PQ one once tone-mapped.
            let want_code = if turned { Some(k as u32) } else { *want_code };
            assert_eq!(*got_code, want_code, "{what}: frame {k}");
            assert!(
                (got_t - want_t).abs() <= 1.5e-3,
                "{what}: frame {k} at {got_t}, source at {want_t}"
            );
        }
    }
}

/// Decoded on each GPU decoder there is and copied down for the CPU to
/// scale (x264 here), each proxy frame is still the source frame with its
/// timestamp; streams the GPU can't decode go to the CPU.
#[cfg(feature = "gpu-tests")]
#[test]
fn gpu_decoded_proxies_stay_exact() {
    use vidcut_core::model::Hdr;
    let decoders = vidcut_av::hw::GpuDecoder::all();
    if decoders.is_empty() {
        eprintln!("skipping: no GPU decoder here");
        return;
    }
    let x264 = encoders(ProxyCodec::H264, &Availability::default());
    let hd = |codec, container| FixtureSpec::new(codec, container, 1280, 720, (30, 1), 45);
    let sd = FixtureSpec::new(VideoCodec::H264, Container::Mp4, 640, 360, (30, 1), 45);
    let cases = [
        ("H.264", hd(VideoCodec::H264, Container::Mp4), true),
        ("HEVC 10-bit", hd(VideoCodec::Hevc10, Container::Mkv), true),
        ("VP9", hd(VideoCodec::Vp9, Container::Mkv), true),
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
        ("interlaced BT.601", sd.interlaced(true), true),
        (
            "turned 10-bit",
            hd(VideoCodec::Hevc10, Container::Mp4).rotation(90),
            true,
        ),
        (
            "PQ",
            hd(VideoCodec::Hevc10, Container::Mkv).hdr(Hdr::Pq),
            true,
        ),
        (
            "turned HLG",
            hd(VideoCodec::Hevc10, Container::Mp4)
                .hdr(Hdr::Hlg)
                .rotation(90),
            true,
        ),
        ("ProRes", hd(VideoCodec::ProRes422Hq, Container::Mov), false),
    ];
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-decoded-")
        .tempdir()
        .unwrap();
    let write = |source: &std::path::Path, dest: &std::path::Path, decoder| {
        vidcut_av::proxy::write(
            &Input::File(source.to_path_buf()),
            640,
            360,
            dest,
            None,
            &x264,
            decoder,
        )
    };
    // Made on the CPU, to compare the pictures with.
    let on_cpu: Vec<_> = cases
        .iter()
        .enumerate()
        .map(|(n, (what, spec, _))| {
            let dest = tmp.path().join(format!("cpu-{n}.mkv"));
            write(
                &fixture::ensure(spec).unwrap(),
                &dest,
                vidcut_av::hw::Decoding::Cpu,
            )
            .unwrap_or_else(|e| panic!("CPU, {what}: {e}"));
            pictures(&Input::File(dest))
        })
        .collect();
    for decoder in &decoders {
        let name = format!("{} {}", decoder.kind().name(), decoder.on());
        for (n, (what, spec, gpu)) in cases.iter().enumerate() {
            let source = fixture::ensure(spec).unwrap();
            let dest = tmp.path().join(format!("decoded-{n}.mkv"));
            let turned = spec.rotation != 0 || spec.hdr.is_some();
            let written = write(&source, &dest, vidcut_av::hw::Decoding::Gpu(decoder))
                .unwrap_or_else(|e| panic!("{name}, {what}: {e}"));
            // Scaled on another GPU (or not), the pictures are the CPU's but
            // for the scalers' own rounding: on average within a level and
            // a half, no level shifted (a matrix taken wrong shifts white).
            let worst = pictures(&Input::File(dest.clone()))
                .iter()
                .zip(&on_cpu[n])
                .flat_map(|(a, b)| distance(a, b))
                .fold((0.0f64, 0.0f64), |(abs, shift), (a, s)| {
                    (abs.max(a), shift.max(s.abs()))
                });
            assert!(
                worst.0 <= 1.5 && worst.1 <= 0.5,
                "{name}, {what}: on average {:.2} levels from the CPU's, shifted {:.2}",
                worst.0,
                worst.1
            );
            let want = gpu.then_some(decoder.kind());
            // A GPU may lack a codec (AV1, VP9 on older ones): the CPU then.
            if written.decoder != want {
                assert!(*gpu && written.decoder.is_none(), "{name}, {what}");
                eprintln!("{name}: {what} decoded on the CPU");
            }
            let src = frames(&Input::File(source));
            let got = frames(&Input::File(dest));
            assert_eq!(got.len(), src.len(), "{name}, {what}: frames");
            for (k, ((want_code, want_t), (got_code, got_t))) in src.iter().zip(&got).enumerate() {
                let want_code = if turned { Some(k as u32) } else { *want_code };
                assert_eq!(*got_code, want_code, "{name}, {what}: frame {k}");
                assert!(
                    (got_t - want_t).abs() <= 1.5e-3,
                    "{name}, {what}: frame {k} at {got_t}, source at {want_t}"
                );
            }
        }
    }
}
