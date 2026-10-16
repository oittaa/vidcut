//! ProRes decoded on the GPU in use through FFmpeg's Vulkan decoder
//! (NVIDIA's, FFmpeg 8.1 and later): its frames are the CPU decoder's,
//! barcode and timestamp, read through and after seeks, and each comes
//! down to memory once. Interlaced, its frames are the CPU's deinterlaced
//! ones within a level, deinterlaced on the GPU where `bwdif_vulkan` gives
//! `bwdif`'s frames and in memory where it doesn't. An NVENC export of it
//! makes every frame on the GPU, nothing copied, and a CPU encoder's export
//! at a smaller size gets each frame shrunk there before it comes down. A
//! player keeps each decoder's pictures in GPU memory only where its
//! display shows that decoder's, and counts each picture it brings down:
//! none where its display shows Vulkan's. A picture kept on Vulkan counts
//! the memory of its images, and the picture prepared for the plan played
//! next comes down to memory: held, it would hold its decoder's whole pool.
//! Skips where the GPU in use doesn't decode ProRes on Vulkan.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use vidcut_av::copies::{Copies, Guard};
use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::ffmpeg::codec::Id;
use vidcut_av::ffmpeg::{ffi, frame};
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, Decoder, ManualOutput, Player};
use vidcut_core::editor::Copied;
use vidcut_core::export::{
    Container as OutContainer, EncoderChoice, ExportSettings, HdrMode, VideoCodec as OutCodec,
    VideoSettings,
};
use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

/// The GPU decoder in use, where it decodes ProRes on Vulkan.
fn on_vulkan() -> Option<GpuDecoder> {
    let gpu = GpuDecoder::best()?;
    let vulkan = gpu.kind_for(Id::PRORES) == Some(Kind::Vulkan);
    (vulkan && gpu.why_not(Id::PRORES, || false).is_none()).then_some(gpu)
}

/// `frames` frames of `codec`'s barcodes at 640×360 and 30 a second.
fn clip(codec: VideoCodec, container: Container, frames: u32) -> std::path::PathBuf {
    let spec = FixtureSpec::new(codec, container, 640, 360, (30, 1), frames);
    fixture::ensure(&spec).unwrap()
}

