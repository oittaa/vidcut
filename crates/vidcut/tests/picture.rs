//! The viewer's shader (`gui::picture`) against the export's conversion:
//! each kind of picture the player hands out (8-bit planar, P010, NV12,
//! turned every way, pillarboxed, SD and HD colors, full range, PQ and HLG)
//! drawn on the GPU shows the barcode `frame render` shows, and its pixels
//! are within a level of `Picture::to_rgba`, the export's own
//! filters. Pictures NVDEC, VA-API and Direct3D 11 decoded show from GPU
//! memory as they do copied down. Runs on any adapter, lavapipe too; skips
//! without one, and each GPU's checks without that GPU.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vidcut::gui::picture::{Drawn, Gpu, Screen};
use vidcut_av::fixture::{self, Container, FixtureSpec, ImageSpec, Picture as Content, VideoCodec};
use vidcut_av::player::{Config, Layout, ManualOutput, Memory, Picture, Player};
use vidcut_core::model::Hdr;
use vidcut_core::render::{Image, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

const W: u32 = 320;
const H: u32 = 180;

fn fps() -> Rational {
    Rational::new(30, 1).unwrap()
}

/// A piece of `frames` frames from `spec`'s fixture.
fn piece(spec: FixtureSpec, from: i64, frames: i64) -> VideoPiece {
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    VideoPiece::Source {
        media: "m".into(),
        path,
        origin,
        source_frame: from,
        frames,
        image: None,
    }
}

fn plan(video: Vec<VideoPiece>) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    RenderPlan {
        fps: fps(),
        width: W,
        height: H,
        sample_rate: 48_000,
        channels: 2,
        frames,
        video,
        audio: Vec::new(),
        range: (0, frames),
        name: "pictures".into(),
    }
}

/// The single-frame piece timeline frame `t` shows.
fn piece_at(plan: &RenderPlan, t: i64) -> VideoPiece {
    let mut k = t;
    for p in &plan.video {
        if k < p.frames() {
            let mut one = p.clone();
            match &mut one {
                VideoPiece::Source {
                    source_frame,
                    frames,
                    image,
                    ..
                } => {
                    if !matches!(image, Some(Image::Still)) {
                        *source_frame += k;
                    }
                    *frames = 1;
                }
                VideoPiece::Black { frames } => *frames = 1,
            }
            return one;
        }
        k -= p.frames();
    }
    panic!("frame {t} is outside the plan");
}

fn barcode(rgba: &[u8], w: u32, h: u32) -> Option<u32> {
    use vidcut_av::ffmpeg::{format::Pixel, frame};
    let mut f = frame::Video::new(Pixel::RGBA, w, h);
    let stride = f.stride(0);
    let row = w as usize * 4;
    for y in 0..h as usize {
        f.data_mut(0)[y * stride..y * stride + row].copy_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    vidcut_av::decode::read_barcode(&f).unwrap()
}

/// Mean and largest difference of two RGBA pictures' color channels.
fn diff(a: &[u8], b: &[u8]) -> (f64, u8) {
    let (mut sum, mut max, mut n) = (0u64, 0u8, 0u64);
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        for c in 0..3 {
            let d = p[c].abs_diff(q[c]);
            sum += u64::from(d);
            max = max.max(d);
            n += 1;
        }
    }
    (sum as f64 / n as f64, max)
}

/// [`diff`] where `b` is flat: each pixel like those two away each way.
fn flat_diff(a: &[u8], b: &[u8], w: usize) -> (f64, u8) {
    let h = b.len() / 4 / w;
    let px = |x: usize, y: usize| &b[(y * w + x) * 4..(y * w + x) * 4 + 3];
    let (mut sum, mut max, mut n) = (0u64, 0u8, 0u64);
    for y in 2..h.saturating_sub(2) {
        for x in 2..w.saturating_sub(2) {
            let here = px(x, y);
            if [px(x - 2, y), px(x + 2, y), px(x, y - 2), px(x, y + 2)]
                .iter()
                .any(|q| *q != here)
            {
                continue;
            }
            let i = (y * w + x) * 4;
            for c in 0..3 {
                let d = a[i + c].abs_diff(b[i + c]);
                sum += u64::from(d);
                max = max.max(d);
                n += 1;
            }
        }
    }
    (sum as f64 / n.max(1) as f64, max)
}

