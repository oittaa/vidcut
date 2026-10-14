//! The player's sound beyond playing forward (docs/decisions/player.md):
//! backward it plays reversed, still the clock; frame steps and paused
//! seeks play the frame's sound (spec §4.5, decision D42), as does the
//! scrubber alone. Headless: the fixtures' beeps (a 1 kHz tone for the
//! first 10 ms of every source second, PCM so exact to the sample) through
//! a manual or recording output.

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::beep;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player, Scrubber};
use vidcut_core::render::RenderPlan;

/// Four seconds of PCM beeps: source frames 0, 30, 60 and 90 start one.
fn beeps() -> Src {
    src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mov, W, H, (30, 1), 120),
    )
}

fn player(plan: RenderPlan, scrubbing: bool) -> (Player, ManualOutput) {
    let out = ManualOutput::new();
    let config = Config {
        audio_scrubbing: scrubbing,
        ..Config::default()
    };
    let player = Player::new(plan, config, Box::new(out.clone())).unwrap();
    (player, out)
}

/// Pulls until play stops, every buffer full but the last.
fn play_out(player: &Player, out: &ManualOutput) -> Vec<f32> {
    let mut sound = Vec::new();
    let started = Instant::now();
    while player.status().playing {
        assert!(started.elapsed() < Duration::from_secs(30), "too slow");
        sound.extend(pull_full(out, 480));
    }
    sound
}

/// The first sample of timeline frame `t`.
fn sample(t: i64) -> i64 {
    fps().frame_to_sample(t, RATE)
}

/// A frame's sound that starts with a beep: one, loud within the 3 ms
/// fade-in.
fn starts_with_beep(s: &[f32]) -> bool {
    matches!(beep::onsets(s, 0.1, 100).as_slice(), [i] if *i < 144)
}

#[test]
fn backward_the_sound_plays_reversed_sample_for_sample() {
    let a = beeps();
    let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), false);
    player.seek(110);
    assert!(player.wait_ready(110, Duration::from_secs(10)));
    player.play(-1.0);
    let sound = play_out(&player, &out);
    assert_eq!(player.status().position, 0, "back to the start");
    // Output sample i is timeline sample `from - i`: frame 110's last
    // sample first.
    let from = sample(111) - 1;
    let mono = left(&sound);
    assert!(mono.len() as i64 > from, "all of it played");
    // Each beep comes out reversed: its first sample last, at the place of
    // its first sample backward.
    let ends: Vec<i64> = burst_ends(&mono, 0.1, 4_800)
        .into_iter()
        .map(|i| i as i64)
        .collect();
    let want: Vec<i64> = [90, 60, 30, 0].iter().map(|&f| from - sample(f)).collect();
    assert_eq!(ends, want, "the beeps where reversed play puts them");
    // And it is the beep itself, reversed, to the sample: nothing dropped
    // or doubled on the way.
    for &end in &want[..3] {
        let start = 48_000 * (3 - want.iter().position(|&e| e == end).unwrap() as u64);
        for k in 0..480u64 {
            let heard = mono[(end - k as i64) as usize];
            let beep = beep::sample_at(RATE, start + k);
            assert!((heard - beep).abs() < 1e-3, "sample {k} of the beep");
        }
    }
}

#[test]
fn other_speeds_keep_the_pitch_both_ways() {
    // A steady 1 kHz tone stays 1 kHz; every beep is heard, and the sound
    // lasts as long as the speed makes it. At 4× a 10 ms beep lasts
    // 2.5 ms, a quarter of a fragment's hop, and may blend away: counted
    // up to 2×.
    let a = beeps();
    let tone = src(
        "m2",
        FixtureSpec::new(VideoCodec::H264, Container::Mov, W, H, (30, 1), 120)
            .sound(vidcut_av::fixture::Sound::Tone),
    );
    let played = |s: &Src, speed: f64| {
        let (player, out) = player(plan(&[clip(s, 0, 120)], 0), false);
        let from = if speed > 0.0 { 0 } else { 119 };
        player.seek(from);
        assert!(player.wait_ready(from, Duration::from_secs(10)));
        player.play(speed);
        left(&play_out(&player, &out))
    };
    for speed in [0.5, 2.0, 4.0, 8.0, -0.5, -2.0, -4.0, -8.0] {
        let sound = played(&tone, speed);
        let mid = &sound[sound.len() / 4..sound.len() * 3 / 4];
        let crossings = mid
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        let hz = crossings as f64 / 2.0 / (mid.len() as f64 / 48_000.0);
        let beeps = played(&a, speed);
        let heard = beep::onsets(&beeps, 0.1, (4_800.0 / speed.abs()) as usize).len();
        let seconds = beeps.len() as f64 / 48_000.0;
        eprintln!("{speed}x: the tone at {hz:.0} Hz; {heard} of 4 beeps; {seconds:.2} s of sound");
        assert!(
            (hz - 1000.0).abs() < 20.0,
            "{speed}x: the tone at {hz:.0} Hz"
        );
        let plan_secs = 4.0 / speed.abs();
        assert!(
            (seconds - plan_secs).abs() < 0.05,
            "{speed}x: {seconds:.2} s of sound for {plan_secs:.2}"
        );
        if speed.abs() <= 2.0 {
            assert_eq!(heard, 4, "{speed}x: every beep");
        }
    }
}

