mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::beep;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player};
use vidcut_core::render::RenderPlan;

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

fn play_out(player: &Player, out: &ManualOutput) -> Vec<f32> {
    let mut sound = Vec::new();
    let started = Instant::now();
    while player.status().playing {
        assert!(started.elapsed() < Duration::from_secs(30), "too slow");
        sound.extend(pull_full(out, 480));
    }
    sound
}

fn sample(t: i64) -> i64 {
    fps().frame_to_sample(t, RATE)
}

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
    let from = sample(111) - 1;
    let mono = left(&sound);
    assert!(mono.len() as i64 > from, "all of it played");
    let ends: Vec<i64> = burst_ends(&mono, 0.1, 4_800)
        .into_iter()
        .map(|i| i as i64)
        .collect();
    let want: Vec<i64> = [90, 60, 30, 0].iter().map(|&f| from - sample(f)).collect();
    assert_eq!(ends, want, "the beeps where reversed play puts them");
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
    for speed in [0.5, 2.0, -0.5, -2.0] {
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
        assert!(
            (hz - 1000.0).abs() < 20.0,
            "{speed}x: the tone at {hz:.0} Hz"
        );
        let plan_secs = 4.0 / speed.abs();
        assert!(
            (seconds - plan_secs).abs() < 0.05,
            "{speed}x: {seconds:.2} s of sound for {plan_secs:.2}"
        );
        assert_eq!(heard, 4, "{speed}x: every beep");
    }
}

#[test]
fn a_step_plays_the_sound_of_its_frame() {
    let a = beeps();
    let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), true);
    let one = sample(1) as usize;
    let take = |out: &ManualOutput| {
        let start = Instant::now();
        while out.queued() < one as u64 && start.elapsed() < Duration::from_millis(300) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let queued = out.queued() as usize;
        left(&out.pull(queued, Instant::now()))
    };
    let silent = |s: &[f32]| s.iter().all(|v| v.abs() < 1e-3);
    player.seek(29);
    let s = take(&out);
    assert_eq!(s.len(), one, "one frame's sound");
    assert!(silent(&s), "frame 29 is silent");
    player.step(1);
    let s = take(&out);
    assert_eq!(s.len(), one);
    let fade = 144;
    for (i, &v) in s.iter().enumerate().take(one - fade).skip(fade) {
        let want = beep::sample_at(RATE, 48_000 + i as u64);
        assert!((v - want).abs() < 1e-3, "sample {i}: {v} for {want}");
    }
    player.step(1);
    assert!(silent(&take(&out)), "frame 31");
    player.step(-1);
    let s = take(&out);
    assert!(starts_with_beep(&s), "the beep from its start");
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
fn the_levels_are_the_peaks_of_the_sound_heard() {
    let a = beeps();
    let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), false);
    assert!(player.wait_ready(0, Duration::from_secs(10)));
    player.play(1.0);
    let heard = [pull_full(&out, 480), pull_full(&out, 480)].concat();
    let mut peaks = vec![0.0f32; 2];
    for frame in heard.chunks(2) {
        for (p, s) in peaks.iter_mut().zip(frame) {
            *p = p.max(s.abs());
        }
    }
    assert!(peaks[0] > 0.1, "the beep was heard");
    let levels = player.levels(Instant::now()).expect("playing");
    assert_eq!(levels.mix, peaks);
    assert_eq!(levels.lanes, vec![peaks]);
    for _ in 0..10 {
        pull_full(&out, 480);
    }
    let levels = player.levels(Instant::now()).expect("playing");
    assert_eq!(levels.mix, vec![0.0, 0.0]);
    player.pause();
    assert_eq!(player.levels(Instant::now()), None, "paused");
}

#[test]
fn backward_the_lanes_show_the_sound_heard() {
    let a = beeps();
    let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), false);
    player.seek(35);
    assert!(player.wait_ready(35, Duration::from_secs(10)));
    player.play(-1.0);
    let heard = [pull_full(&out, 480), pull_full(&out, 480)].concat();
    assert!(heard.iter().all(|&s| s == 0.0), "silence heard");
    let levels = player.levels(Instant::now()).expect("playing");
    assert_eq!(levels.lanes, vec![vec![0.0, 0.0]]);
}

#[test]
fn muted_the_mix_meter_still_shows_the_sound() {
    let a = beeps();
    let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), false);
    assert!(player.wait_ready(0, Duration::from_secs(10)));
    player.set_muted(true);
    player.play(1.0);
    let heard = [pull_full(&out, 480), pull_full(&out, 480)].concat();
    assert!(heard.iter().all(|&s| s == 0.0), "muted");
    let levels = player.levels(Instant::now()).expect("playing");
    assert!(levels.mix[0] > 0.1, "the beep on the mix's meter");
}