fn picture_at(player: &Player, t: i64) -> Arc<Picture> {
    player.seek(t);
    assert!(player.wait_ready(t, Duration::from_secs(20)), "frame {t}");
    let shown = player.present(Instant::now()).unwrap();
    assert!(shown.exact);
    shown.picture
}

#[test]
fn the_shader_draws_what_the_export_renders() {
    let Some(gpu) = Gpu::headless() else {
        eprintln!("skipping: no GPU adapter (not even a software one)");
        return;
    };
    let spec = |codec, container, w, h| FixtureSpec::new(codec, container, w, h, (30, 1), 30);
    let pieces = vec![
        // 0: 8-bit 4:2:0, planar.
        piece(spec(VideoCodec::H264, Container::Mp4, W, H), 3, 10),
        // 10: 10-bit, P010.
        piece(spec(VideoCodec::Hevc10, Container::Mkv, W, H), 4, 10),
        // 20, 30, 40: stored turned, shown upright.
        piece(
            spec(VideoCodec::H264, Container::Mp4, W, H).rotation(90),
            5,
            10,
        ),
        piece(
            spec(VideoCodec::H264, Container::Mp4, W, H).rotation(180),
            6,
            10,
        ),
        piece(
            spec(VideoCodec::H264, Container::Mp4, W, H).rotation(270),
            7,
            10,
        ),
        // 50: 4:3, pillarboxed.
        piece(spec(VideoCodec::H264, Container::Mp4, 240, 180), 8, 10),
        // 60: stripes in HD (BT.709), 70: in SD (BT.601).
        piece(
            spec(VideoCodec::H264, Container::Mp4, 1280, 720).picture(Content::Colors),
            0,
            10,
        ),
        piece(
            spec(VideoCodec::H264, Container::Mp4, 640, 480).picture(Content::Colors),
            0,
            10,
        ),
        // 80: PQ, 90: HLG (barcodes), 100: PQ stripes.
        piece(
            spec(VideoCodec::Hevc10, Container::Mkv, W, H).hdr(Hdr::Pq),
            9,
            10,
        ),
        piece(
            spec(VideoCodec::Hevc10, Container::Mkv, W, H).hdr(Hdr::Hlg),
            10,
            10,
        ),
        piece(
            spec(VideoCodec::Hevc10, Container::Mkv, W, H)
                .hdr(Hdr::Pq)
                .picture(Content::Colors),
            0,
            10,
        ),
        // 110: black.
        VideoPiece::Black { frames: 10 },
    ];
    let mut pieces = pieces;
    // 120: a still in its file's colors (RGB, coded BT.709 by the player).
    let still = fixture::ensure_images(&ImageSpec::still(123, W, H, "png")).unwrap();
    pieces.push(VideoPiece::Source {
        media: "s".into(),
        path: still[0].clone(),
        origin: 0.0,
        source_frame: 0,
        frames: 10,
        image: Some(Image::Still),
    });
    // 130: full-range 4:2:0 (a JPEG still).
    let jpeg = fixture::ensure_images(&ImageSpec::still(321, W, H, "jpg")).unwrap();
    pieces.push(VideoPiece::Source {
        media: "j".into(),
        path: jpeg[0].clone(),
        origin: 0.0,
        source_frame: 0,
        frames: 10,
        image: Some(Image::Still),
    });
    let p = plan(pieces);
    let player = Player::new(
        p.clone(),
        Config {
            audio_scrubbing: false,
            ..Config::default()
        },
        Box::new(ManualOutput::new()),
    )
    .unwrap();
    let mut screen = Screen::new();
    for t in (5..140).step_by(10) {
        let picture = picture_at(&player, t);
        // In a target of the timeline's aspect that shows it at its own
        // size (by its height): the export's conversion of the same
        // picture, to within rounding. That checks the colors; scaled, the
        // filters differ at edges.
        let upright = if picture.rotation() % 180 == 90 {
            picture.width()
        } else {
            picture.height()
        };
        let (w, h) = ((upright * W / H).div_ceil(2) * 2, upright);
        let theirs = picture.to_rgba(w, h).unwrap();
        assert_ne!(screen.draw(&gpu, Some(&picture), w, h), Drawn::Same);
        let ours = screen.read(&gpu).unwrap();
        assert_eq!((ours.width, ours.height), (w, h));
        let (mean, max) = diff(&ours.rgba, &theirs.rgba);
        let (flat, flat_max) = flat_diff(&ours.rgba, &theirs.rgba, w as usize);
        let rendered = vidcut_av::export::still::rgba(&piece_at(&p, t), p.fps, w, h).unwrap();
        let want = barcode(&rendered.rgba, w, h);
        eprintln!(
            "frame {t}: {:?} {:?} turned {} at {w}x{h}: {mean:.2} levels on average from the \
             export's, at most {max}; where flat {flat:.2}, at most {flat_max}",
            picture.layout(),
            picture.colors(),
            picture.rotation()
        );
        assert_eq!(barcode(&ours.rgba, w, h), want, "frame {t}'s barcode");
        // The colors: within rounding of the export's where the picture is
        // flat, no sample more than a level off (SDR exact on NVIDIA's,
        // Intel's and lavapipe, PQ and HLG a level at most). At edges the
        // filters differ: at its own size the export repeats each chroma
        // sample, the shader interpolates between them.
        assert!(
            flat < 1.0 && flat_max <= 1,
            "frame {t}: {flat:.2}, {flat_max}"
        );
        assert!(mean < 2.0, "frame {t}: {mean:.2} levels apart on average");
        // SDR stripes against the matrix's exact numbers.
        if (60..80).contains(&t) {
            let (kr, kb) = if t < 70 {
                (0.2126, 0.0722)
            } else {
                (0.299, 0.114)
            };
            let (x0, y0, pw, ph) = picture.placement(w, h);
            for (k, (yy, cb, cr)) in Content::COLORS.iter().enumerate() {
                let y = (f64::from(*yy) - 16.0) / 219.0;
                let (cb, cr) = (
                    (f64::from(*cb) - 128.0) / 224.0,
                    (f64::from(*cr) - 128.0) / 224.0,
                );
                let r = y + (2.0 - 2.0 * kr) * cr;
                let b = y + (2.0 - 2.0 * kb) * cb;
                let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
                let x = x0 + pw * (2 * k as u32 + 1) / 8;
                let i = ((y0 + ph / 2) * w + x) as usize * 4;
                for (c, want) in [r, g, b].into_iter().enumerate() {
                    let got = f64::from(ours.rgba[i + c]);
                    assert!(
                        (got - want * 255.0).abs() <= 1.0,
                        "frame {t}, stripe {k}, channel {c}: {got} for {:.1}",
                        want * 255.0
                    );
                }
            }
        }
        // The same picture again draws nothing.
        assert_eq!(screen.draw(&gpu, Some(&picture), w, h), Drawn::Same);
        // Scaled, as the viewer usually is: the barcode frame render shows
        // at that size.
        for (w, h) in [(640, 360), (W, H), (200, 112)] {
            assert_eq!(screen.draw(&gpu, Some(&picture), w, h), Drawn::Target);
            let ours = screen.read(&gpu).unwrap();
            let rendered = vidcut_av::export::still::rgba(&piece_at(&p, t), p.fps, w, h).unwrap();
            assert_eq!(
                barcode(&ours.rgba, w, h),
                barcode(&rendered.rgba, w, h),
                "frame {t} at {w}x{h}"
            );
            let (mean, _) = diff(&ours.rgba, &rendered.rgba);
            eprintln!("  at {w}x{h}: {mean:.2} levels on average");
            // Edges differ the most: HD stripes four times smaller, and
            // HDR stripes, whose edges the tone curve exaggerates.
            assert!(mean < 5.0, "frame {t} at {w}x{h}: {mean:.2} levels apart");
        }
    }
    // Nothing to show: black.
    screen.draw(&gpu, None, W, H);
    let black = screen.read(&gpu).unwrap();
    assert!(black.rgba.chunks(4).all(|p| p == [0, 0, 0, 255]));
}

