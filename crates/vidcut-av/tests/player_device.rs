//! The player on the system's audio device (`--features audio-tests`): its
//! callback runs the clock in real time. The plan is black and silence, so
//! nothing is heard. Skips when there is no device.
#![cfg(feature = "audio-tests")]

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::player::{Config, CpalOutput, Player};

#[test]
fn the_device_runs_the_clock() {
    let p = plan(&[gap(150)], 0);
    let player = match Player::new(p, Config::default(), Box::new(CpalOutput::new())) {
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

/// A device chosen by its id runs the clock, the default one listed among
/// the system's devices; one that isn't there plays on the default.
#[test]
fn a_chosen_device_runs_the_clock() {
    let devices = match vidcut_av::player::output_devices() {
        Ok(d) if !d.is_empty() => d,
        other => {
            eprintln!("skipping: no audio devices listed here ({other:?})");
            return;
        }
    };
    let default = devices.iter().find(|d| d.default).map(|d| d.id.clone());
    for chosen in [default, Some("wasapi:{gone}".to_string())] {
        let p = plan(&[gap(60)], 0);
        let output = Box::new(CpalOutput::on(chosen.clone()));
        let player =
            Player::new(p, Config::default(), output).unwrap_or_else(|e| panic!("{chosen:?}: {e}"));
        assert!(player.wait_ready(0, Duration::from_secs(5)), "{chosen:?}");
        let start = Instant::now();
        player.play(1.0);
        while player.status().position == 0 {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "{chosen:?}: the clock never moved"
            );
            std::thread::sleep(Duration::from_millis(5));
            player.present(Instant::now());
        }
        assert!(player.status().error.is_none(), "{chosen:?}");
    }
}
