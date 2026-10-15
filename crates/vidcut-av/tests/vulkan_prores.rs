//! ProRes decoded on the GPU in use through FFmpeg's Vulkan decoder
//! (NVIDIA's, FFmpeg 8.1 and later): its frames are the CPU decoder's,
//! barcode and timestamp, read through and after seeks, and each comes
//! down to memory once. A player keeps each decoder's pictures in GPU
//! memory only where its display shows that decoder's. Skips where the GPU
//! in use doesn't decode ProRes on Vulkan.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut_av::copies::{Copies, Guard};
use vidcut_av::decode::{Input, VideoReader, read_barcode};
use vidcut_av::ffmpeg::codec::Id;
use vidcut_av::fixture::{self, Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, Decoder, ManualOutput, Player};
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