/// NV12, as NVDEC's VP9 frames come down: needs the GPU's decoder.
#[test]
fn nv12_pictures_draw_as_the_export_renders_them() {
    let Some(gpu) = Gpu::headless() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    if vidcut_av::cuda::CudaDevice::open().is_err() {
        eprintln!("skipping: no CUDA device for NV12 pictures");
        return;
    }
    let p = plan(vec![piece(
        FixtureSpec::new(VideoCodec::Vp9, Container::Webm, W, H, (30, 1), 30),
        2,
        20,
    )]);
    let player = Player::new(
        p.clone(),
        Config {
            audio_scrubbing: false,
            hardware: true,
            ..Config::default()
        },
        Box::new(ManualOutput::new()),
    )
    .unwrap();
    let mut screen = Screen::new();
    for t in [0, 7, 19] {
        let picture = picture_at(&player, t);
        assert_eq!(picture.layout(), vidcut_av::player::Layout::Nv12);
        screen.draw(&gpu, Some(&picture), W, H);
        let ours = screen.read(&gpu).unwrap();
        let theirs = picture.to_rgba(W, H).unwrap();
        let (mean, _) = diff(&ours.rgba, &theirs.rgba);
        assert!(mean < 1.5, "frame {t}: {mean:.2}");
        assert_eq!(barcode(&ours.rgba, W, H), Some(2 + t as u32), "frame {t}");
    }
}

