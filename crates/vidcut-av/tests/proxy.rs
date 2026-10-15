//! A proxy keeps the source frame's barcode and presentation timestamp,
//! with each encoder proxies can be made with (decision D41), and a 10-bit
//! proxy an HDR source's colors.

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::export::encode::{Availability, detect};
use vidcut_av::ffmpeg;
use vidcut_av::fixture::{self, Container, FixtureSpec, HdrMeta, ImageSpec, VideoCodec};
use vidcut_av::hw::GpuDecoder;
use vidcut_av::proxy::{ProxyEncoder, encoders};
use vidcut_core::model::Hdr;
use vidcut_core::preferences::ProxyCodec;

fn secs(pts: i64, tb: ffmpeg::Rational) -> f64 {
    pts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
}

/// HDR10's static values: a 1000-nit mastering display, MaxCLL 1000 and
/// MaxFALL 400.
const HDR10: HdrMeta = HdrMeta {
    max_nits: 1000,
    light: Some((1000, 400)),
    hdr10plus: false,
    hdr10plus_frames: None,
};

/// A source: 8-bit H.264, 10-bit HEVC, or 10-bit HEVC in PQ with HDR10's
/// static values.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    H264,
    Hevc10,
    Pq,
}

impl Source {
    /// Its fixture in `container`, `w`×`h`, of `frames` frames.
    fn spec(self, container: Container, (w, h): (u32, u32), frames: u32) -> FixtureSpec {
        let codec = match self {
            Source::H264 => VideoCodec::H264,
            Source::Hevc10 | Source::Pq => VideoCodec::Hevc10,
        };
        let spec = FixtureSpec::new(codec, container, w, h, (30, 1), frames);
        match self {
            Source::Pq => spec.hdr(Hdr::Pq).hdr_meta(HDR10),
            _ => spec,
        }
    }
}

/// What a proxy's pictures are, besides the source's barcodes and times.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pictures {
    /// As the encoder makes them.
    Any,
    /// 10-bit.
    TenBit,
    /// 10-bit PQ in BT.2020: the barcode's white at the source's level,
    /// with its static HDR metadata.
    Pq,
    /// SDR BT.709.
    Sdr,
}

/// The first frame of the file at `path`, decoded on the CPU.
fn first_frame(path: &std::path::Path) -> ffmpeg::frame::Video {
    let mut reader = VideoReader::open_input(&Input::File(path.to_path_buf())).unwrap();
    reader.next_frame().unwrap().expect("a frame")
}

/// `f`'s primaries, transfer and matrix.
fn tags(
    f: &ffmpeg::frame::Video,
) -> (
    ffmpeg::ffi::AVColorPrimaries,
    ffmpeg::ffi::AVColorTransferCharacteristic,
    ffmpeg::ffi::AVColorSpace,
) {
    // SAFETY: plain fields of a decoded frame.
    let raw = unsafe { &*f.as_ptr() };
    (raw.color_primaries, raw.color_trc, raw.colorspace)
}

/// The barcode's white in 10-bit `f`: the middle of its luma levels above
/// 300, between PQ's black (64) and SDR white in PQ (573).
fn white(f: &ffmpeg::frame::Video) -> u16 {
    let (stride, data) = (f.stride(0), f.data(0));
    let mut bright = Vec::new();
    for y in 0..f.height() as usize {
        for x in 0..f.width() as usize {
            let at = y * stride + 2 * x;
            let level = u16::from_le_bytes([data[at], data[at + 1]]);
            if level > 300 {
                bright.push(level);
            }
        }
    }
    bright.sort_unstable();
    bright[bright.len() / 2]
}

