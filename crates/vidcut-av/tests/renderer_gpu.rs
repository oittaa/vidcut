//! Timeline tiles and the two-up decoded on each GPU decoder there is
//! (docs/decisions/preview.md §8): every picture is the one the CPU gives.

#![cfg(feature = "gpu-tests")]

use vidcut_av::decode::read_barcode;
use vidcut_av::export::renderer::{FrameReader, StillRenderer};
use vidcut_av::ffmpeg::{format::Pixel, frame};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::GpuDecoder;
use vidcut_av::thumbs::Image;
use vidcut_core::render::VideoPiece;
use vidcut_core::time::Rational;

fn piece(path: &std::path::Path, source_frame: i64) -> VideoPiece {
    let origin = vidcut_av::probe::probe(path)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    VideoPiece::Source {
        media: "m".into(),
        path: path.to_path_buf(),
        origin,
        source_frame,
        frames: 1,
        image: None,
    }
}

/// The barcode in a rendered picture.
fn code(img: &Image) -> Option<u32> {
    let mut f = frame::Video::new(Pixel::RGBA, img.width, img.height);
    let stride = f.stride(0);
    let row = img.width as usize * 4;
    for y in 0..img.height as usize {
        f.data_mut(0)[y * stride..y * stride + row]
            .copy_from_slice(&img.rgba[y * row..(y + 1) * row]);
    }
    read_barcode(&f).unwrap()
}

/// How far `a`'s red, green and blue are from `b`'s on average, and the
/// average of the differences (a shift of levels).
fn distance(a: &Image, b: &Image) -> (f64, f64) {
    let (mut abs, mut sum, mut n) = (0.0, 0.0, 0.0);
    for (pa, pb) in a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
    {
        for c in 0..3 {
            let d = f64::from(pa[c]) - f64::from(pb[c]);
            abs += d.abs();
            sum += d;
            n += 1.0;
        }
    }
    (abs / n, sum / n)
}

/// Tiles read in jumps and two-up frames stepped back, decoded on each
/// GPU decoder, are the CPU's frames and pictures: HEVC, AV1 and VP9 on
/// the GPU, shrunk there (and turned), H.264 on the CPU.
#[test]
fn gpu_decoded_tiles_and_two_up_are_the_cpus() {
    let decoders = GpuDecoder::all();
    if decoders.is_empty() {
        eprintln!("skipping: no GPU decoder here");
        return;
    }
    let fps = Rational::new(30, 1).unwrap();
    // Keyframes 60 frames apart, with B-frames.
    let spec = |codec, container| {
        FixtureSpec::new(codec, container, 1280, 720, (30, 1), 120)
            .gop(60)
            .bframes(2)
    };
    use vidcut_av::ffmpeg::codec::Id;
    // What each is, its codec and bits, and whether it is stored turned.
    let sources = [
        (
            "HEVC 10-bit",
            spec(VideoCodec::Hevc10, Container::Mkv),
            (Id::HEVC, 10),
            false,
        ),
        (
            "AV1 10-bit",
            spec(VideoCodec::Av1, Container::Mp4),
            (Id::AV1, 10),
            false,
        ),
        (
            "VP9 turned",
            spec(VideoCodec::Vp9, Container::Mp4).rotation(90),
            (Id::VP9, 8),
            true,
        ),
        (
            "H.264",
            spec(VideoCodec::H264, Container::Mp4),
            (Id::H264, 8),
            false,
        ),
    ];
    let (w, h) = (320, 180);
    for (what, spec, (codec, bits), turned) in &sources {
        let path = fixture::ensure(spec).unwrap();
        // Frame `k`'s barcode: the source's, or `k` once turned upright.
        let want = |k: i64| {
            let img = FrameReader::default()
                .rgba(&piece(&path, k), fps, w, h)
                .unwrap();
            (if *turned { Some(k as u32) } else { code(&img) }, img)
        };
        let jumps: Vec<i64> = (0..120).step_by(17).collect();
        let cpu: Vec<_> = jumps.iter().map(|&k| want(k)).collect();
        for decoder in &decoders {
            let name = format!("{} {}, {what}", decoder.kind().name(), decoder.on());
            // On the GPU where it lists the codec, but H.264, which the
            // CPU decodes faster for the timeline (preview.md §8).
            let on_gpu = (*codec != Id::H264
                && decoder.decodes(*codec)
                && decoder.hardware_decodes(*codec, *bits) != Some(false))
            .then_some(decoder.kind());
            // On a stream or device of their own, as the GUI's tiles and
            // two-up take it (`preferences::timeline_decoder`).
            let own = || decoder.own().unwrap_or_else(|e| panic!("{name}: {e}"));
            // Tiles: frames far apart, in order.
            let mut reader = FrameReader::decoding_on(own());
            for (&k, (want_code, want_img)) in jumps.iter().zip(&cpu) {
                let img = reader.rgba(&piece(&path, k), fps, w, h).unwrap();
                assert_eq!(code(&img), *want_code, "{name}: tile of frame {k}");
                assert_eq!(reader.decoded_on(), on_gpu, "{name}: tile of frame {k}");
                // Shrunk 4:1, the GPUs' scalers filter edges their own way
                // (the Radeon's video processor up to 3 levels on average);
                // levels hold (NVDEC's halvings round half a level up).
                let (abs, shift) = distance(&img, want_img);
                assert!(
                    abs <= 4.0 && shift.abs() <= 0.75,
                    "{name}: tile of frame {k} is {abs:.2} levels from the CPU's, shifted {shift:.2}"
                );
            }
            // The two-up: back a frame at a time across a keyframe, the
            // pictures the CPU's as the tiles are.
            let mut r = StillRenderer::default().decoding_on(own());
            for k in (40..=75).rev() {
                let img = r.rgba(&piece(&path, k), 0, fps, w, h).unwrap();
                let (want_code, want_img) = want(k);
                let want_code = if *turned { Some(k as u32) } else { want_code };
                assert_eq!(code(&img), want_code, "{name}: frame {k} stepping back");
                let (abs, shift) = distance(&img, &want_img);
                assert!(
                    abs <= 4.0 && shift.abs() <= 0.75,
                    "{name}: frame {k} stepping back is {abs:.2} levels from the CPU's, shifted {shift:.2}"
                );
            }
        }
    }
}