/// Pictures NVDEC decoded, kept in GPU memory and shown from there (memory
/// the device shares with CUDA, `gui::shared`): the same pixels as the
/// same picture copied down and uploaded, NV12 and P010, deinterlaced on
/// the GPU, at a size whose rows and chroma aren't round. Needs CUDA on the
/// adapter's GPU.
#[test]
fn pictures_in_gpu_memory_draw_as_copied_down() {
    let Some(gpu) = Gpu::headless() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    if !gpu.shares_with_cuda() {
        eprintln!("skipping: the adapter shares no memory with CUDA, or isn't CUDA's GPU");
        return;
    }
    let p = plan(vec![
        piece(
            FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 30),
            0,
            20,
        ),
        piece(
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 30),
            0,
            20,
        ),
        piece(
            FixtureSpec::new(VideoCodec::H264, Container::Mkv, W, H, (30, 1), 30)
                .interlaced(true)
                .no_audio(),
            0,
            20,
        ),
        piece(
            FixtureSpec::new(VideoCodec::H264, Container::Mp4, 330, 186, (30, 1), 30),
            0,
            20,
        ),
    ]);
    let player = Player::new(
        p.clone(),
        Config {
            audio_scrubbing: false,
            hardware: true,
            gpu_pictures: true,
            ..Config::default()
        },
        Box::new(ManualOutput::new()),
    )
    .unwrap();
    let (mut shown, mut copied) = (Screen::new(), Screen::new());
    for (t, layout) in [
        (0, Layout::Nv12),
        (19, Layout::Nv12),
        (20, Layout::P010),
        (33, Layout::P010),
        (40, Layout::Nv12),
        (51, Layout::Nv12),
        (60, Layout::Nv12),
        (79, Layout::Nv12),
    ] {
        let picture = picture_at(&player, t);
        assert_eq!(picture.memory(), Memory::Cuda, "frame {t}");
        assert_eq!(picture.layout(), layout, "frame {t}");
        shown.draw(&gpu, Some(&picture), W, H);
        assert!(shown.shared(), "frame {t} was copied down to be shown");
        let ours = shown.read(&gpu).unwrap();
        let down = Arc::new(picture.copied_down().unwrap());
        assert_eq!(down.memory(), Memory::Cpu);
        copied.draw(&gpu, Some(&down), W, H);
        let theirs = copied.read(&gpu).unwrap();
        let (mean, max) = diff(&ours.rgba, &theirs.rgba);
        assert_eq!((mean, max), (0.0, 0), "frame {t}");
        let rendered = vidcut_av::export::still::rgba(&piece_at(&p, t), p.fps, W, H).unwrap();
        assert_eq!(
            barcode(&ours.rgba, W, H),
            barcode(&rendered.rgba, W, H),
            "frame {t}"
        );
    }
}

