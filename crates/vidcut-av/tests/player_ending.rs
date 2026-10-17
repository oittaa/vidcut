//! A player dropped on NVDEC while the next file opens ahead: once the
//! threads it left end (`vidcut_av::ending::join`), no reader is open and,
//! on Linux, no thread named after the player's remains, so no NVDEC
//! decoder or its driver thread is left to close while the process exits
//! (gotchas.md). Runs on the GPU decoder in use when it is NVDEC and skips
//! otherwise.

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::decode::open_readers;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::hw::{GpuDecoder, Kind};
use vidcut_av::player::{Config, ManualOutput, Player};

/// The process's threads named after the player's (`vidcut-play-…`) or
/// its opener's (`vidcut-open`): theirs, and the driver's they started.
#[cfg(target_os = "linux")]
fn players_threads() -> Vec<String> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
        .map(|name| name.trim_end().to_string())
        .filter(|name| name.starts_with("vidcut-play") || name.starts_with("vidcut-open"))
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn players_threads() -> Vec<String> {
    Vec::new()
}

#[test]
fn a_player_dropped_on_nvdec_leaves_no_decoder_once_its_threads_end() {
    // Opened here first, so the CUDA context's own thread is named after
    // this one.
    if GpuDecoder::best()
        .filter(|g| g.kind() == Kind::Nvdec)
        .is_none()
    {
        eprintln!("skipping: the GPU decoder in use isn't NVDEC");
        return;
    }
    let spec = |frames| FixtureSpec::new(VideoCodec::H264, Container::Mkv, W, H, (30, 1), frames);
    let (a, b) = (src("a", spec(60)), src("b", spec(90)));
    let config = Config {
        audio_scrubbing: false,
        hardware: true,
        gpu_pictures: vec![Kind::Nvdec],
        ..Config::default()
    };
    let plan = plan(&[clip(&a, 0, 60), clip(&b, 0, 90)], 0);
    let before = open_readers();
    let player = Player::new(plan, config, Box::new(ManualOutput::new())).unwrap();
    player.seek(50);
    assert!(player.wait_ready(50, Duration::from_secs(20)));
    // Decoding ahead, the reader sends the next file to the opener.
    let until = Instant::now() + Duration::from_secs(20);
    while open_readers() < before + 2 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(open_readers(), before + 2, "the next file opens ahead");
    drop(player);
    vidcut_av::ending::join();
    assert_eq!(open_readers(), before);
    assert_eq!(players_threads(), Vec::<String>::new());
}