/// Up to `n` frames on from where `reader` is: timestamps and barcodes,
/// each frame in GPU memory brought down first.
fn read_on(reader: &mut VideoReader, n: usize) -> Vec<(i64, Option<u32>)> {
    let mut out = Vec::new();
    while out.len() < n {
        let Some(f) = reader.next_frame().unwrap() else {
            break;
        };
        let f = if vidcut_av::hw::on_gpu(&f) {
            vidcut_av::hw::download(&f).unwrap()
        } else {
            f
        };
        out.push((f.pts().expect("pts"), read_barcode(&f).unwrap()));
    }
    out
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

/// The rows of `got`, a 10-bit 4:2:2 picture, with a sample more than a
/// level of 8 bits (4 codes) from `cpu`'s, as (plane, row).
fn rows_off(cpu: &frame::Video, got: &frame::Video) -> Vec<(usize, usize)> {
    let mut off = Vec::new();
    for p in 0..cpu.planes() {
        let (w, h) = (cpu.plane_width(p) as usize, cpu.plane_height(p) as usize);
        let sample = |f: &frame::Video, x: usize, y: usize| {
            let i = y * f.stride(p) + 2 * x;
            u16::from_le_bytes([f.data(p)[i], f.data(p)[i + 1]])
        };
        for y in 0..h {
            if (0..w).any(|x| sample(got, x, y).abs_diff(sample(cpu, x, y)) > 4) {
                off.push((p, y));
            }
        }
    }
    off
}

#[test]
fn interlaced_prores_deinterlaced_on_vulkan_is_the_cpus() {
    let Some(gpu) = on_vulkan() else {
        return;
    };
    for top in [true, false] {
        let spec = FixtureSpec::new(
            VideoCodec::ProRes422Hq,
            Container::Mov,
            640,
            360,
            (30, 1),
            30,
        )
        .interlaced(top);
        let input = Input::File(fixture::ensure(&spec).unwrap());
        let truth = pictures(&mut VideoReader::open_input(&input).unwrap(), usize::MAX);
        assert_eq!(truth.len(), 30);
        let mut reader = VideoReader::open_on(&input, &gpu).unwrap();
        assert_eq!(reader.gpu(), Some(Kind::Vulkan));
        let got = pictures(&mut reader, usize::MAX);
        assert_eq!(got.len(), truth.len(), "top field first: {top}");
        for (k, (g, t)) in got.iter().zip(&truth).enumerate() {
            let what = format!("top field first: {top}, frame {k}");
            // Deinterlaced on the GPU, or in memory where bwdif_vulkan
            // doesn't give bwdif's frames.
            assert!(matches!(g.kind, Some(Kind::Vulkan) | None), "{what}");
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

#[test]
fn prores_decoded_on_vulkan_is_the_cpus() {
    let Some(gpu) = on_vulkan() else {
        return;
    };
    let input = Input::File(clip(VideoCodec::ProRes422Hq, Container::Mov, 30));
    let truth = read_on(&mut VideoReader::open_input(&input).unwrap(), usize::MAX);
    assert_eq!(truth.len(), 30);
    let copies = Arc::new(Copies::default());
    let _counting = Guard::set(copies.clone());
    let mut reader = VideoReader::open_on(&input, &gpu).unwrap();
    assert_eq!(reader.gpu(), Some(Kind::Vulkan));
    assert_eq!(read_on(&mut reader, usize::MAX), truth);
    assert_eq!(copies.counted().down, 30);
    for (k, (pts, _)) in truth.iter().enumerate() {
        reader.seek(*pts).unwrap();
        let want = &truth[k..(k + 3).min(truth.len())];
        assert_eq!(read_on(&mut reader, 3), want, "after a seek to frame {k}");
    }
}

/// A piece of `frames` frames of the clip at `path`, from its first.
fn piece(path: std::path::PathBuf, frames: i64) -> VideoPiece {
    let info = vidcut_av::probe::probe(&path, None).unwrap();
    let origin = info.video.and_then(|v| v.first_frame_time).unwrap_or(0.0);
    VideoPiece::Source {
        pace: Default::default(),
        media: path.display().to_string(),
        path,
        origin,
        source_frame: 0,
        frames,
        image: None,
    }
}

#[test]
fn a_player_keeps_nvdecs_pictures_on_the_gpu_and_brings_vulkans_down() {
    let Some(gpu) = on_vulkan().filter(|g| g.kind() == Kind::Nvdec) else {
        return;
    };
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: 640,
        height: 360,
        sample_rate: 48_000,
        channels: 2,
        frames: 20,
        // ProRes first: a Vulkan picture the player failed to keep would
        // bring every picture after it down too.
        video: vec![
            piece(clip(VideoCodec::ProRes422Hq, Container::Mov, 10), 10),
            piece(clip(VideoCodec::H264, Container::Mp4, 10), 10),
        ],
        audio: Vec::new(),
        range: (0, 20),
        name: "kinds".into(),
    };
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        // A display that shows NVDEC's pictures from GPU memory, as the
        // viewer does on NVIDIA.
        gpu_pictures: vec![gpu.kind()],
        ..Config::default()
    };
    let player = Player::new(plan, config, Box::new(ManualOutput::new())).unwrap();
    let shown = |frame: i64| {
        player.seek(frame);
        assert!(player.wait_ready(frame, Duration::from_secs(20)));
        player.present(Instant::now()).unwrap().picture
    };
    let prores = shown(5);
    assert_eq!(prores.decoder(), Some(Decoder::Gpu(Kind::Vulkan)));
    assert_eq!(prores.gpu(), None);
    let h264 = shown(15);
    assert_eq!(h264.gpu(), Some(Kind::Nvdec));
}

/// A player of the 10 frames of the ProRes fixture whose display shows the
/// pictures of `shows` from GPU memory, each of its frames decoded: shown
/// once.
fn played_through(shows: Vec<Kind>) -> Player {
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: 640,
        height: 360,
        sample_rate: 48_000,
        channels: 2,
        frames: 10,
        video: vec![piece(clip(VideoCodec::ProRes422Hq, Container::Mov, 10), 10)],
        audio: Vec::new(),
        range: (0, 10),
        name: "copies".into(),
    };
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: shows,
        ..Config::default()
    };
    let player = Player::new(plan, config, Box::new(ManualOutput::new())).unwrap();
    for frame in 0..10 {
        player.seek(frame);
        assert!(player.wait_ready(frame, Duration::from_secs(20)));
    }
    player
}

