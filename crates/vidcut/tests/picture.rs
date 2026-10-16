//! The viewer's shader (`gui::picture`) against the export's conversion: a
//! picture in each planar layout the CPU's decoders give, drawn on the GPU
//! at its own size, is within a level of `Picture::to_rgba` where it is
//! flat. Pictures turned a quarter have chroma that changes down the rows
//! they are stored in, upright ones along the rows. A picture with alpha is
//! drawn without it, as the export converts it. A dissolve's pictures are
//! drawn over each other as `Picture::to_rgba` mixes them. Runs on any
//! adapter, a software one too. Skips without one. ProRes pictures decoded
//! on Vulkan and kept in GPU memory are drawn from there, their planes
//! imported, as closely, and drawn again from the same import; those tests
//! skip where the GPU in use doesn't decode ProRes on Vulkan. AV1 and H.264
//! pictures decoded on NVDEC and kept in memory of their own are drawn
//! from there as closely; those skip without NVDEC.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut::gui::picture::{Gpu, Screen};
use vidcut_av::ffmpeg::codec::Id;
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture as Content, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, Layout, ManualOutput, Picture, Player};
use vidcut_core::render::{Over, Ramp, RenderPlan, VideoPiece};
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

/// A device of its own on the first Vulkan adapter that shows `kind`'s
/// pictures from GPU memory: one on the GPU that decodes them. Otherwise
/// what each adapter said.
fn gpu_showing(kind: Kind) -> Result<Gpu, String> {
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
                if shows.contains(&kind) {
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
    played_with(clip(codec, rotation, Content::Colors), config)
}

/// Ten frames of `content` in `codec`, stored turned against `rotation`.
fn clip(codec: VideoCodec, rotation: u16, content: Content) -> VideoPiece {
    clip_in(Container::Mov, codec, rotation, content)
}

/// [`clip`] in `container`.
fn clip_in(container: Container, codec: VideoCodec, rotation: u16, content: Content) -> VideoPiece {
    let spec = FixtureSpec::new(codec, container, W, H, (30, 1), 10)
        .rotation(rotation)
        .picture(content);
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path, None).unwrap();
    let origin = info.video.and_then(|v| v.first_frame_time).unwrap_or(0.0);
    VideoPiece::Source {
        pace: Default::default(),
        media: "m".into(),
        path,
        origin,
        source_frame: 0,
        frames: 10,
        image: None,
    }
}

/// `over` rising over `under` through their ten frames.
fn dissolve(under: VideoPiece, over: VideoPiece) -> VideoPiece {
    VideoPiece::Mix {
        under: Box::new(under),
        over: vec![Over {
            piece: over,
            ramp: Ramp {
                at: 0,
                of: 10,
                falling: false,
            },
        }],
    }
}

/// The player's picture of frame 5 of `piece`, ten frames long, decoded on
/// the CPU.
fn played(piece: VideoPiece) -> Arc<Picture> {
    let config = Config {
        audio_scrubbing: false,
        ..Config::default()
    };
    played_with(piece, config)
}

