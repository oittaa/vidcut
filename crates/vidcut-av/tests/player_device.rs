//! The player on the system's audio device (`--features audio-tests`): its
//! callback runs the clock in real time. The plan is black and silence, so
//! nothing is heard. Skips when there is no device.
#![cfg(feature = "audio-tests")]

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::player::{Config, CpalOutput, Player, PlaysOn};

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

/// Every device listed opens as itself, chosen by its id, and runs the
/// clock; their names are told apart. An id that isn't there plays on the
/// default, saying so.
#[test]
fn each_listed_device_plays_as_itself() {
    let devices = match vidcut_av::player::output_devices() {
        Ok(d) if !d.is_empty() => d,
        other => {
            eprintln!("skipping: no audio devices listed here ({other:?})");
            return;
        }
    };
    let names: std::collections::HashSet<&str> = devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names.len(), devices.len(), "names repeat: {devices:#?}");
    let runs_the_clock = |chosen: Option<String>, want: PlaysOn| {
        let p = plan(&[gap(60)], 0);
        let player = Player::new(
            p,
            Config::default(),
            Box::new(CpalOutput::on(chosen.clone())),
        )?;
        assert_eq!(player.plays_on(), want, "{chosen:?}");
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
        Ok::<_, vidcut_av::Error>(())
    };
    let mut opened = 0;
    for d in &devices {
        // A device may refuse the plan's rate and channels (a surround
        // output, an S/PDIF one); one that opens is the one chosen.
        match runs_the_clock(Some(d.id.clone()), PlaysOn::Chosen(d.id.clone())) {
            Ok(()) => {
                eprintln!("{} ({}) plays", d.name, d.id);
                opened += 1;
            }
            Err(e) => eprintln!("{} ({}) won't open: {e}", d.name, d.id),
        }
    }
    eprintln!("{opened} of {} devices opened", devices.len());
    assert!(opened > 0, "none opened: {devices:#?}");
    // Where the default opens.
    let gone = "wasapi:{gone}".to_string();
    let default = CpalOutput::new();
    if let Err(e) = Player::new(plan(&[gap(1)], 0), Config::default(), Box::new(default)) {
        eprintln!("the default device won't open: {e}");
        return;
    }
    runs_the_clock(Some(gone.clone()), PlaysOn::Missing(gone)).unwrap();
}
