//! The viewer's shader (`gui::picture`) against the export's conversion: a
//! picture in each planar layout the CPU's decoders give, drawn on the GPU
//! at its own size, is within a level of `Picture::to_rgba` where it is
//! flat. Pictures turned a quarter have chroma that changes down the rows
//! they are stored in. Runs on any adapter, a software one too. Skips
//! without one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut::gui::picture::{Gpu, Screen};
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture as Content, VideoCodec};
use vidcut_av::player::{Config, Layout, ManualOutput, Picture, Player};
use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

/// The upright picture's size.
const W: u32 = 640;
const H: u32 = 360;

/// A device of its own on the first adapter, without a window.
fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: vidcut::gui::picture::features(&adapter),
        ..Default::default()
    }))
    .ok()?;
    Some(Gpu::new(&device, &queue))
}

/// The player's picture of frame 5 of `codec`'s stripes, decoded on the
/// CPU and stored turned against `rotation`.
fn picture(codec: VideoCodec, rotation: u16) -> Arc<Picture> {
    let spec = FixtureSpec::new(codec, Container::Mov, W, H, (30, 1), 10)
        .rotation(rotation)
        .picture(Content::Colors);
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path, None).unwrap();
    let origin = info.video.and_then(|v| v.first_frame_time).unwrap_or(0.0);
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: W,
        height: H,
        sample_rate: 48_000,
        channels: 2,
        frames: 10,
        video: vec![VideoPiece::Source {
            pace: Default::default(),
            media: "m".into(),
            path,
            origin,
            source_frame: 0,
            frames: 10,
            image: None,
        }],
        audio: Vec::new(),
        range: (0, 10),
        name: "pictures".into(),
    };
    let config = Config {
        audio_scrubbing: false,
        ..Config::default()
    };
    let player = Player::new(plan, config, Box::new(ManualOutput::new())).unwrap();
    player.seek(5);
    assert!(player.wait_ready(5, Duration::from_secs(20)));
    player.present(Instant::now()).unwrap().picture
}

/// How many pixels of `b` (RGBA rows `w` wide) are flat, like those two
/// away each way, and how far `a` is from `b` there at most, in levels of
/// a color channel.
fn flat_diff(a: &[u8], b: &[u8], w: usize) -> (usize, u8) {
    let h = b.len() / 4 / w;
    let px = |x: usize, y: usize| &b[(y * w + x) * 4..(y * w + x) * 4 + 3];
    let (mut flat, mut most) = (0, 0);
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let here = px(x, y);
            if [px(x - 2, y), px(x + 2, y), px(x, y - 2), px(x, y + 2)]
                .iter()
                .any(|q| *q != here)
            {
                continue;
            }
            let i = (y * w + x) * 4;
            for c in 0..3 {
                most = most.max(a[i + c].abs_diff(b[i + c]));
            }
            flat += 1;
        }
    }
    (flat, most)
}

/// `codec`'s picture, turned by `rotation` and in `layout`, drawn at its
/// own size: within a level of the export's conversion in each channel
/// wherever it is flat. At edges the export repeats each chroma sample
/// where the shader interpolates.
fn draws_as_converted(codec: VideoCodec, rotation: u16, layout: Layout) {
    let Some(gpu) = gpu() else {
        eprintln!("skipping: no GPU adapter, not even a software one");
        return;
    };
    let picture = picture(codec, rotation);
    assert_eq!(picture.layout(), layout);
    let theirs = picture.to_rgba(W, H).unwrap();
    let mut screen = Screen::new();
    screen.draw(&gpu, Some(&picture), W, H);
    let ours = screen.read(&gpu).unwrap();
    let (flat, most) = flat_diff(&ours.rgba, &theirs.rgba, W as usize);
    assert!(flat > (W * H / 2) as usize, "only {flat} pixels are flat");
    assert!(most <= 1, "{most} levels apart where the picture is flat");
}

#[test]
fn a_10_bit_4_2_2_picture_draws_as_the_export_converts_it() {
    draws_as_converted(VideoCodec::ProRes422Hq, 90, Layout::Yuv422p10);
}

#[test]
fn an_8_bit_4_2_2_picture_draws_as_the_export_converts_it() {
    // Not turned: DNxHR's pictures are tagged BT.709, and the export
    // converts a tagged 4:2:2 picture turned a quarter in other colors.
    draws_as_converted(VideoCodec::DnxHrHq, 0, Layout::Yuv422p);
}

#[test]
fn a_10_bit_planar_4_2_0_picture_draws_as_the_export_converts_it() {
    draws_as_converted(VideoCodec::Hevc10, 90, Layout::Yuv420p10);
}