/// The player's picture of frame 5 of `piece`, ten frames long, decoded
/// and kept as `config` says.
fn played_with(piece: VideoPiece, config: Config) -> Arc<Picture> {
    let plan = RenderPlan {
        fps: Rational::new(30, 1).unwrap(),
        width: W,
        height: H,
        sample_rate: 48_000,
        channels: 2,
        frames: 10,
        video: vec![piece],
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
    let picture = picture(codec, rotation);
    assert_eq!(picture.layout(), layout);
    draws_as_rgba(&picture);
}

/// `picture` drawn at its own size: within a level of `Picture::to_rgba`
/// (the export's conversion, a dissolve's pictures mixed in its RGB) in
/// each channel wherever it is flat.
fn draws_as_rgba(picture: &Arc<Picture>) {
    let Some(gpu) = gpu() else {
        eprintln!("skipping: no GPU adapter, not even a software one");
        return;
    };
    let theirs = picture.to_rgba(W, H).unwrap();
    let mut screen = Screen::new();
    screen.draw(&gpu, Some(picture), W, H);
    let ours = screen.read(&gpu).unwrap();
    let (flat, most) = flat_diff(&ours.rgba, &theirs.rgba, W as usize);
    assert!(flat > (W * H / 2) as usize, "only {flat} pixels are flat");
    assert!(most <= 1, "{most} levels apart where the picture is flat");
}

#[test]
fn a_fade_from_black_draws_as_its_rgba_mix() {
    let stripes = clip(VideoCodec::H264, 0, Content::Colors);
    let picture = played(dissolve(VideoPiece::Black { frames: 10 }, stripes));
    assert_eq!(picture.over().len(), 1);
    draws_as_rgba(&picture);
}

#[test]
fn a_cross_dissolve_of_two_layouts_draws_as_its_rgba_mix() {
    let stripes = clip(VideoCodec::H264, 0, Content::Colors);
    let turned = clip(VideoCodec::ProRes422Hq, 90, Content::Colors);
    let picture = played(dissolve(stripes, turned));
    assert_eq!(picture.over().len(), 1);
    draws_as_rgba(&picture);
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

#[test]
fn a_12_bit_4_4_4_picture_draws_as_the_export_converts_it() {
    draws_as_converted(VideoCodec::ProRes4444, 0, Layout::Yuv444p12);
}

#[test]
fn a_12_bit_4_4_4_picture_with_alpha_draws_as_the_export_converts_it() {
    draws_as_converted(VideoCodec::ProRes4444Alpha, 90, Layout::Yuv444p12);
}

/// A display device on NVIDIA's GPU in use, and the player's picture of
/// `codec`, a ProRes stored turned against `rotation`, decoded on Vulkan
/// and kept in GPU memory for it; `None` where the GPU in use doesn't
/// decode ProRes on Vulkan.
fn kept_on_vulkan(codec: VideoCodec, rotation: u16) -> Option<(Gpu, Arc<Picture>)> {
    let on_vulkan = GpuDecoder::best().is_some_and(|g| {
        g.kind_for(Id::PRORES) == Some(Kind::Vulkan) && g.why_not(Id::PRORES, || false).is_none()
    });
    if !on_vulkan {
        eprintln!("skipping: the GPU in use doesn't decode ProRes on Vulkan");
        return None;
    }
    let gpu = gpu_showing(Kind::Vulkan)
        .unwrap_or_else(|why| panic!("no display device on the GPU that decodes on Vulkan: {why}"));
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: gpu.gpu_pictures(),
        ..Config::default()
    };
    let picture = picture_with(codec, rotation, config);
    assert_eq!(picture.gpu(), Some(Kind::Vulkan));
    Some((gpu, picture))
}

/// `codec`'s picture, turned by `rotation` and in `layout`, kept on Vulkan
/// and drawn from GPU memory: within a level of the export's conversion
/// wherever it is flat.
fn draws_from_vulkan_as_converted(codec: VideoCodec, rotation: u16, layout: Layout) {
    let Some((gpu, picture)) = kept_on_vulkan(codec, rotation) else {
        return;
    };
    draws_from_gpu_as_converted(&gpu, &picture, layout);
}

/// `picture`, in `layout` and kept in GPU memory, drawn from there: within
/// a level of the export's conversion wherever it is flat.
fn draws_from_gpu_as_converted(gpu: &Gpu, picture: &Arc<Picture>, layout: Layout) {
    assert_eq!(picture.layout(), layout);
    // The same picture, come down and converted by the export's filters.
    let theirs = picture.to_rgba(W, H).unwrap();
    let mut screen = Screen::new();
    screen.draw(gpu, Some(picture), W, H);
    assert!(screen.shared(), "the picture came down to be drawn");
    let ours = screen.read(gpu).unwrap();
    let (flat, most) = flat_diff(&ours.rgba, &theirs.rgba, W as usize);
    assert!(flat > (W * H / 2) as usize, "only {flat} pixels are flat");
    assert!(most <= 1, "{most} levels apart where the picture is flat");
}

#[test]
fn a_prores_picture_kept_on_vulkan_draws_from_gpu_memory_as_the_export_converts_it() {
    draws_from_vulkan_as_converted(VideoCodec::ProRes422Hq, 90, Layout::Yuv422p10);
}

#[test]
fn a_prores_4444_picture_kept_on_vulkan_draws_from_gpu_memory_as_the_export_converts_it() {
    draws_from_vulkan_as_converted(VideoCodec::ProRes4444, 0, Layout::Yuv444p12);
}

#[test]
fn a_prores_4444_picture_with_alpha_kept_on_vulkan_draws_from_gpu_memory_without_it() {
    draws_from_vulkan_as_converted(VideoCodec::ProRes4444Alpha, 90, Layout::Yuv444p12);
}

#[test]
fn a_prores_picture_kept_on_vulkan_draws_from_gpu_memory_again() {
    let Some((gpu, picture)) = kept_on_vulkan(VideoCodec::ProRes422Hq, 90) else {
        return;
    };
    let mut first = Screen::new();
    first.draw(&gpu, Some(&picture), W, H);
    // Another screen imports its memory again.
    let mut second = Screen::new();
    second.draw(&gpu, Some(&picture), W, H);
    assert!(second.shared(), "the picture came down when imported again");
}

#[test]
fn a_prores_picture_kept_on_vulkan_and_drawn_again_is_imported_once() {
    let Some((gpu, picture)) = kept_on_vulkan(VideoCodec::ProRes422Hq, 90) else {
        return;
    };
    let theirs = picture.to_rgba(W, H).unwrap();
    let mut screen = Screen::new();
    screen.draw(&gpu, Some(&picture), W / 2, H / 2);
    // Another size: drawn again from the same import.
    screen.draw(&gpu, Some(&picture), W, H);
    assert_eq!(screen.imports(), 1);
    let ours = screen.read(&gpu).unwrap();
    let (flat, most) = flat_diff(&ours.rgba, &theirs.rgba, W as usize);
    assert!(flat > (W * H / 2) as usize, "only {flat} pixels are flat");
    assert!(most <= 1, "{most} levels apart where the picture is flat");
}

/// A display device on NVIDIA's GPU in use, and the player's picture of
/// `codec` (`id`) in Matroska, decoded on NVDEC and kept in GPU memory for
/// it; `None` where the GPU in use doesn't decode `id` on NVDEC.
fn kept_on_nvdec(codec: VideoCodec, id: Id) -> Option<(Gpu, Arc<Picture>)> {
    let on_nvdec = GpuDecoder::best()
        .is_some_and(|g| g.kind_for(id) == Some(Kind::Nvdec) && g.why_not(id, || false).is_none());
    if !on_nvdec {
        eprintln!("skipping: the GPU in use doesn't decode {id:?} on NVDEC");
        return None;
    }
    let gpu = gpu_showing(Kind::Nvdec)
        .unwrap_or_else(|why| panic!("no display device on the GPU NVDEC decodes on: {why}"));
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: gpu.gpu_pictures(),
        ..Config::default()
    };
    let picture = played_with(clip_in(Container::Mkv, codec, 0, Content::Colors), config);
    assert_eq!(picture.gpu(), Some(Kind::Nvdec));
    Some((gpu, picture))
}

#[test]
fn an_av1_picture_kept_on_nvdec_draws_from_gpu_memory_as_the_export_converts_it() {
    let Some((gpu, picture)) = kept_on_nvdec(VideoCodec::Av1, Id::AV1) else {
        return;
    };
    draws_from_gpu_as_converted(&gpu, &picture, Layout::P010);
}

#[test]
fn an_h264_picture_kept_on_nvdec_draws_from_gpu_memory_as_the_export_converts_it() {
    let Some((gpu, picture)) = kept_on_nvdec(VideoCodec::H264, Id::H264) else {
        return;
    };
    draws_from_gpu_as_converted(&gpu, &picture, Layout::Nv12);
}

#[test]
fn an_av1_picture_kept_on_nvdec_takes_its_rows_padded_to_cudas_alignment() {
    let Some((_, picture)) = kept_on_nvdec(VideoCodec::Av1, Id::AV1) else {
        return;
    };
    // 640 samples of 2 bytes in rows of 1536, 360 rows and 180 of chroma.
    assert_eq!(picture.bytes(), 1536 * 540);
}
