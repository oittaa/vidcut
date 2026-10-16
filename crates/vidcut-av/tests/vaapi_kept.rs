//! VA-API's kept pictures: copied by the driver's `vaCopy` into surfaces of
//! their own where it has one (Intel's), else the decoder's surfaces, from
//! the pool its decoders share (`hw::vaapi::Keeper`). Either way a kept
//! picture is the frame as decoded, every sample: copied down, it is what
//! the same decoder's frame gives copied down without being kept. Runs on
//! the GPU decoder in use when it is VA-API (`VIDCUT_HWDEC=vaapi:...` picks
//! one) and skips otherwise; the surface of its own is checked where the
//! GPU in use is Intel's. A prepared picture, held whatever the budget,
//! holds no pool its decoders share: it is a copy of its own, or in memory.

mod player_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::ffmpeg::ffi;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, ManualOutput, Picture, Player};

/// The GPU decoder in use, when it is VA-API.
fn vaapi() -> Option<GpuDecoder> {
    let gpu = GpuDecoder::best().filter(|g| g.kind() == Kind::Vaapi);
    if gpu.is_none() {
        eprintln!("skipping: the GPU decoder in use isn't VA-API");
    }
    gpu
}

/// The player's picture of frame 5 of a 10-bit HEVC clip, decoded on the
/// GPU, kept there for `kept` decoders and come down for the rest.
fn picture(kept: Vec<Kind>) -> Arc<Picture> {
    let clip_src = src(
        "m",
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 10).no_audio(),
    );
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: kept,
        ..Config::default()
    };
    let plan = plan(&[clip(&clip_src, 0, 10)], 0);
    let player = Player::new(plan, config, Box::new(ManualOutput::new())).unwrap();
    player.seek(5);
    assert!(player.wait_ready(5, Duration::from_secs(20)));
    player.present(Instant::now()).unwrap().picture
}

/// The samples of `p`'s frame in memory, rows packed.
fn samples(p: &Picture) -> Vec<u8> {
    let f = p.frame();
    // SAFETY: a frame in memory, its planes copied out at its own size.
    unsafe {
        let raw = &*f.as_ptr();
        let format = std::mem::transmute::<i32, ffi::AVPixelFormat>(raw.format);
        let size = ffi::av_image_get_buffer_size(format, raw.width, raw.height, 1);
        let mut out = vec![0u8; size as usize];
        let copied = ffi::av_image_copy_to_buffer(
            out.as_mut_ptr(),
            size,
            raw.data.as_ptr() as *const *const u8,
            raw.linesize.as_ptr(),
            format,
            raw.width,
            raw.height,
            1,
        );
        assert_eq!(copied, size);
        out
    }
}

#[test]
fn a_picture_kept_on_va_api_is_the_frame_as_decoded() {
    if vaapi().is_none() {
        return;
    }
    let kept = picture(vec![Kind::Vaapi]);
    assert_eq!(kept.gpu(), Some(Kind::Vaapi));
    let down = picture(Vec::new());
    assert_eq!(down.gpu(), None);
    assert!(
        samples(&kept.copied_down().unwrap()) == samples(&down),
        "the kept picture's samples aren't the frame's"
    );
}

#[test]
fn a_picture_intels_driver_keeps_is_in_a_surface_of_its_own() {
    let Some(gpu) = vaapi() else {
        return;
    };
    if gpu.gpu().is_none_or(|g| g.vendor != "Intel") {
        eprintln!("skipping: the GPU in use isn't Intel's");
        return;
    }
    let kept = picture(vec![Kind::Vaapi]);
    // SAFETY: a VA-API frame's frames context.
    let pool = unsafe {
        let frames = (*kept.frame().as_ptr()).hw_frames_ctx;
        (*((*frames).data as *const ffi::AVHWFramesContext)).initial_pool_size
    };
    // Frames of its one surface; the decoders' pool grows as they need.
    assert_eq!(pool, 1);
}

#[test]
fn a_prepared_picture_holds_no_pool_the_decoders_share() {
    if vaapi().is_none() {
        return;
    }
    let clip_src = src(
        "m",
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 10).no_audio(),
    );
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: vec![Kind::Vaapi],
        ..Config::default()
    };
    let plan = plan(&[clip(&clip_src, 0, 10)], 0);
    let player = Player::new(plan.clone(), config, Box::new(ManualOutput::new())).unwrap();
    player.prepare(Some((plan.clone(), 7)));
    let started = Instant::now();
    while !player.status().prepared {
        assert!(started.elapsed() < Duration::from_secs(20), "not prepared");
        std::thread::sleep(Duration::from_millis(10));
    }
    player.set_plan_at(plan, 7).unwrap();
    let held = player.present(Instant::now()).unwrap().picture;
    // SAFETY: a VA-API frame's frames context, where it is one.
    let own = held.gpu().is_none()
        || unsafe {
            let frames = (*held.frame().as_ptr()).hw_frames_ctx;
            (*((*frames).data as *const ffi::AVHWFramesContext)).initial_pool_size == 1
        };
    assert!(own, "the prepared picture is a shared pool's surface");
}
