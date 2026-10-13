//! The player decoding on the GPU (NVDEC) every stream it decodes (H.264,
//! HEVC, AV1, VP9): the same pictures as decoding on the CPU, frame for
//! frame and within a level or two in their pixels, copied down to memory
//! in NV12 or P010; streams NVDEC can't decode, stills and black on the
//! CPU; and switching decoders while it plays. Kept in GPU memory for a
//! display that shows them from there, they are the same pictures. Needs
//! an NVIDIA GPU: `--features vidcut-av/gpu-tests`.
#![cfg(feature = "gpu-tests")]

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, Decoder, Layout, ManualOutput, Memory, Picture, Player};

fn player(plan: vidcut_core::render::RenderPlan, hardware: bool) -> Player {
    let config = Config {
        audio_scrubbing: false,
        hardware,
        ..Config::default()
    };
    Player::new(plan, config, Box::new(ManualOutput::new())).unwrap()
}

fn picture_at(player: &Player, t: i64) -> std::sync::Arc<Picture> {
    player.seek(t);
    assert!(
        player.wait_ready(t, Duration::from_secs(10)),
        "frame {t}: {:?}",
        player.status()
    );
    player.present(Instant::now()).unwrap().picture
}

/// Mean difference of two pictures as the export converts them.
fn mean_diff(a: &Picture, b: &Picture) -> f64 {
    let (x, y) = (a.to_rgba(W, H).unwrap(), b.to_rgba(W, H).unwrap());
    let sum: u64 = x
        .rgba
        .iter()
        .zip(&y.rgba)
        .map(|(p, q)| u64::from(p.abs_diff(*q)))
        .sum();
    sum as f64 / x.rgba.len() as f64
}

