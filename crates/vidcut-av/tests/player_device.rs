//! The player on the system's audio device (`--features audio-tests`): its
//! callback runs the clock in real time. The plan is black and silence, so
//! nothing is heard. Skips when there is no device.
#![cfg(feature = "audio-tests")]

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::player::{Config, CpalOutput, Output, Player};

#[test]
fn the_device_runs_the_clock() {
    let p = plan(&[gap(150)], 0);
    let player = match Player::new(
        p,
        Config::new(Output::rgba(W, H)),
        Box::new(CpalOutput::new()),
    ) {
        Ok(player) => player,
        Err(e) => {
            eprintln!("skipping: no audio device here ({e})");
            return;
        }
    };
    assert!(player.wait_ready(0, Duration::from_secs(5)));
    let start = Instant::now();
    player.play(1.0);
    let mut moved = None;
    while start.elapsed() < Duration::from_millis(1500) {
        let s = player.present(Instant::now()).unwrap();
        if s.frame > 0 && moved.is_none() {
            moved = Some(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let elapsed = start.elapsed();
    let status = player.status();
    let moved = moved.expect("the clock never moved");
    eprintln!(
        "the clock moved {:.0} ms after play; {} frames in {:.2} s; output latency {:?}; {:?}",
        moved.as_secs_f64() * 1e3,
        status.position,
        elapsed.as_secs_f64(),
        status.latency,
        status.error
    );
    assert!(
        status
            .latency
            .is_some_and(|l| l < Duration::from_millis(500))
    );
    assert!(status.error.is_none(), "{:?}", status.error);
    // Real time, give or take the device's latency at the start.
    let expected = (elapsed - moved).as_secs_f64() * 30.0;
    assert!(
        (status.position as f64 - expected).abs() <= 4.0,
        "{} frames, expected about {expected:.0}",
        status.position
    );
}
