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
use vidcut_av::player::{Config, ManualOutput, PacedOutput, Player, Scrubber};
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
fn backward_the_pictures_stay_with_the_reversed_sound() {
    // A real-time device stand-in, buffers heard 40 ms after they are
    // pulled. Backward, a beep's first sample is the last of it heard, and
    // right after it frame 30k gives way to frame 30k - 1.
    let a = beeps();
    let p = plan(&[clip(&a, 0, 120)], 0);
    let (out, recording) =
        PacedOutput::recording(Duration::from_millis(10), Duration::from_millis(40));
    let config = Config {
        audio_scrubbing: false,
        ..Config::default()
    };
    let player = Player::new(p, config, Box::new(out)).unwrap();
    player.seek(119);
    assert!(player.wait_ready(119, Duration::from_secs(10)));
    player.play(-1.0);
    let mut first_shown: Vec<(i64, Instant, Option<u32>)> = Vec::new();
    let started = Instant::now();
    while player.status().playing && started.elapsed() < Duration::from_secs(8) {
        let now = Instant::now();
        if let Some(s) = player.present(now)
            && s.exact
            && first_shown.last().is_none_or(|(f, ..)| *f != s.frame)
        {
            first_shown.push((s.frame, now, code(&s.picture)));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let status = player.status();
    drop(player);
    assert!(!status.playing, "played to the start: {status:?}");
    for (f, _, c) in &first_shown {
        assert_eq!(*c, Some(*f as u32), "frame {f}'s picture");
    }
    assert_eq!(
        first_shown.iter().map(|(f, ..)| *f).collect::<Vec<_>>(),
        (0..=119).rev().collect::<Vec<_>>(),
        "every frame, backward"
    );
    let rec = recording.lock().unwrap();
    let ends = burst_ends(&left(&rec.samples), 0.1, 4_800);
    assert_eq!(ends.len(), 4, "a beep a second: {ends:?}");
    let mut offsets = Vec::new();
    for (k, &end) in ends[..3].iter().enumerate() {
        // The sample after the beep's first is frame 30k - 1's last.
        let heard = rec.heard_at(end + 1, RATE).unwrap();
        let frame = 30 * (3 - k as i64) - 1;
        let (_, shown, _) = first_shown
            .iter()
            .find(|(f, ..)| *f == frame)
            .unwrap_or_else(|| panic!("frame {frame} never shown"));
        let ms = if *shown >= heard {
            (*shown - heard).as_secs_f64() * 1e3
        } else {
            -(heard - *shown).as_secs_f64() * 1e3
        };
        offsets.push(ms);
    }
    eprintln!("backward, picture after its beep (ms): {offsets:.1?}");
    for ms in &offsets {
        assert!(
            (-1.0..=8.0).contains(ms),
            "picture vs. sound: {offsets:.1?} ms"
        );
    }
}

#[test]
fn other_speeds_backward_keep_the_pitch() {
    // Through atempo: the beeps come at the distance the speed makes, still
    // at 1 kHz, within atempo's window (about a frame) of where the picture
    // is. At 8× a 10 ms beep lasts 1.25 ms, shorter than that window: some
    // are smeared away, so 8× is only printed.
    let a = beeps();
    for speed in [-0.5, -2.0, -4.0, -8.0] {
        let (player, out) = player(plan(&[clip(&a, 0, 120)], 0), false);
        player.seek(119);
        assert!(player.wait_ready(119, Duration::from_secs(10)));
        player.play(speed);
        let sound = play_out(&player, &out);
        let mono = left(&sound);
        let from = sample(120) - 1;
        let gap = (4_800.0 / speed.abs()) as usize;
        let ends = burst_ends(&mono, 0.1, gap);
        let mut found = Vec::new();
        for (k, &end) in ends.iter().enumerate().take(4) {
            let nominal = (from - sample(30 * (3 - k as i64))) as f64 / speed.abs();
            let off_ms = (end as f64 - nominal) / 48.0;
            // Zero crossings just before the burst's end: 2 a period.
            let burst = &mono[end.saturating_sub(100)..end];
            let crossings = burst
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
            let hz = crossings as f64 / 2.0 / (burst.len() as f64 / 48_000.0);
            found.push((off_ms, hz));
        }
        eprintln!(
            "{speed}x: {} of 4 beeps, (ms off, Hz) {found:.1?}; {:.2} s of sound",
            ends.len(),
            mono.len() as f64 / 48_000.0
        );
        if speed.abs() <= 4.0 {
            assert_eq!(ends.len(), 4, "{speed}x: every beep: {ends:?}");
            for (k, (off_ms, hz)) in found.iter().enumerate() {
                assert!(off_ms.abs() < 35.0, "{speed}x: beep {k} {off_ms:.1} ms off");
                assert!(
                    (800.0..1250.0).contains(hz),
                    "{speed}x: beep {k} at {hz:.0} Hz"
                );
            }
        }
        assert!(
            (mono.len() as f64 - from as f64 / speed.abs()).abs() < 0.05 * 48_000.0,
            "{speed}x: {} samples of sound for {from} of plan",
            mono.len()
        );
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
    // over 3 ms and out at the frame's end.
    player.step(1);
    let s = take(&out);
    assert_eq!(s.len(), one);
    let fade = 144;
    for (i, &v) in s.iter().enumerate() {
        let g = if i < fade {
            (i + 1) as f32 / fade as f32
        } else if i >= one - fade {
            (one - i) as f32 / fade as f32
        } else {
            1.0
        };
        let want = beep::sample_at(RATE, 48_000 + i as u64) * g;
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