/// Pictures VA-API decoded, kept in GPU memory (the decoder's surfaces)
/// and shown from there (imported as DMA-BUFs, taken over from VA-API):
/// the same pixels as the same picture copied down and uploaded, NV12 and
/// P010, at a size whose rows and chroma aren't round, odd sides too;
/// interlaced ones come down for the CPU's deinterlacer. Needs VA-API, and an adapter that
/// imports DMA-BUFs.
#[cfg(target_os = "linux")]
#[test]
fn vaapi_pictures_draw_as_copied_down() {
    use vidcut_av::hw::{GpuDecoder, Kind};
    let Some((gpu, adapter)) = Gpu::headless_with_info() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    if !gpu.imports_dma_bufs() {
        eprintln!("skipping: the adapter imports no DMA-BUFs");
        return;
    }
    // VA-API on the adapter's GPU, by its render node's PCI address, where
    // the adapter says its own.
    let pci = |node: &str| {
        let name = std::path::Path::new(node).file_name()?.to_owned();
        let device = std::path::Path::new("/sys/class/drm")
            .join(name)
            .join("device");
        let address = std::fs::read_link(device).ok()?;
        Some(address.file_name()?.to_string_lossy().to_lowercase())
    };
    let ours = adapter.device_pci_bus_id.to_lowercase();
    let Some(vaapi) = GpuDecoder::all().into_iter().find(|d| {
        d.kind() == Kind::Vaapi && (ours.is_empty() || pci(d.on()).as_deref() == Some(&*ours))
    }) else {
        eprintln!(
            "skipping: no VA-API on the adapter's GPU ({})",
            adapter.name
        );
        return;
    };
    let p = plan(vec![
        piece(
            FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 30),
            0,
            20,
        ),
        piece(
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 30),
            0,
            20,
        ),
        piece(
            FixtureSpec::new(VideoCodec::H264, Container::Mkv, W, H, (30, 1), 30)
                .interlaced(true)
                .no_audio(),
            0,
            20,
        ),
        // Odd sides, in colors: the surface rounded up, its chroma read
        // where the copy's is.
        piece(
            FixtureSpec::new(VideoCodec::Vp9, Container::Webm, 331, 187, (30, 1), 30)
                .picture(Content::Colors),
            0,
            20,
        ),
    ]);
    let player = Player::new(
        p.clone(),
        Config {
            audio_scrubbing: false,
            hardware: true,
            gpu: Some(vaapi),
            gpu_pictures: true,
            ..Config::default()
        },
        Box::new(ManualOutput::new()),
    )
    .unwrap();
    let (mut shown, mut copied) = (Screen::new(), Screen::new());
    for (t, memory) in [
        (0, Memory::Vaapi),
        (19, Memory::Vaapi),
        (20, Memory::Vaapi),
        (33, Memory::Vaapi),
        (45, Memory::Cpu),
        (60, Memory::Vaapi),
        (79, Memory::Vaapi),
    ] {
        let picture = picture_at(&player, t);
        assert_eq!(picture.memory(), memory, "frame {t}");
        shown.draw(&gpu, Some(&picture), W, H);
        if memory == Memory::Vaapi {
            assert!(shown.shared(), "frame {t} was copied down to be shown");
        }
        let ours = shown.read(&gpu).unwrap();
        let down = Arc::new(picture.copied_down().unwrap());
        copied.draw(&gpu, Some(&down), W, H);
        let theirs = copied.read(&gpu).unwrap();
        let (mean, max) = diff(&ours.rgba, &theirs.rgba);
        assert_eq!((mean, max), (0.0, 0), "frame {t}");
        let rendered = vidcut_av::export::still::rgba(&piece_at(&p, t), p.fps, W, H).unwrap();
        assert_eq!(
            barcode(&ours.rgba, W, H),
            barcode(&rendered.rgba, W, H),
            "frame {t}"
        );
    }
}

