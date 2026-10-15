//! Dissolves (decision D50): each frame of a fade from black and of a cross
//! dissolve at its level, exported on the CPU and on NVIDIA's GPU for
//! NVENC.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use vidcut_av::decode::{VideoReader, read_barcode};
use vidcut_av::export::{self, encode};
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture, VideoCodec};
use vidcut_core::export::{
    Container as OutContainer, EncoderChoice, ExportSettings, HdrMode, VideoCodec as VCodec,
    VideoSettings,
};
use vidcut_core::render::{Over, Ramp, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

fn fps() -> Rational {
    Rational::new(25, 1).unwrap()
}

/// A 320×180 clip of `picture`, 40 frames at 25 a second, no sound.
fn clip(codec: VideoCodec, picture: Picture) -> PathBuf {
    let spec = FixtureSpec::new(codec, Container::Mkv, 320, 180, (25, 1), 40)
        .picture(picture)
        .no_audio();
    fixture::ensure(&spec).unwrap()
}

fn piece(path: &Path, source_frame: i64, frames: i64) -> VideoPiece {
    let origin = vidcut_av::probe::probe(path, None)
        .unwrap()
        .video
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path: path.to_path_buf(),
        origin,
        source_frame,
        frames,
        image: None,
    }
}

/// `over` rising over `under` across all of their frames.
fn mix(under: VideoPiece, over: VideoPiece) -> VideoPiece {
    let of = under.frames();
    VideoPiece::Mix {
        under: Box::new(under),
        over: vec![Over {
            piece: over,
            ramp: Ramp {
                at: 0,
                of,
                falling: false,
            },
        }],
    }
}

fn plan(video: Vec<VideoPiece>) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    RenderPlan {
        fps: fps(),
        width: 320,
        height: 180,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio: vec![],
        range: (0, frames),
        name: "t".into(),
    }
}

/// H.264 without audio on `encoder`, lossless on the CPU.
fn settings(encoder: EncoderChoice) -> ExportSettings {
    ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec: VCodec::H264,
            encoder,
            quality: Some(if encoder == EncoderChoice::Cpu { 0 } else { 1 }),
            bitrate: None,
            speed: if encoder == EncoderChoice::Cpu {
                Some("ultrafast".into())
            } else {
                None
            },
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

fn export_on(
    plan: &RenderPlan,
    settings: &ExportSettings,
    available: &encode::Availability,
    decoder: Option<&vidcut_av::hw::GpuDecoder>,
    out: &Path,
) {
    export::run_decoding_on(
        plan,
        settings,
        out,
        available,
        decoder,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("export: {e}"));
}

/// The mean (Y, Cb, Cr) of the first stripe of [`Picture::Colors`] in
/// each frame of `path`, in its middle rows.
fn first_stripe(path: &Path) -> Vec<[f64; 3]> {
    let mut r = VideoReader::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        let mean = |p: usize| {
            let sub = if p == 0 { 1 } else { 2 };
            let (stride, data) = (f.stride(p), f.data(p));
            let h = f.height() as usize / sub;
            let (mut sum, mut n) = (0.0, 0.0);
            for y in h / 4..h * 3 / 4 {
                for x in 8 / sub..72 / sub {
                    sum += f64::from(data[y * stride + x]);
                    n += 1.0;
                }
            }
            sum / n
        };
        out.push([mean(0), mean(1), mean(2)]);
    }
    out
}

fn out_dir(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("vidcut-dissolve-{name}-"))
        .tempdir()
        .unwrap()
}