#[test]
fn a_player_whose_display_takes_no_vulkan_pictures_copies_each_prores_frame_down_once() {
    let Some(gpu) = on_vulkan().filter(|g| g.kind() == Kind::Nvdec) else {
        return;
    };
    let player = played_through(vec![gpu.kind()]);
    // 640×360 at 4:2:2 in 16-bit samples: 921,600 bytes a frame.
    let want = Copied {
        down: 10,
        down_bytes: 9_216_000,
        up: 0,
        up_bytes: 0,
    };
    assert_eq!(player.stats().copies, want);
}

#[test]
fn a_player_whose_display_takes_vulkans_pictures_copies_none_of_them() {
    let Some(gpu) = on_vulkan().filter(|g| g.kind() == Kind::Nvdec) else {
        return;
    };
    let player = played_through(vec![gpu.kind(), Kind::Vulkan]);
    assert_eq!(player.stats().copies, Copied::default());
}

#[test]
fn a_picture_kept_on_vulkan_counts_the_memory_of_its_images() {
    let Some(gpu) = on_vulkan().filter(|g| g.kind() == Kind::Nvdec) else {
        return;
    };
    let player = played_through(vec![gpu.kind(), Kind::Vulkan]);
    let kept = player.present(Instant::now()).unwrap().picture;
    assert_eq!(kept.gpu(), Some(Kind::Vulkan));
    // SAFETY: a Vulkan frame's AVVkFrame, alive as long as it.
    let images: usize = unsafe {
        let vkf = &*((*kept.frame().as_ptr()).data[0] as *const ffi::AVVkFrame);
        vkf.mem
            .iter()
            .zip(vkf.size)
            .filter(|(m, _)| **m != 0)
            .map(|(_, s)| s)
            .sum()
    };
    assert_eq!(kept.bytes(), images);
}

#[test]
fn a_prepared_prores_picture_comes_down_to_memory() {
    let Some(gpu) = on_vulkan().filter(|g| g.kind() == Kind::Nvdec) else {
        return;
    };
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: 640,
        height: 360,
        sample_rate: 48_000,
        channels: 2,
        frames: 10,
        video: vec![piece(clip(VideoCodec::ProRes422Hq, Container::Mov, 10), 10)],
        audio: Vec::new(),
        range: (0, 10),
        name: "prepared".into(),
    };
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: vec![gpu.kind(), Kind::Vulkan],
        ..Config::default()
    };
    let player = Player::new(plan.clone(), config, Box::new(ManualOutput::new())).unwrap();
    player.prepare(Some((plan.clone(), 7)));
    let started = Instant::now();
    while !player.status().prepared {
        assert!(started.elapsed() < Duration::from_secs(20), "not prepared");
        std::thread::sleep(Duration::from_millis(10));
    }
    player.set_plan_at(plan, 7).unwrap();
    let prepared = player.present(Instant::now()).unwrap().picture;
    assert_eq!(prepared.decoder(), Some(Decoder::Gpu(Kind::Vulkan)));
    assert_eq!(prepared.gpu(), None);
}