/// Checks that the proxy at `path`, made with `enc`, has `pictures`.
fn check_pictures(path: &std::path::Path, enc: &ProxyEncoder, pictures: Pictures) {
    use ffmpeg::ffi::{
        AVColorPrimaries as P, AVColorSpace as S, AVColorTransferCharacteristic as T,
    };
    let probed = vidcut_av::probe::probe(path, None).unwrap();
    let video = probed.video.expect("a picture");
    let depth = vidcut_av::probe::pixel_depth(&video.pix_fmt);
    let first = first_frame(path);
    let what = format!("{} {}", enc.name, path.display());
    match pictures {
        Pictures::Any => {}
        Pictures::TenBit => assert_eq!(depth, 10, "{what}"),
        Pictures::Pq => {
            assert_eq!(depth, 10, "{what}");
            assert_eq!(video.transfer, "smpte2084", "{what}");
            let pq = (
                P::AVCOL_PRI_BT2020,
                T::AVCOL_TRC_SMPTE2084,
                S::AVCOL_SPC_BT2020_NCL,
            );
            assert_eq!(tags(&first), pq, "{what}");
            // Not tone-mapped: SDR white stays at 203 nits.
            let level = white(&first);
            assert!(
                level.abs_diff(fixture::PQ_WHITE) <= 4,
                "{what}: white at {level}"
            );
            let kept = vidcut_av::hdr::of_frame(&first);
            assert_eq!(kept.light, HDR10.values().light, "{what}");
            let mastering = kept.mastering.expect("a mastering display");
            // As close as AV1's fixed point codes them.
            let close = |a: f64, b: f64| (a - b).abs() < 1e-4;
            assert!(
                close(mastering.max_nits, 1000.0) && close(mastering.min_nits, 0.0001),
                "{what}: {mastering:?}"
            );
        }
        Pictures::Sdr => {
            assert_eq!(video.transfer, "bt709", "{what}");
            let sdr = (P::AVCOL_PRI_BT709, T::AVCOL_TRC_BT709, S::AVCOL_SPC_BT709);
            assert_eq!(tags(&first), sdr, "{what}");
        }
    }
}

/// The barcode of `f`. An HDR frame's (10-bit on the CPU) is read from
/// black to the fixtures' white in its transfer, SDR white: 203 nits in PQ,
/// 75% in HLG.
fn barcode(f: &ffmpeg::frame::Video) -> Option<u32> {
    let white = match vidcut_av::color::hdr_of(f) {
        Some(Hdr::Pq) => fixture::PQ_WHITE,
        Some(Hdr::Hlg) => fixture::HLG_WHITE,
        None => return read_barcode(f).unwrap(),
    };
    let (stride, data) = (f.stride(0), f.data(0));
    let (w, h) = (f.width() as usize, f.height() as usize);
    vidcut_av::barcode::read_with(w, h, |x, y| {
        let at = y * stride + 2 * x;
        let level = i32::from(u16::from_le_bytes([data[at], data[at + 1]]));
        // Black (64) to white, as 8-bit's 16 to 235.
        (16 + (level - 64) * 219 / (i32::from(white) - 64)).clamp(0, 255) as u8
    })
}

/// `(barcode, timestamp seconds)` of every frame.
fn frames(input: &Input) -> Vec<(Option<u32>, f64)> {
    let mut reader = VideoReader::open_input(input).unwrap();
    let tb = reader.time_base();
    let mut out = Vec::new();
    while let Some(frame) = reader.next_frame().unwrap() {
        let pts = frame.pts().expect("pts");
        out.push((barcode(&frame), secs(pts, tb)));
    }
    out
}

/// The software encoders: x264, SVT-AV1 and x265 where this FFmpeg has
/// them, ProRes and DNxHR.
fn software() -> Vec<ProxyEncoder> {
    let none = Availability::default();
    [
        ProxyCodec::H264,
        ProxyCodec::Av1,
        ProxyCodec::Hevc,
        ProxyCodec::Prores,
        ProxyCodec::Dnxhr,
    ]
    .into_iter()
    .flat_map(|c| encoders(c, &none, &[]))
    .collect()
}