#[test]
fn a_fade_from_black_shows_each_frame_at_its_level() {
    let stripes = clip(VideoCodec::Ffv1, Picture::Colors);
    let p = plan(vec![
        mix(VideoPiece::Black { frames: 10 }, piece(&stripes, 0, 10)),
        piece(&stripes, 10, 5),
    ]);
    let dir = out_dir("fade");
    let out = dir.path().join("fade.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let luma: Vec<f64> = first_stripe(&out).iter().map(|m| m[0]).collect();
    // The stripe's BT.709 luma is 96: 16 + (96 − 16) × (k + ½) / 10, then 96.
    assert_eq!(
        luma,
        [
            20.0, 28.0, 36.0, 44.0, 52.0, 60.0, 68.0, 76.0, 84.0, 92.0, 96.0, 96.0, 96.0, 96.0,
            96.0
        ]
    );
}

#[test]
fn a_fade_from_black_takes_the_chroma_from_its_middle() {
    let stripes = clip(VideoCodec::Ffv1, Picture::Colors);
    let p = plan(vec![
        mix(VideoPiece::Black { frames: 10 }, piece(&stripes, 0, 10)),
        piece(&stripes, 10, 1),
    ]);
    let dir = out_dir("chroma");
    let out = dir.path().join("fade.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let cb: Vec<f64> = first_stripe(&out).iter().map(|m| m[1]).collect();
    // The stripe's BT.709 Cb is 94: 128 + (94 − 128) × (k + ½) / 10.
    assert_eq!(
        cb,
        [
            126.0, 123.0, 120.0, 116.0, 113.0, 109.0, 106.0, 103.0, 99.0, 96.0, 94.0
        ]
    );
}

#[test]
fn a_cross_dissolve_mixes_two_files_frame_by_frame() {
    let stripes = clip(VideoCodec::Ffv1, Picture::Colors);
    let black = clip(VideoCodec::Ffv1, Picture::Black);
    let p = plan(vec![
        piece(&stripes, 0, 2),
        mix(piece(&stripes, 2, 10), piece(&black, 0, 10)),
        piece(&black, 10, 2),
    ]);
    let dir = out_dir("cross");
    let out = dir.path().join("cross.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let luma: Vec<f64> = first_stripe(&out).iter().map(|m| m[0]).collect();
    // 96 − (96 − 16) × (k + ½) / 10 between the stripe's 96 and black.
    assert_eq!(
        luma,
        [
            96.0, 96.0, 92.0, 84.0, 76.0, 68.0, 60.0, 52.0, 44.0, 36.0, 28.0, 20.0, 16.0, 16.0
        ]
    );
}

#[test]
fn a_dissolve_inside_one_file_reads_it_twice_and_reads_on() {
    let codes = clip(VideoCodec::H264, Picture::Barcode);
    let p = plan(vec![
        piece(&codes, 0, 5),
        mix(piece(&codes, 5, 4), piece(&codes, 20, 4)),
        piece(&codes, 24, 5),
    ]);
    let dir = out_dir("one-file");
    let out = dir.path().join("one.mkv");
    export_on(
        &p,
        &settings(EncoderChoice::Cpu),
        &Default::default(),
        None,
        &out,
    );
    let mut r = VideoReader::open(&out).unwrap();
    let mut shown = Vec::new();
    while let Some(f) = r.next_frame().unwrap() {
        shown.push(read_barcode(&f).unwrap());
    }
    let plain: Vec<Option<u32>> = shown[..5].iter().chain(&shown[9..]).copied().collect();
    let want: Vec<Option<u32>> = (0..5).chain(24..29).map(Some).collect();
    assert_eq!(plain, want);
}

/// NVENC's pictures come from NVDEC's frames mixed on the GPU, and match
/// the CPU's within a level. Skips without NVIDIA's encoder and decoder.
#[test]
fn nvenc_fades_as_the_cpu_does() {
    let Some(gpu) = vidcut_av::hw::GpuDecoder::best().filter(|g| g.cuda().is_some()) else {
        return;
    };
    let available = encode::detect(false);
    if !available
        .encoders
        .iter()
        .any(|e| e.ok && e.encoder == "h264_nvenc")
    {
        return;
    }
    let stripes = clip(VideoCodec::H264, Picture::Colors);
    let black = clip(VideoCodec::H264, Picture::Black);
    let p = plan(vec![
        mix(VideoPiece::Black { frames: 10 }, piece(&stripes, 0, 10)),
        mix(piece(&stripes, 10, 10), piece(&black, 0, 10)),
    ]);
    let dir = out_dir("nvenc");
    let (cpu, nvenc) = (dir.path().join("cpu.mkv"), dir.path().join("nvenc.mkv"));
    export_on(&p, &settings(EncoderChoice::Cpu), &available, None, &cpu);
    export_on(
        &p,
        &settings(EncoderChoice::Nvenc),
        &available,
        Some(&gpu),
        &nvenc,
    );
    let (a, b) = (first_stripe(&cpu), first_stripe(&nvenc));
    assert_eq!(a.len(), 20);
    assert_eq!(b.len(), 20);
    for (k, (x, y)) in a.iter().zip(&b).enumerate() {
        for c in 0..3 {
            assert!(
                (x[c] - y[c]).abs() <= 1.0,
                "frame {k}, plane {c}: CPU {x:?}, NVENC {y:?}"
            );
        }
    }
}
