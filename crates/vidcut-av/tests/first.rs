//! The player comes first (player.md §1): a job's decoding waits while a
//! player plays, and goes on once it stops. A test binary of its own, so
//! no other test's player holds the jobs back.

mod player_support;

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use player_support::*;
use vidcut_av::decode::Input;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player};

/// Runs `job` on a thread that yields to players; its result comes back.
fn yielding(job: impl FnOnce() -> bool + Send + 'static) -> Receiver<bool> {
    let (done, result) = channel();
    std::thread::spawn(move || {
        vidcut_av::first::yield_to_players();
        let _ = done.send(job());
    });
    result
}

#[test]
fn jobs_decode_once_the_player_stops() {
    let a = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 90),
    );
    let out = ManualOutput::new();
    let player = Player::new(
        plan(&[clip(&a, 0, 90)], 0),
        Config::default(),
        Box::new(out.clone()),
    )
    .unwrap();
    // The clock stands until a test pulls the sound, so it plays on. A
    // frame decoded ahead means the decode thread has seen it play.
    player.play(1.0);
    assert!(player.wait_ready(6, Duration::from_secs(10)));

    let input = || Input::File(a.path.clone());
    let (strip, stats) = (input(), input());
    let jobs = [
        (
            "filmstrip",
            yielding(move || vidcut_av::thumbs::filmstrip(&strip, 4, 64, 36, None).is_ok()),
        ),
        (
            "picture stats",
            yielding(move || vidcut_av::analysis::picture_stats(&stats, None, None).is_ok()),
        ),
    ];
    for (job, result) in &jobs {
        assert!(
            result.recv_timeout(Duration::from_millis(500)).is_err(),
            "the {job} decoded while the player played"
        );
    }
    player.pause();
    for (job, result) in &jobs {
        assert_eq!(
            result.recv_timeout(Duration::from_secs(20)),
            Ok(true),
            "the {job} once the player stopped"
        );
    }
}
