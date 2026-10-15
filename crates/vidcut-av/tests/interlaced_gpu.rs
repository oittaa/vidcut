//! Interlaced sources decoded on NVIDIA's GPU, H.264 on NVDEC and ProRes on
//! Vulkan beside it, give the CPU's deinterlaced frames. A reader's are the
//! CPU reader's, barcode and timestamp, through and after seeks, and within
//! a level in every sample, deinterlaced on the GPU where its filter gives
//! `bwdif`'s frames and in memory where it doesn't. NVENC's exports, and the
//! proxies of Vulkan's ProRes encoder and NVENC, are the frames of the same
//! jobs decoding on the CPU. Skips where the GPU in use isn't NVIDIA's, and
//! the ProRes jobs where it doesn't decode ProRes on Vulkan.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::export::encode::detect;
use vidcut_av::ffmpeg::codec::Id;
use vidcut_av::ffmpeg::format::Pixel;
use vidcut_av::ffmpeg::frame;
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::proxy::{ProxyEncoder, encoders};
use vidcut_core::export::{
    Container as OutContainer, EncoderChoice, ExportSettings, HdrMode, VideoCodec as OutCodec,
    VideoSettings,
};
use vidcut_core::preferences::ProxyCodec;
use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

/// The GPU decoder in use, where it is NVIDIA's.
fn nvidia() -> Option<GpuDecoder> {
    GpuDecoder::best().filter(|g| g.kind() == Kind::Nvdec)
}

/// `gpu`, where it decodes ProRes on Vulkan.
fn on_vulkan(gpu: &GpuDecoder) -> bool {
    gpu.kind_for(Id::PRORES) == Some(Kind::Vulkan) && gpu.why_not(Id::PRORES, || false).is_none()
}

/// The interlaced H.264 fixture: 320×180, 50 frames at 25 a second.
fn h264(top: bool) -> PathBuf {
    let spec = FixtureSpec::new(VideoCodec::H264, Container::Mkv, 320, 180, (25, 1), 50)
        .interlaced(top)
        .no_audio();
    fixture::ensure(&spec).unwrap()
}

/// The interlaced ProRes 422 HQ fixture: 640×360, 30 frames at 30 a second.
fn prores(top: bool) -> PathBuf {
    let spec = FixtureSpec::new(
        VideoCodec::ProRes422Hq,
        Container::Mov,
        640,
        360,
        (30, 1),
        30,
    )
    .interlaced(top);
    fixture::ensure(&spec).unwrap()
}

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

/// Whether `f` holds 10-bit samples.
fn ten_bit(f: &frame::Video) -> bool {
    matches!(
        f.format(),
        Pixel::YUV420P10LE | Pixel::YUV422P10LE | Pixel::YUV444P10LE
    )
}

/// Sample (`x`, `y`) of component `c` (Y, Cb, Cr) of `f`: planar YUV of 8
/// or 10 bits, or NV12.
fn sample(f: &frame::Video, c: usize, x: usize, y: usize) -> u16 {
    if c > 0 && f.format() == Pixel::NV12 {
        return u16::from(f.data(1)[y * f.stride(1) + 2 * x + c - 1]);
    }
    let data = f.data(c);
    if ten_bit(f) {
        let i = y * f.stride(c) + 2 * x;
        u16::from_le_bytes([data[i], data[i + 1]])
    } else {
        u16::from(data[y * f.stride(c) + x])
    }
}

/// The rows of `got` with a sample more than a level of 8 bits from
/// `cpu`'s, a planar picture's, as (component, row).
fn rows_off(cpu: &frame::Video, got: &frame::Video) -> Vec<(usize, usize)> {
    let level = if ten_bit(cpu) { 4 } else { 1 };
    let mut off = Vec::new();
    for c in 0..3 {
        let (w, h) = (cpu.plane_width(c) as usize, cpu.plane_height(c) as usize);
        for y in 0..h {
            if (0..w).any(|x| sample(got, c, x, y).abs_diff(sample(cpu, c, x, y)) > level) {
                off.push((c, y));
            }
        }
    }
    off
}

/// The frames of the file at `path`, decoded on the CPU.
fn decoded(path: &Path) -> Vec<Picture> {
    pictures(
        &mut VideoReader::open_input(&Input::File(path.to_path_buf())).unwrap(),
        usize::MAX,
    )
}

/// Checks that `got`, a job's frames, are `want`'s, those of the same job
/// decoding on the CPU, frame `k` showing barcode `k`.
fn the_same(got: &[Picture], want: &[Picture], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}");
    for (k, (g, w)) in got.iter().zip(want).enumerate() {
        assert_eq!(
            (g.pts, g.code),
            (w.pts, Some(k as u32)),
            "{what}: frame {k}"
        );
        assert_eq!(rows_off(&w.picture, &g.picture), [], "{what}: frame {k}");
    }
}

