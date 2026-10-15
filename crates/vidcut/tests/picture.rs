//! The viewer's shader (`gui::picture`) against the export's conversion: a
//! picture in each planar layout the CPU's decoders give, drawn on the GPU
//! at its own size, is within a level of `Picture::to_rgba` where it is
//! flat. Pictures turned a quarter have chroma that changes down the rows
//! they are stored in. Runs on any adapter, a software one too. Skips
//! without one. A ProRes picture decoded on Vulkan and kept in GPU memory is
//! drawn from there, its planes imported, as closely; that test skips where
//! the GPU in use doesn't decode ProRes on Vulkan.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut::gui::picture::{Gpu, Screen};
use vidcut_av::ffmpeg::codec::Id;
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture as Content, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
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

/// A device of its own on the first Vulkan adapter that shows Vulkan's
/// pictures from GPU memory: one on the GPU that decodes them. Otherwise
/// what each adapter said.
fn vulkan_gpu() -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let mut why = Vec::new();
    for adapter in adapters {
        let name = adapter.get_info().name;
        let made = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: vidcut::gui::picture::features(&adapter),
            ..Default::default()
        }));
        match made {
            Ok((device, queue)) => {
                let gpu = Gpu::new(&device, &queue);
                let shows = gpu.gpu_pictures();
                if shows.contains(&Kind::Vulkan) {
                    return Ok(gpu);
                }
                why.push(format!("{name} shows {shows:?}"));
            }
            Err(e) => why.push(format!("{name}: {e}")),
        }
    }
    Err(why.join("; "))
}

/// The player's picture of frame 5 of `codec`'s stripes, decoded on the
/// CPU and stored turned against `rotation`.
fn picture(codec: VideoCodec, rotation: u16) -> Arc<Picture> {
    let config = Config {
        audio_scrubbing: false,
        ..Config::default()
    };
    picture_with(codec, rotation, config)
}

/// The player's picture of frame 5 of `codec`'s stripes, stored turned
/// against `rotation`, decoded and kept as `config` says.
fn picture_with(codec: VideoCodec, rotation: u16, config: Config) -> Arc<Picture> {
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
    draws_as_converted(VideoCodec::DnxHrHq, 90, Layout::Yuv422p);
}

#[test]
fn a_10_bit_planar_4_2_0_picture_draws_as_the_export_converts_it() {
    draws_as_converted(VideoCodec::Hevc10, 90, Layout::Yuv420p10);
}

/// A display device on NVIDIA's GPU in use, and the player's ProRes picture
/// decoded on Vulkan and kept in GPU memory for it; `None` where the GPU in
/// use doesn't decode ProRes on Vulkan.
fn kept_on_vulkan() -> Option<(Gpu, Arc<Picture>)> {
    let on_vulkan = GpuDecoder::best().is_some_and(|g| {
        g.kind_for(Id::PRORES) == Some(Kind::Vulkan) && g.why_not(Id::PRORES, || false).is_none()
    });
    if !on_vulkan {
        eprintln!("skipping: the GPU in use doesn't decode ProRes on Vulkan");
        return None;
    }
    let gpu = vulkan_gpu()
        .unwrap_or_else(|why| panic!("no display device on the GPU that decodes on Vulkan: {why}"));
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: gpu.gpu_pictures(),
        ..Config::default()
    };
    let picture = picture_with(VideoCodec::ProRes422Hq, 90, config);
    assert_eq!(picture.gpu(), Some(Kind::Vulkan));
    Some((gpu, picture))
}

#[test]
fn a_prores_picture_kept_on_vulkan_draws_from_gpu_memory_as_the_export_converts_it() {
    let Some((gpu, picture)) = kept_on_vulkan() else {
        return;
    };
    assert_eq!(picture.layout(), Layout::Yuv422p10);
    // The same picture, come down and converted by the export's filters.
    let theirs = picture.to_rgba(W, H).unwrap();
    let mut screen = Screen::new();
    screen.draw(&gpu, Some(&picture), W, H);
    assert!(screen.shared(), "the picture came down to be drawn");
    let ours = screen.read(&gpu).unwrap();
    let (flat, most) = flat_diff(&ours.rgba, &theirs.rgba, W as usize);
    assert!(flat > (W * H / 2) as usize, "only {flat} pixels are flat");
    assert!(most <= 1, "{most} levels apart where the picture is flat");
}

#[test]
fn a_prores_picture_kept_on_vulkan_draws_from_gpu_memory_again() {
    let Some((gpu, picture)) = kept_on_vulkan() else {
        return;
    };
    let mut screen = Screen::new();
    screen.draw(&gpu, Some(&picture), W, H);
    // Another size: its memory is imported again.
    screen.draw(&gpu, Some(&picture), W / 2, H / 2);
    assert!(screen.shared(), "the picture came down when drawn again");
}
