//! Playing backward decodes each frame about once: the windows behind the
//! playhead are kept whole (player.md §3). A test binary of its own, as it
//! counts every frame the process decodes.

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player};

#[test]
fn playing_backward_decodes_each_frame_about_once() {
    // Keyframes 120 frames apart, decoded on the CPU into memory.
    let long = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 300).gop(120),
    );
    let out = ManualOutput::new();
    let player = Player::new(
        plan(&[clip(&long, 0, 300)], 0),
        Config::default(),
        Box::new(out.clone()),
    )
    .unwrap();
    player.seek(299);
    assert!(player.wait_ready(299, Duration::from_secs(10)));
    let before = vidcut_av::decode::frames_decoded();
    player.play(-1.0);
    let started = Instant::now();
    while player.status().playing {
        assert!(started.elapsed() < Duration::from_secs(60), "too slow");
        pull_full(&out, 480);
        let t = player.status().position;
        assert!(player.wait_ready(t, Duration::from_secs(10)), "frame {t}");
    }
    let decoded = vidcut_av::decode::frames_decoded() - before;
    // 300 frames, and a GOP more at most where windows start.
    assert!(
        decoded <= 420,
        "{decoded} frames decoded to play 300 backward"
    );
}