#[test]
fn interlaced_h264_deinterlaced_on_nvdec_is_the_cpus() {
    let Some(gpu) = nvidia() else {
        return;
    };
    for top in [true, false] {
        let input = Input::File(h264(top));
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

/// A piece of `frames` frames of the clip at `path`, from its first.
fn piece(path: &Path, frames: i64) -> VideoPiece {
    let info = vidcut_av::probe::probe(path, None).unwrap();
    let origin = info.video.and_then(|v| v.first_frame_time).unwrap_or(0.0);
    VideoPiece::Source {
        pace: Default::default(),
        media: path.display().to_string(),
        path: path.to_path_buf(),
        origin,
        source_frame: 0,
        frames,
        image: None,
    }
}

/// NVENC's H.264 at a quality that keeps barcodes, in Matroska, no sound.
fn nvenc() -> ExportSettings {
    ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec: OutCodec::H264,
            encoder: EncoderChoice::Nvenc,
            quality: Some(18),
            bitrate: None,
            speed: None,
            profile: None,
            bit_depth: None,
            width: None,
            height: None,
            hdr: HdrMode::Sdr,
            passes: None,
            first_pass: Default::default(),
            multipass: Default::default(),
            lookahead: None,
            hdr_metadata: Default::default(),
            mastering_display: None,
            max_cll: None,
        }),
        audio: None,
        declick: false,
    }
}

/// The frames of an NVENC export of the `frames` frames at `fps` of the
/// clip at `path`, at its own `size`, its piece decoding on `gpu` (`None`:
/// the CPU), each decoded on the CPU.
fn nvenc_export(
    path: &Path,
    frames: i64,
    fps: i64,
    (width, height): (u32, u32),
    gpu: Option<&GpuDecoder>,
) -> Vec<Picture> {
    let plan = RenderPlan {
        fps: Rational::new(fps, 1).unwrap(),
        width,
        height,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video: vec![piece(path, frames)],
        audio: Vec::new(),
        range: (0, frames),
        name: "interlaced".into(),
    };
    let dir = tempfile::Builder::new()
        .prefix("vidcut-interlaced-")
        .tempdir()
        .unwrap();
    let dest = dir.path().join("out.mkv");
    vidcut_av::export::run_decoding_on(
        &plan,
        &nvenc(),
        &dest,
        &detect(false),
        gpu,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    decoded(&dest)
}

#[test]
fn an_nvenc_export_of_interlaced_h264_decoded_on_nvdec_is_the_cpus() {
    let Some(gpu) = nvidia() else {
        return;
    };
    for top in [true, false] {
        let path = h264(top);
        let got = nvenc_export(&path, 50, 25, (320, 180), Some(&gpu));
        let want = nvenc_export(&path, 50, 25, (320, 180), None);
        the_same(&got, &want, &format!("top field first: {top}"));
    }
}

#[test]
fn an_nvenc_export_of_interlaced_prores_decoded_on_vulkan_is_the_cpus() {
    let Some(gpu) = nvidia().filter(on_vulkan) else {
        return;
    };
    for top in [true, false] {
        let path = prores(top);
        let got = nvenc_export(&path, 30, 30, (640, 360), Some(&gpu));
        let want = nvenc_export(&path, 30, 30, (640, 360), None);
        the_same(&got, &want, &format!("top field first: {top}"));
    }
}

/// The frames of `enc`'s proxy of the clip at `path`, at its own `size`,
/// decoding on `gpu` (`None`: the CPU), each decoded on the CPU.
fn proxy(
    enc: &ProxyEncoder,
    path: &Path,
    (w, h): (u32, u32),
    gpu: Option<&GpuDecoder>,
) -> Vec<Picture> {
    let dir = tempfile::Builder::new()
        .prefix("vidcut-interlaced-proxy-")
        .tempdir()
        .unwrap();
    let dest = dir.path().join("proxy.mkv");
    vidcut_av::proxy::write(
        &Input::File(path.to_path_buf()),
        w,
        h,
        &dest,
        None,
        std::slice::from_ref(enc),
        gpu,
        "",
    )
    .unwrap_or_else(|e| panic!("{} of {}: {e}", enc.name, path.display()));
    decoded(&dest)
}

#[test]
fn proxies_of_interlaced_prores_decoded_on_vulkan_are_the_cpus() {
    let Some(gpu) = nvidia().filter(on_vulkan) else {
        return;
    };
    let on_it = detect(true).on(gpu.gpu().expect("NVIDIA's GPU"));
    // Vulkan's ProRes encoder, and NVENC's AV1, or its HEVC on a GPU
    // without AV1.
    let vulkan = encoders(ProxyCodec::Prores, &on_it, &[])
        .into_iter()
        .find(|e| e.kind == EncoderChoice::Vulkan)
        .expect("Vulkan's ProRes encoder");
    let nvenc = [ProxyCodec::Av1, ProxyCodec::Hevc]
        .into_iter()
        .flat_map(|c| encoders(c, &on_it, &[]))
        .find(|e| e.kind == EncoderChoice::Nvenc)
        .expect("NVENC's AV1 or HEVC");
    for enc in [vulkan, nvenc] {
        for top in [true, false] {
            let path = prores(top);
            let got = proxy(&enc, &path, (640, 360), Some(&gpu));
            let want = proxy(&enc, &path, (640, 360), None);
            the_same(
                &got,
                &want,
                &format!("{}, top field first: {top}", enc.name),
            );
        }
    }
}