/// Each frame's barcode and timestamp match the source's in `out` proxies
/// of `w`×`h` `source`s, the frames decoded on `decoder` (`None`: the CPU),
/// and the proxies have `pictures`.
fn matches_source(
    enc: &ProxyEncoder,
    decoder: Option<&GpuDecoder>,
    source: Source,
    size: (u32, u32),
    out: (u32, u32),
    pictures: Pictures,
) {
    let original = fixture::ensure(&source.spec(Container::Mp4, size, 24)).unwrap();
    let offset = source
        .spec(Container::MpegTs, size, 12)
        .start_offset_ms(1400);
    let shifted = fixture::ensure(&offset).unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("vidcut-proxy-")
        .tempdir()
        .unwrap();
    let dir = tmp.path();
    for (i, path) in [original, shifted].iter().enumerate() {
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
        check_pictures(&dest, enc, pictures);
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
    let small = (320, 180);
    for enc in software() {
        matches_source(&enc, None, Source::H264, small, small, Pictures::Any);
    }
    // A 10-bit source, with the encoders that take 10 bits: SVT-AV1 and
    // x265 where this FFmpeg's do, and ProRes.
    for enc in software().into_iter().filter(|e| e.ten_bit) {
        matches_source(&enc, None, Source::Hevc10, small, small, Pictures::TenBit);
    }
}

/// A PQ source's proxies by those encoders keep its HDR, and x264's is SDR.
#[test]
fn a_ten_bit_proxy_keeps_an_hdr_sources_colors() {
    let small = (320, 180);
    for enc in software().into_iter().filter(|e| e.codec != "dnxhr") {
        let pictures = if enc.ten_bit {
            Pictures::Pq
        } else {
            Pictures::Sdr
        };
        matches_source(&enc, None, Source::Pq, small, small, pictures);
    }
}

/// `source`'s frames on `decoder` into `enc`'s proxies at their own size
/// and at half of 1280×720 ([`matches_source`]).
fn on_the_gpu(enc: &ProxyEncoder, decoder: &GpuDecoder, source: Source, pictures: Pictures) {
    let small = (320, 180);
    matches_source(enc, Some(decoder), source, small, small, pictures);
    matches_source(
        enc,
        Some(decoder),
        source,
        (1280, 720),
        (640, 360),
        pictures,
    );
}

/// The frames each GPU decodes, made into H.264 and HEVC proxies by its own
/// encoders: NVENC from NVDEC, AMF from Direct3D 11 and VideoToolbox from
/// VideoToolbox take them on the GPU, as VAAPI does from VA-API. A 10-bit
/// source's and a PQ source's go into AV1, HEVC and ProRes proxies by the
/// encoders there that take 10 bits, and the PQ source's into an SDR H.264
/// one too. Untagged 320×180 is read as BT.601, which only NVIDIA's path
/// converts on the GPU (libplacebo, a picture a frame late), and untagged
/// 1280×720 as BT.709, which every GPU fits.
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
        for codec in [ProxyCodec::H264, ProxyCodec::Hevc] {
            let hardware = encoders(codec, &on_it, &[])
                .into_iter()
                .find(|e| e.kind.gpu());
            if let Some(enc) = hardware {
                on_the_gpu(&enc, decoder, Source::H264, Pictures::Any);
                if codec == ProxyCodec::H264 {
                    on_the_gpu(&enc, decoder, Source::Pq, Pictures::Sdr);
                }
            }
        }
        for codec in [ProxyCodec::Av1, ProxyCodec::Hevc, ProxyCodec::Prores] {
            let ten_bit = encoders(codec, &on_it, &[])
                .into_iter()
                .find(|e| e.kind.gpu() && e.ten_bit);
            if let Some(enc) = ten_bit {
                on_the_gpu(&enc, decoder, Source::Hevc10, Pictures::TenBit);
                on_the_gpu(&enc, decoder, Source::Pq, Pictures::Pq);
            }
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
