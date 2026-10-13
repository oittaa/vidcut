//! The player decoding on each GPU decoder there is (NVDEC; VA-API on each
//! render node; Direct3D 11 on each adapter) every stream it decodes
//! (H.264, HEVC, AV1, VP9): the same pictures as decoding on the CPU, frame
//! for frame and within a level or two in their pixels, copied down to
//! memory in NV12 or P010; streams the GPU can't decode, stills and black
//! on the CPU; and switching decoders while it plays. Kept in GPU memory
//! for a display that shows them from there, NVDEC's are the same
//! pictures, and a seek keeps what it decodes on its way for the steps
//! back. Needs a GPU: `--features vidcut-av/gpu-tests`.
#![cfg(feature = "gpu-tests")]

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, Decoder, Layout, ManualOutput, Memory, Picture, Player};

fn player(plan: vidcut_core::render::RenderPlan, hardware: bool) -> Player {
    let config = Config {
        audio_scrubbing: false,
        hardware,
        ..Config::default()
    };
    Player::new(plan, config, Box::new(ManualOutput::new())).unwrap()
}

/// A player decoding on `gpu`.
fn player_on(plan: vidcut_core::render::RenderPlan, gpu: GpuDecoder) -> Player {
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu: Some(gpu),
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
fn gpu_pictures_are_the_cpus() {
    let decoders = GpuDecoder::all();
    if decoders.is_empty() {
        eprintln!("skipping: no GPU decoder");
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
    // Interlaced: deinterlaced as on the CPU, on the GPU for NVDEC's
    // frames (`bwdif_cuda`), the others' copied down for it.
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
    // ProRes: no GPU decodes it, so the CPU.
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
    for gpu_decoder in decoders {
        let kind = gpu_decoder.kind();
        let name = format!("{} {}", kind.name(), gpu_decoder.on());
        let h264 = gpu_decoder.decodes(vidcut_av::ffmpeg::codec::Id::H264);
        let gpu = player_on(p.clone(), gpu_decoder);
        for &t in &ts_checked {
            let g = picture_at(&gpu, t);
            let c = picture_at(&cpu, t);
            // Upright, as the export shows it.
            let upright = |p: &Picture| {
                let img = p.to_rgba(W, H).unwrap();
                code_rgba(&img.rgba, W, H)
            };
            assert_eq!(upright(&g), expect[t as usize], "frame {t} on {name}");
            assert_eq!(
                upright(&g),
                upright(&c),
                "frame {t}: {name} against the CPU"
            );
            assert_eq!(g.memory(), Memory::Cpu, "copied down");
            assert_eq!(g.colors(), c.colors(), "frame {t}'s colors");
            assert_eq!(g.rotation(), c.rotation());
            let diff = mean_diff(&g, &c);
            assert!(diff < 2.0, "frame {t}: {diff:.2} levels from the CPU's");
        }
        // Which decoded what: H.264 every GPU that decodes it right (not
        // AMD's Direct3D 11), interlaced too; ProRes none; the rest NVDEC,
        // and the other GPUs where they decode them.
        let on_gpu = Some(Decoder::Gpu(kind));
        let decoder = |t: i64| {
            let _ = picture_at(&gpu, t);
            gpu.status().decoder
        };
        let (h264_on, h264_layout) = if h264 {
            (on_gpu, Layout::Nv12)
        } else {
            (Some(Decoder::Cpu), Layout::Yuv420p)
        };
        assert_eq!(decoder(5), h264_on, "{name}: H.264");
        assert_eq!(picture_at(&gpu, 5).layout(), h264_layout);
        assert_eq!(decoder(starts[6] + 3), h264_on, "{name}: interlaced H.264");
        for (t, what) in [
            (35, "HEVC 10-bit"),
            (starts[3] + 3, "VP9"),
            (starts[4] + 3, "AV1"),
            (starts[5] + 3, "HEVC in MPEG-TS"),
            (starts[7] + 3, "HEVC turned"),
        ] {
            let d = decoder(t);
            eprintln!("{name}: {what} decodes on {d:?}");
            if kind == Kind::Nvdec {
                assert_eq!(d, on_gpu, "{name}: {what}");
            } else {
                assert!(d == on_gpu || d == Some(Decoder::Cpu), "{name}: {what}");
            }
        }
        assert_eq!(picture_at(&gpu, 35).layout(), Layout::P010);
        assert_eq!(picture_at(&gpu, starts[3] + 3).layout(), Layout::Nv12);
        assert_eq!(decoder(starts[8] + 3), Some(Decoder::Cpu), "{name}: ProRes");

        // Steps both ways through HEVC, black and VP9 on the GPU.
        gpu.seek(30);
        assert_eq!(shown_at(&gpu, 30), expect[30]);
        for t in 31..85 {
            gpu.step(1);
            assert_eq!(shown_at(&gpu, t), expect[t as usize], "{name}: step to {t}");
        }
        for t in (30..84).rev() {
            gpu.step(-1);
            assert_eq!(shown_at(&gpu, t), expect[t as usize], "{name}: back to {t}");
        }

        // Switching decoders shows the new one's pictures from the one
        // shown.
        gpu.seek(5);
        assert_eq!(shown_at(&gpu, 5), expect[5]);
        // A display presenting: what it shows says what decoded it.
        let shows = |player: &Player| {
            player.present(Instant::now());
            player.status().decoder
        };
        gpu.set_hardware(false);
        let started = Instant::now();
        while shows(&gpu) != Some(Decoder::Cpu) {
            assert!(started.elapsed() < Duration::from_secs(10), "still {name}");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(shown_at(&gpu, 5), expect[5]);
        gpu.set_hardware(true);
        let started = Instant::now();
        while shows(&gpu) != h264_on {
            assert!(started.elapsed() < Duration::from_secs(10), "still the CPU");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(shown_at(&gpu, 5), expect[5]);
        assert_eq!(gpu.status().error, None);
    }
    assert_eq!(cpu.status().decoder, Some(Decoder::Cpu));
}

/// The GPU decoders there are, with the memory their pictures stay in for a
/// display that shows them from there: NVDEC's, and VA-API's and Direct3D
/// 11's (copied out of the decoder's surfaces).
fn keeping_decoders() -> Vec<(GpuDecoder, Memory)> {
    GpuDecoder::all()
        .into_iter()
        .map(|d| {
            let memory = match d.kind() {
                Kind::Nvdec => Memory::Cuda,
                Kind::Vaapi => Memory::Vaapi,
                Kind::D3d11va => Memory::D3d11,
            };
            (d, memory)
        })
        .collect()
}

/// A player on `gpu` keeping its pictures in GPU memory.
fn keeping(plan: vidcut_core::render::RenderPlan, gpu: GpuDecoder) -> Player {
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu: Some(gpu),
        gpu_pictures: true,
        ..Config::default()
    };
    Player::new(plan, config, Box::new(ManualOutput::new())).unwrap()
}

#[test]
fn pictures_kept_in_gpu_memory_are_the_ones_copied_down() {
    let decoders = keeping_decoders();
    if decoders.is_empty() {
        eprintln!("skipping: no GPU decoder");
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
    let nv12 = (W * H * 3 / 2) as usize;
    for (gpu, on_gpu) in decoders {
        let name = format!("{} {}", gpu.kind().name(), gpu.on());
        let kept = keeping(p.clone(), gpu.clone());
        let down = player_on(p.clone(), gpu.clone());
        // H.264 where the GPU decodes it right (`GpuDecoder::decodes`).
        let h264 = if gpu.decodes(vidcut_av::ffmpeg::codec::Id::H264) {
            on_gpu
        } else {
            Memory::Cpu
        };
        // Interlaced: deinterlaced on the GPU for NVDEC's, copied down for
        // `bwdif` for the others'.
        let interlaced = if gpu.kind() == Kind::Nvdec {
            on_gpu
        } else {
            Memory::Cpu
        };
        for (t, memory, bytes) in [
            (3, h264, nv12),
            (14, on_gpu, 2 * nv12),
            (25, interlaced, nv12),
            (36, on_gpu, 2 * nv12),
            (47, Memory::Cpu, 2 * nv12),
        ] {
            let k = picture_at(&kept, t);
            let d = picture_at(&down, t);
            assert_eq!(k.memory(), memory, "{name}: frame {t}");
            assert_eq!(k.layout(), d.layout(), "{name}: frame {t}");
            if memory != Memory::Cpu {
                let layout = if bytes == nv12 {
                    Layout::Nv12
                } else {
                    Layout::P010
                };
                assert_eq!(k.layout(), layout, "{name}: frame {t}");
            }
            // The budget counts what the picture takes where it is.
            assert_eq!(k.bytes(), bytes, "{name}: frame {t}");
            assert_eq!((k.colors(), k.rotation()), (d.colors(), d.rotation()));
            assert_eq!(
                mean_diff(&k, &d),
                0.0,
                "{name}: frame {t}: kept against copied down"
            );
            let img = k.to_rgba(W, H).unwrap();
            assert_eq!(
                code_rgba(&img.rgba, W, H),
                rendered(&p, t),
                "{name}: frame {t}"
            );
        }
        assert_eq!(kept.status().error, None, "{name}");
    }
}

#[test]
fn a_seek_in_video_memory_keeps_its_way_for_the_steps_back() {
    let decoders = keeping_decoders();
    if decoders.is_empty() {
        eprintln!("skipping: no GPU decoder");
        return;
    }
    // Keyframes 120 frames apart, with B-frames.
    let long = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 300)
            .gop(120)
            .bframes(2),
    );
    let p = plan(&[clip(&long, 0, 300)], 0);
    // Those that decode H.264 (not AMD's Direct3D 11).
    let decoders = decoders
        .into_iter()
        .filter(|(gpu, _)| gpu.decodes(vidcut_av::ffmpeg::codec::Id::H264));
    for (gpu, memory) in decoders {
        let name = format!("{} {}", gpu.kind().name(), gpu.on());
        let player = keeping(p.clone(), gpu);
        // In video memory: read once copied down.
        let barcode = |p: &Picture| code(&p.copied_down().unwrap());
        // The first picture says where pictures go: video memory.
        assert_eq!(picture_at(&player, 10).memory(), memory, "{name}");
        // Frame 200 is 80 past its keyframe (120): the frames decoded on the
        // way are kept, so steps back right after the seek show at once.
        assert_eq!(barcode(&picture_at(&player, 200)), Some(200), "{name}");
        for t in (150..200).rev() {
            player.step(-1);
            assert!(
                player.status().ready,
                "{name}: step back to {t}: kept from the seek"
            );
            let shown = player.present(Instant::now()).unwrap();
            assert_eq!((shown.frame, shown.exact), (t, true), "{name}");
            assert_eq!(barcode(&shown.picture), Some(t as u32), "{name}");
        }
    }
}

/// Seeks far into a long open GOP (keyframes 120 frames apart, B-frames
/// referring to the GOP before), and steps on and back from there: every
/// frame its own, on each GPU decoder. AMD's Direct3D 11 decoder showed one
/// picture over and over doing so with H.264, which the CPU decodes there
/// instead (`GpuDecoder::decodes`).
#[test]
fn seeks_into_long_open_gops_show_their_frames() {
    let decoders = GpuDecoder::all();
    if decoders.is_empty() {
        eprintln!("skipping: no GPU decoder");
        return;
    }
    let long = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 300)
            .gop(120)
            .bframes(2),
    );
    let p = plan(&[clip(&long, 0, 300)], 0);
    for gpu in decoders {
        let name = format!("{} {}", gpu.kind().name(), gpu.on());
        let player = player_on(p.clone(), gpu);
        for t in [200, 150, 260] {
            player.seek(t);
            assert_eq!(shown_at(&player, t), Some(t as u32), "{name}: seek to {t}");
        }
        player.seek(200);
        assert_eq!(shown_at(&player, 200), Some(200), "{name}");
        for t in 201..210 {
            player.step(1);
            assert_eq!(shown_at(&player, t), Some(t as u32), "{name}: step to {t}");
        }
        for t in (190..209).rev() {
            player.step(-1);
            assert_eq!(shown_at(&player, t), Some(t as u32), "{name}: back to {t}");
        }
    }
}
