use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut::gui::picture::{Gpu, Screen};
use vidcut_av::ffmpeg::codec::Id;
use vidcut_av::fixture::{self, Container, FixtureSpec, Picture as Content, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, Layout, ManualOutput, Picture, Player};
use vidcut_core::render::{Over, Ramp, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

const W: u32 = 640;
const H: u32 = 360;

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

fn picture(codec: VideoCodec, rotation: u16) -> Arc<Picture> {
    let config = Config {
        audio_scrubbing: false,
        ..Config::default()
    };
    picture_with(codec, rotation, config)
}

fn picture_with(codec: VideoCodec, rotation: u16, config: Config) -> Arc<Picture> {
    played_with(clip(codec, rotation, Content::Colors), config)
}

fn clip(codec: VideoCodec, rotation: u16, content: Content) -> VideoPiece {
    clip_in(Container::Mov, codec, rotation, content)
}

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

fn played(piece: VideoPiece) -> Arc<Picture> {
    let config = Config {
        audio_scrubbing: false,
        ..Config::default()
    };
    played_with(piece, config)
}

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

fn draws_as_converted(codec: VideoCodec, rotation: u16, layout: Layout) {
    let picture = picture(codec, rotation);
    assert_eq!(picture.layout(), layout);
    draws_as_rgba(&picture);
}

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

fn draws_from_vulkan_as_converted(codec: VideoCodec, rotation: u16, layout: Layout) {
    let Some((gpu, picture)) = kept_on_vulkan(codec, rotation) else {
        return;
    };
    draws_from_gpu_as_converted(&gpu, &picture, layout);
}

fn draws_from_gpu_as_converted(gpu: &Gpu, picture: &Arc<Picture>, layout: Layout) {
    assert_eq!(picture.layout(), layout);
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
    screen.draw(&gpu, Some(&picture), W, H);
    assert_eq!(screen.imports(), 1);
    let ours = screen.read(&gpu).unwrap();
    let (flat, most) = flat_diff(&ours.rgba, &theirs.rgba, W as usize);
    assert!(flat > (W * H / 2) as usize, "only {flat} pixels are flat");
    assert!(most <= 1, "{most} levels apart where the picture is flat");
}

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

fn gpu_on_node(node: &str) -> Option<Gpu> {
    let name = std::path::Path::new(node).file_name()?;
    let device = std::fs::canonicalize(
        std::path::Path::new("/sys/class/drm")
            .join(name)
            .join("device"),
    )
    .ok()?;
    let slot = device.file_name()?.to_str()?.to_string();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN))
        .into_iter()
        .find(|a| a.get_info().device_pci_bus_id == slot)?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: vidcut::gui::picture::features(&adapter),
        ..Default::default()
    }))
    .ok()?;
    Some(Gpu::new(&device, &queue))
}

fn kept_on_vaapi(codec: VideoCodec) -> Option<(Gpu, Arc<Picture>)> {
    let Some(decoder) = GpuDecoder::best().filter(|g| g.kind() == Kind::Vaapi) else {
        eprintln!("skipping: the GPU decoder in use isn't VA-API");
        return None;
    };
    let Some(gpu) = gpu_on_node(decoder.on()) else {
        eprintln!(
            "skipping: no Vulkan device found at the PCI address of VA-API's node {:?}",
            decoder.on()
        );
        return None;
    };
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: gpu.gpu_pictures(),
        ..Config::default()
    };
    let picture = played_with(clip_in(Container::Mkv, codec, 0, Content::Colors), config);
    assert_eq!(picture.gpu(), Some(Kind::Vaapi));
    Some((gpu, picture))
}

#[test]
fn an_h264_picture_kept_on_vaapi_draws_from_gpu_memory_as_the_export_converts_it() {
    let Some((gpu, picture)) = kept_on_vaapi(VideoCodec::H264) else {
        return;
    };
    draws_from_gpu_as_converted(&gpu, &picture, Layout::Nv12);
}

#[test]
fn a_10_bit_hevc_picture_kept_on_vaapi_draws_from_gpu_memory_as_the_export_converts_it() {
    let Some((gpu, picture)) = kept_on_vaapi(VideoCodec::Hevc10) else {
        return;
    };
    draws_from_gpu_as_converted(&gpu, &picture, Layout::P010);
}

#[test]
fn an_av1_picture_kept_on_nvdec_takes_its_rows_padded_to_cudas_alignment() {
    let Some((_, picture)) = kept_on_nvdec(VideoCodec::Av1, Id::AV1) else {
        return;
    };
    assert_eq!(picture.bytes(), 1536 * 540);
}