/// `encoder`'s H.264 at a quality that keeps barcodes, in Matroska, no
/// sound.
fn h264(encoder: EncoderChoice) -> ExportSettings {
    ExportSettings {
        container: OutContainer::Mkv,
        video: Some(VideoSettings {
            codec: OutCodec::H264,
            encoder,
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

/// The barcodes and timestamps (seconds) of the frames of `path`.
fn coded(path: &std::path::Path) -> Vec<(Option<u32>, f64)> {
    let mut reader = VideoReader::open_input(&Input::File(path.to_path_buf())).unwrap();
    let tb = reader.time_base();
    let tick = f64::from(tb.numerator()) / f64::from(tb.denominator());
    let mut out = Vec::new();
    while let Some(f) = reader.next_frame().unwrap() {
        out.push((
            read_barcode(&f).unwrap(),
            f.pts().expect("pts") as f64 * tick,
        ));
    }
    out
}

/// The ProRes fixture's 30 frames exported at `out` with `encoder`'s H.264,
/// its pieces decoding on `gpu`, each with the source's barcode, timed at
/// 30 a second: the report.
fn exported(
    gpu: &GpuDecoder,
    encoder: EncoderChoice,
    out: (u32, u32),
) -> vidcut_av::export::Report {
    let path = clip(VideoCodec::ProRes422Hq, Container::Mov, 30);
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: out.0,
        height: out.1,
        sample_rate: 48_000,
        channels: 2,
        frames: 30,
        video: vec![piece(path, 30)],
        audio: Vec::new(),
        range: (0, 30),
        name: "nvenc".into(),
    };
    let dir = tempfile::Builder::new()
        .prefix("vidcut-vulkan-nvenc-")
        .tempdir()
        .unwrap();
    let dest = dir.path().join("out.mkv");
    let report = vidcut_av::export::run_decoding_on(
        &plan,
        &h264(encoder),
        &dest,
        &vidcut_av::export::encode::detect(false),
        Some(gpu),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let what = format!("{encoder:?} at {}x{}", out.0, out.1);
    let want: Vec<(Option<u32>, f64)> = (0..30).map(|k| (Some(k), f64::from(k) / 30.0)).collect();
    let got = coded(&dest);
    assert_eq!(got.len(), want.len(), "{what}");
    for (k, ((got_code, got_t), (want_code, want_t))) in got.iter().zip(&want).enumerate() {
        assert_eq!(got_code, want_code, "{what}: frame {k}");
        assert!(
            (got_t - want_t).abs() <= 1.5e-3,
            "{what}: frame {k} at {got_t}"
        );
    }
    report
}

#[test]
fn an_nvenc_export_of_prores_makes_every_frame_on_the_gpu() {
    let Some(gpu) = on_vulkan().filter(|g| g.kind() == Kind::Nvdec) else {
        return;
    };
    // At its own size, and shrunk and placed on black.
    for out in [(640, 360), (480, 360)] {
        let report = exported(&gpu, EncoderChoice::Nvenc, out);
        assert_eq!(report.gpu_frames, 30, "at {out:?}");
        assert_eq!(report.copies, Copied::default(), "at {out:?}");
    }
}

#[test]
fn a_cpu_encoders_export_of_prores_gets_its_frames_shrunk_on_the_gpu() {
    let Some(gpu) = on_vulkan() else {
        return;
    };
    let report = exported(&gpu, EncoderChoice::Cpu, (320, 180));
    // Each frame came down at 320x180 in 10-bit 4:2:2, as decoded: 230,400
    // bytes, a quarter of its own.
    let want = Copied {
        down: 30,
        down_bytes: 6_912_000,
        up: 0,
        up_bytes: 0,
    };
    assert_eq!(report.copies, want);
}