#[test]
fn a_step_plays_the_sound_of_its_frame() {
    let a = beeps();
    let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), true);
    let one = sample(1) as usize;
    let take = |out: &ManualOutput| {
        // A frame's sound arrives whole, or nothing within a while.
        let start = Instant::now();
        while out.queued() < one as u64 && start.elapsed() < Duration::from_millis(300) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let queued = out.queued() as usize;
        left(&out.pull(queued, Instant::now()))
    };
    let silent = |s: &[f32]| s.iter().all(|v| v.abs() < 1e-3);
    // A paused seek plays its frame: 29 has no beep.
    player.seek(29);
    let s = take(&out);
    assert_eq!(s.len(), one, "one frame's sound");
    assert!(silent(&s), "frame 29 is silent");
    // A step onto frame 30 plays the beep from its first sample, faded in
    // and out over its first and last 3 ms.
    player.step(1);
    let s = take(&out);
    assert_eq!(s.len(), one);
    let fade = 144;
    for (i, &v) in s.iter().enumerate().take(one - fade).skip(fade) {
        let want = beep::sample_at(RATE, 48_000 + i as u64);
        assert!((v - want).abs() < 1e-3, "sample {i}: {v} for {want}");
    }
    // Stepping back onto 30 plays it forward too (D42).
    player.step(1);
    assert!(silent(&take(&out)), "frame 31");
    player.step(-1);
    let s = take(&out);
    assert!(starts_with_beep(&s), "the beep from its start");
    // Mute and scrubbing off: steps are silent.
    player.set_muted(true);
    player.step(-30);
    assert!(take(&out).is_empty(), "muted");
    player.set_muted(false);
    player.set_audio_scrubbing(false);
    player.step(30);
    assert!(take(&out).is_empty(), "scrubbing off");
    assert_eq!(player.status().position, 30);
}

#[test]
fn held_steps_play_frame_after_frame_without_piling_up() {
    let a = beeps();
    let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), true);
    let one = sample(1) as u64;
    player.seek(0);
    let start = Instant::now();
    while out.queued() < one {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
    // Thirty steps while frame 0's sound waits to be heard: the newest
    // replaces the one waiting, nothing queues behind.
    for _ in 0..30 {
        player.step(1);
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(out.queued(), one, "only frame 0's sound is queued");
    let first = left(&out.pull(one as usize, Instant::now()));
    assert!(starts_with_beep(&first), "frame 0's beep");
    // Then the newest: frame 30, with the next second's beep.
    let start = Instant::now();
    while out.queued() < one {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the newest never came"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let newest = left(&out.pull(one as usize, Instant::now()));
    assert!(starts_with_beep(&newest), "frame 30's beep");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(out.queued(), 0, "nothing more");
}

#[test]
fn the_scrubber_plays_frames_without_a_player() {
    // For a viewer that shows pictures some other way: the sound of a
    // frame of a plan, nothing else.
    let a = beeps();
    let out = ManualOutput::new();
    let scrubber = Scrubber::new(RATE, 2, Box::new(out.clone())).unwrap();
    // The beep 10 frames into the second clip: timeline frame 50.
    scrubber.set_plan(plan(&[clip(&a, 0, 40), clip(&a, 50, 40)], 0));
    let one = sample(1) as u64;
    let take = || {
        let start = Instant::now();
        while out.queued() < one && start.elapsed() < Duration::from_millis(300) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let queued = out.queued() as usize;
        left(&out.pull(queued, Instant::now()))
    };
    scrubber.play(50);
    let s = take();
    assert_eq!(s.len() as u64, one);
    assert!(starts_with_beep(&s), "frame 50's beep");
    scrubber.play(49);
    let s = take();
    assert!(s.iter().all(|v| v.abs() < 1e-3), "frame 49 is silent");
    // A fader moved: the same beep, 20 dB down.
    scrubber.set_lane_gain(0, -20.0);
    scrubber.play(50);
    let s = take();
    let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.05).abs() < 0.005, "peak {peak}");
    assert_eq!(scrubber.error(), None);
}