#[test]
fn nvdec_pictures_are_the_cpus() {
    if vidcut_av::cuda::CudaDevice::open().is_err() {
        eprintln!("skipping: no CUDA device");
        return;
    }
    let h264 = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 90),
    );
    let hevc = src(
        "m2",
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 90),
    );
    let vp9 = src(
        "m3",
        FixtureSpec::new(VideoCodec::Vp9, Container::Webm, W, H, (30, 1), 60),
    );
    let av1 = src(
        "m4",
        FixtureSpec::new(VideoCodec::Av1, Container::Mkv, W, H, (30, 1), 60),
    );
    let ts = src(
        "m5",
        FixtureSpec::new(VideoCodec::Hevc10, Container::MpegTs, W, H, (30, 1), 60)
            .start_offset_ms(1400),
    );
    // Interlaced: deinterlaced on the GPU (`bwdif_cuda`) as on the CPU.
    let interlaced = src(
        "m6",
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, W, H, (30, 1), 60)
            .interlaced(true)
            .no_audio(),
    );
    let turned = src(
        "m7",
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mp4, W, H, (30, 1), 30).rotation(90),
    );
    // ProRes: no NVDEC decoder, so the CPU.
    let prores = src(
        "m8",
        FixtureSpec::new(VideoCodec::ProRes422Hq, Container::Mov, W, H, (30, 1), 30),
    );
    let p = plan(
        &[
            clip(&h264, 0, 30),
            clip(&hevc, 10, 30),
            gap(5),
            clip(&vp9, 5, 20),
            clip(&av1, 0, 20),
            clip(&ts, 10, 20),
            clip(&interlaced, 10, 20),
            clip(&turned, 0, 10),
            clip(&prores, 0, 10),
            clip(&h264, 60, 30),
        ],
        0,
    );
    let frames = p.frames;
    let expect: Vec<Option<u32>> = (0..frames).map(|t| rendered(&p, t)).collect();
    let gpu = player(p.clone(), true);
    let cpu = player(p.clone(), false);
    // The first frame of each piece, frames inside and the last of each.
    let mut starts = vec![0];
    for v in &p.video {
        starts.push(starts.last().unwrap() + v.frames());
    }
    let mut ts_checked = Vec::new();
    for w in starts.windows(2) {
        ts_checked.extend([w[0], (w[0] + w[1]) / 2, w[1] - 1]);
    }
    for &t in &ts_checked {
        let g = picture_at(&gpu, t);
        let c = picture_at(&cpu, t);
        // Upright, as the export shows it.
        let upright = |p: &Picture| {
            let img = p.to_rgba(W, H).unwrap();
            code_rgba(&img.rgba, W, H)
        };
        assert_eq!(upright(&g), expect[t as usize], "frame {t} on NVDEC");
        assert_eq!(upright(&g), upright(&c), "frame {t}: NVDEC against the CPU");
        assert_eq!(g.memory(), Memory::Cpu, "copied down");
        assert_eq!(g.colors(), c.colors(), "frame {t}'s colors");
        assert_eq!(g.rotation(), c.rotation());
        let diff = mean_diff(&g, &c);
        assert!(diff < 2.0, "frame {t}: {diff:.2} levels from the CPU's");
    }
    // Which decoded what.
    let decoder = |t: i64| {
        let _ = picture_at(&gpu, t);
        gpu.status().decoder
    };
    assert_eq!(decoder(5), Some(Decoder::Nvdec), "H.264");
    assert_eq!(picture_at(&gpu, 5).layout(), Layout::Nv12);
    assert_eq!(
        decoder(starts[6] + 3),
        Some(Decoder::Nvdec),
        "interlaced H.264"
    );
    assert_eq!(decoder(35), Some(Decoder::Nvdec), "HEVC 10-bit");
    assert_eq!(picture_at(&gpu, 35).layout(), Layout::P010);
    assert_eq!(decoder(starts[3] + 3), Some(Decoder::Nvdec), "VP9");
    assert_eq!(picture_at(&gpu, starts[3] + 3).layout(), Layout::Nv12);
    assert_eq!(decoder(starts[4] + 3), Some(Decoder::Nvdec), "AV1");
    assert_eq!(
        decoder(starts[5] + 3),
        Some(Decoder::Nvdec),
        "HEVC in MPEG-TS"
    );
    assert_eq!(decoder(starts[7] + 3), Some(Decoder::Nvdec), "HEVC turned");
    assert_eq!(decoder(starts[8] + 3), Some(Decoder::Cpu), "ProRes");
    assert_eq!(cpu.status().decoder, Some(Decoder::Cpu));

    // Steps both ways through HEVC, black and VP9 on the GPU.
    gpu.seek(30);
    assert_eq!(shown_at(&gpu, 30), expect[30]);
    for t in 31..85 {
        gpu.step(1);
        assert_eq!(shown_at(&gpu, t), expect[t as usize], "step to {t}");
    }
    for t in (30..84).rev() {
        gpu.step(-1);
        assert_eq!(shown_at(&gpu, t), expect[t as usize], "step back to {t}");
    }

    // Switching decoders shows the new one's pictures from the one shown.
    gpu.seek(40);
    assert_eq!(shown_at(&gpu, 40), expect[40]);
    // A display presenting: what it shows says what decoded it.
    let shows = |player: &Player| {
        player.present(Instant::now());
        player.status().decoder
    };
    gpu.set_hardware(false);
    let started = Instant::now();
    while shows(&gpu) != Some(Decoder::Cpu) {
        assert!(started.elapsed() < Duration::from_secs(10), "still NVDEC");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(shown_at(&gpu, 40), expect[40]);
    gpu.set_hardware(true);
    let started = Instant::now();
    while shows(&gpu) != Some(Decoder::Nvdec) {
        assert!(started.elapsed() < Duration::from_secs(10), "still the CPU");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(shown_at(&gpu, 40), expect[40]);
    assert_eq!(gpu.status().error, None);
}

#[test]
fn pictures_kept_in_gpu_memory_are_the_ones_copied_down() {
    if vidcut_av::cuda::CudaDevice::open().is_err() {
        eprintln!("skipping: no CUDA device");
        return;
    }
    let h264 = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 30),
    );
    let hevc = src(
        "m2",
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 30),
    );
    let interlaced = src(
        "m6",
        FixtureSpec::new(VideoCodec::H264, Container::Mkv, W, H, (30, 1), 60)
            .interlaced(true)
            .no_audio(),
    );
    let turned = src(
        "m7",
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mp4, W, H, (30, 1), 30).rotation(90),
    );
    let prores = src(
        "m8",
        FixtureSpec::new(VideoCodec::ProRes422Hq, Container::Mov, W, H, (30, 1), 30),
    );
    let p = plan(
        &[
            clip(&h264, 0, 10),
            clip(&hevc, 10, 10),
            clip(&interlaced, 10, 10),
            clip(&turned, 0, 10),
            clip(&prores, 0, 10),
        ],
        0,
    );
    let kept = Player::new(
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
    let down = player(p.clone(), true);
    let nv12 = (W * H * 3 / 2) as usize;
    for (t, memory, layout, bytes) in [
        (3, Memory::Cuda, Layout::Nv12, nv12),
        (14, Memory::Cuda, Layout::P010, 2 * nv12),
        (25, Memory::Cuda, Layout::Nv12, nv12),
        (36, Memory::Cuda, Layout::P010, 2 * nv12),
        (47, Memory::Cpu, Layout::P010, 2 * nv12),
    ] {
        let k = picture_at(&kept, t);
        let d = picture_at(&down, t);
        assert_eq!((k.memory(), k.layout()), (memory, layout), "frame {t}");
        // The budget counts what the picture takes where it is.
        assert_eq!(k.bytes(), bytes, "frame {t}");
        assert_eq!((k.colors(), k.rotation()), (d.colors(), d.rotation()));
        assert_eq!(
            mean_diff(&k, &d),
            0.0,
            "frame {t}: kept against copied down"
        );
        let img = k.to_rgba(W, H).unwrap();
        assert_eq!(code_rgba(&img.rgba, W, H), rendered(&p, t), "frame {t}");
    }
    assert_eq!(kept.status().error, None);
}
