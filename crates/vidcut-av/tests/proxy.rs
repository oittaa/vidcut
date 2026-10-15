//! A proxy keeps the source frame's barcode and presentation timestamp,
//! with each encoder proxies can be made with (decision D41).

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::export::encode::{Availability, detect};
use vidcut_av::ffmpeg;
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, VideoCodec};
use vidcut_av::hw::GpuDecoder;
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
    .flat_map(|c| encoders(c, &none, &[]))
    .collect()
}

/// Each frame's barcode and timestamp match the source's in `out` proxies
/// of `w`×`h` sources, the frames decoded on `decoder` (`None`: the CPU).
fn matches_source(
    enc: &ProxyEncoder,
    decoder: Option<&GpuDecoder>,
    (w, h): (u32, u32),
    out: (u32, u32),
) {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mp4, w, h, (30, 1), 24);
    let source = fixture::ensure(&spec).unwrap();
    let offset = FixtureSpec::new(VideoCodec::H264, Container::MpegTs, w, h, (30, 1), 12)
        .start_offset_ms(1400);
    let shifted = fixture::ensure(&offset).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    for (i, path) in [source, shifted].iter().enumerate() {
        let dest = dir.join(format!("proxy-{i}.mkv"));
        let written = vidcut_av::proxy::write(
            &Input::File(path.clone()),
            out.0,
            out.1,
            &dest,
            None,
            std::slice::from_ref(enc),
            decoder,
        )
        .unwrap_or_else(|e| panic!("{} with {}: {e}", path.display(), enc.name));
        assert_eq!(written.encoder, *enc);
        assert_eq!((written.width, written.height), out);
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

#[test]
fn a_proxy_matches_source_frames_and_timestamps() {
    for enc in software() {
        matches_source(&enc, None, (320, 180), (320, 180));
    }
}

/// The frames each GPU decodes, made into an H.264 proxy by its own
/// encoder: NVENC from NVDEC, AMF from Direct3D 11 and VideoToolbox from
/// VideoToolbox take them on the GPU, as VAAPI does from VA-API. Untagged
/// 320×180 is read as BT.601, which only NVIDIA's path converts on the GPU
/// (libplacebo, a picture a frame late), and untagged 1280×720 as BT.709,
/// which every GPU fits.
#[test]
fn a_gpu_proxy_matches_source_frames_and_timestamps() {
    let decoders = GpuDecoder::all();
    if decoders.is_empty() {
        return;
    }
    let available = detect(false);
    for decoder in &decoders {
        // The encoders on its GPU: every one where no GPU is listed (a
        // Mac's).
        let on_it = match decoder.gpu() {
            Some(gpu) => available.on(gpu),
            None => available.clone(),
        };
        let hardware = encoders(ProxyCodec::H264, &on_it, &[])
            .into_iter()
            .find(|e| e.kind.gpu());
        if let Some(enc) = hardware {
            matches_source(&enc, Some(decoder), (320, 180), (320, 180));
            matches_source(&enc, Some(decoder), (1280, 720), (640, 360));
        }
    }
}

#[test]
fn a_sequence_proxy_uses_the_sequence_timestamps() {
    let spec = ImageSpec::sequence(1, 8, 2, 320, 180);
    let files = fixture::ensure_images(&spec).unwrap();
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
            None,
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