/// Pictures Direct3D 11 decoded, kept in GPU memory (copied out of the
/// decoder's texture array into textures of their own) and shown from there
/// (each texture imported by wgpu's Vulkan device on the same adapter, its
/// planes drawn from views of it): the same pixels as the same picture copied
/// down and uploaded, NV12 and P010, at a size whose rows and chroma aren't
/// round, odd sides too. Needs Direct3D 11 decoding, and a Vulkan adapter on the same GPU
/// that imports its textures (Windows).
#[cfg(windows)]
#[test]
fn d3d11_pictures_draw_as_copied_down() {
    use vidcut_av::hw::{GpuDecoder, Kind};
    let pairs: Vec<(GpuDecoder, Gpu)> = GpuDecoder::all()
        .into_iter()
        .filter(|d| d.kind() == Kind::D3d11va)
        .filter_map(|d| {
            let luid = d.luid()?;
            let gpu = Gpu::headless_all()
                .into_iter()
                .find(|g| g.imports_d3d11() && g.luid() == Some(luid))?;
            Some((d, gpu))
        })
        .collect();
    if pairs.is_empty() {
        eprintln!("skipping: no Direct3D 11 decoder on an adapter wgpu imports from");
        return;
    }
    let p = plan(vec![
        piece(
            FixtureSpec::new(VideoCodec::Vp9, Container::Webm, W, H, (30, 1), 30),
            0,
            20,
        ),
        piece(
            FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 30),
            0,
            20,
        ),
        // Odd sides, in colors: the texture rounded up, its chroma read
        // where the copy's is.
        piece(
            FixtureSpec::new(VideoCodec::Vp9, Container::Webm, 331, 187, (30, 1), 30)
                .picture(Content::Colors),
            0,
            20,
        ),
    ]);
    for (decoder, gpu) in pairs {
        let name = format!("d3d11va {}", decoder.on());
        let player = Player::new(
            p.clone(),
            Config {
                audio_scrubbing: false,
                hardware: true,
                gpu: Some(decoder),
                gpu_pictures: true,
                ..Config::default()
            },
            Box::new(ManualOutput::new()),
        )
        .unwrap();
        let (mut shown, mut copied) = (Screen::new(), Screen::new());
        for (t, layout) in [
            (0, Layout::Nv12),
            (19, Layout::Nv12),
            (20, Layout::P010),
            (39, Layout::P010),
            (40, Layout::Nv12),
            (59, Layout::Nv12),
        ] {
            let picture = picture_at(&player, t);
            assert_eq!(picture.memory(), Memory::D3d11, "{name}: frame {t}");
            assert_eq!(picture.layout(), layout, "{name}: frame {t}");
            shown.draw(&gpu, Some(&picture), W, H);
            assert!(
                shown.shared(),
                "{name}: frame {t} was copied down to be shown"
            );
            let ours = shown.read(&gpu).unwrap();
            let down = Arc::new(picture.copied_down().unwrap());
            copied.draw(&gpu, Some(&down), W, H);
            let theirs = copied.read(&gpu).unwrap();
            let (mean, max) = diff(&ours.rgba, &theirs.rgba);
            assert_eq!((mean, max), (0.0, 0), "{name}: frame {t}");
            let rendered = vidcut_av::export::still::rgba(&piece_at(&p, t), p.fps, W, H).unwrap();
            assert_eq!(
                barcode(&ours.rgba, W, H),
                barcode(&rendered.rgba, W, H),
                "{name}: frame {t}"
            );
        }
    }
}
