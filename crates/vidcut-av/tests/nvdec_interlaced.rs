//! Interlaced H.264 decoded on NVIDIA's NVDEC: its frames are the CPU
//! reader's deinterlaced ones, barcode and timestamp, read through and
//! after seeks, and within a level in every sample, deinterlaced on the GPU
//! where `bwdif_cuda` gives `bwdif`'s frames and in memory where it
//! doesn't. Skips where the GPU in use isn't NVIDIA's.

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::ffmpeg::format::Pixel;
use vidcut_av::ffmpeg::frame;
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};

/// A frame a reader gave: the GPU's memory it was in, its timestamp and
/// barcode, and its picture in memory.
struct Picture {
    kind: Option<Kind>,
    pts: i64,
    code: Option<u32>,
    picture: frame::Video,
}

/// Up to `n` frames on from where `reader` is, each frame in GPU memory
/// brought down.
fn pictures(reader: &mut VideoReader, n: usize) -> Vec<Picture> {
    let mut out = Vec::new();
    while out.len() < n {
        let Some(f) = reader.next_frame().unwrap() else {
            break;
        };
        let kind = Kind::of_frame(&f);
        let picture = if vidcut_av::hw::on_gpu(&f) {
            vidcut_av::hw::download(&f).unwrap()
        } else {
            f
        };
        out.push(Picture {
            kind,
            pts: picture.pts().expect("pts"),
            code: read_barcode(&picture).unwrap(),
            picture,
        });
    }
    out
}

/// Sample (`x`, `y`) of component `c` (Y, Cb, Cr) of `f`, an 8-bit 4:2:0
/// picture: planar, or NV12's.
fn sample(f: &frame::Video, c: usize, x: usize, y: usize) -> u8 {
    match (c, f.format()) {
        (0, _) => f.data(0)[y * f.stride(0) + x],
        (_, Pixel::NV12) => f.data(1)[y * f.stride(1) + 2 * x + c - 1],
        _ => f.data(c)[y * f.stride(c) + x],
    }
}

/// The rows of `got` with a sample more than a level from `cpu`'s, as
/// (component, row).
fn rows_off(cpu: &frame::Video, got: &frame::Video) -> Vec<(usize, usize)> {
    let mut off = Vec::new();
    for c in 0..3 {
        let (w, h) = match c {
            0 => (cpu.width(), cpu.height()),
            _ => (cpu.width().div_ceil(2), cpu.height().div_ceil(2)),
        };
        for y in 0..h as usize {
            if (0..w as usize).any(|x| sample(got, c, x, y).abs_diff(sample(cpu, c, x, y)) > 1) {
                off.push((c, y));
            }
        }
    }
    off
}

#[test]
fn interlaced_h264_deinterlaced_on_nvdec_is_the_cpus() {
    let Some(gpu) = GpuDecoder::best().filter(|g| g.kind() == Kind::Nvdec) else {
        return;
    };
    for top in [true, false] {
        let spec = FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (25, 1), 50)
            .interlaced(top)
            .no_audio();
        let input = Input::File(fixture::ensure(&spec).unwrap());
        let truth = pictures(&mut VideoReader::open_input(&input).unwrap(), usize::MAX);
        assert_eq!(truth.len(), 50);
        let mut reader = VideoReader::open_on(&input, &gpu).unwrap();
        assert_eq!(reader.gpu(), Some(Kind::Nvdec));
        let got = pictures(&mut reader, usize::MAX);
        assert_eq!(got.len(), truth.len(), "top field first: {top}");
        for (k, (g, t)) in got.iter().zip(&truth).enumerate() {
            let what = format!("top field first: {top}, frame {k}");
            // Deinterlaced on the GPU, or in memory where bwdif_cuda
            // doesn't give bwdif's frames.
            assert!(matches!(g.kind, Some(Kind::Nvdec) | None), "{what}");
            assert_eq!((g.pts, g.code), (t.pts, t.code), "{what}");
            assert_eq!(rows_off(&t.picture, &g.picture), [], "{what}");
        }
        let coded = |frames: &[Picture]| -> Vec<(i64, Option<u32>)> {
            frames.iter().map(|f| (f.pts, f.code)).collect()
        };
        for (k, frame) in truth.iter().enumerate() {
            reader.seek(frame.pts).unwrap();
            let want = coded(&truth[k..(k + 3).min(truth.len())]);
            let after = coded(&pictures(&mut reader, 3));
            assert_eq!(
                after, want,
                "top field first: {top}, after a seek to frame {k}"
            );
        }
    }
}
