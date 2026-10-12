//! The player engine, headless (docs/decisions/player.md): every picture is
//! the one `frame render` makes, at seeks, steps both ways and playing
//! across cuts and files; the sound is the clock, and the pictures stay
//! with it. No display, no GPU, no audio device: a manual output pulls
//! the sound when a test says, a paced one in real time.

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::beep;
use vidcut_av::fixture::{Container, FixtureSpec, Sound, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Output, PacedOutput, Player};

fn h264() -> Src {
    src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 90),
    )
}

fn hevc() -> Src {
    src(
        "m2",
        FixtureSpec::new(VideoCodec::Hevc10, Container::Mkv, W, H, (30, 1), 90),
    )
}

/// PCM sound: beeps exact to the sample.
fn pcm() -> Src {
    src(
        "m3",
        FixtureSpec::new(VideoCodec::H264, Container::Mov, W, H, (30, 1), 90),
    )
}

fn manual(plan: vidcut_core::render::RenderPlan) -> (Player, ManualOutput) {
    let out = ManualOutput::new();
    let player = Player::new(plan, Config::new(Output::rgba(W, H)), Box::new(out.clone())).unwrap();
    (player, out)
}

#[test]
fn seeks_and_steps_show_what_frame_render_shows() {
    let (a, b) = (h264(), hevc());
    // Cuts within a file, between files and codecs, into and out of black;
    // the timeline starts at frame 1000.
    let p = plan(
        &[
            clip(&a, 0, 30),
            clip(&b, 10, 30),
            gap(5),
            clip(&a, 60, 30),
            clip(&a, 20, 10),
        ],
        1000,
    );
    let expect: Vec<Option<u32>> = (1000..1105).map(|t| rendered(&p, t)).collect();
    let (player, _out) = manual(p);
    let at = |t: i64| expect[(t - 1000) as usize];

    // Seeks to every boundary, both sides, and between.
    for t in [
        1000, 1029, 1030, 1031, 1059, 1060, 1064, 1065, 1094, 1095, 1104, 1050, 1001,
    ] {
        player.seek(t);
        assert_eq!(shown_at(&player, t), at(t), "seek to {t}");
    }
    // Steps forward through all of it, and back.
    player.seek(1000);
    for t in 1001..1105 {
        player.step(1);
        assert_eq!(shown_at(&player, t), at(t), "step forward to {t}");
    }
    for t in (1000..1104).rev() {
        player.step(-1);
        assert_eq!(shown_at(&player, t), at(t), "step back to {t}");
    }
    // Past the ends: the first and last frames.
    player.seek(5000);
    assert_eq!(shown_at(&player, 1104), at(1104));
    player.step(-10_000);
    assert_eq!(shown_at(&player, 1000), at(1000));
    assert_eq!(player.status().error, None);
}

#[test]
fn stills_and_image_sequences_show_their_pictures() {
    use vidcut_av::fixture::{self, ImageSpec};
    use vidcut_core::render::{AudioPiece, Image, VideoPiece};
    let a = h264();
    let still = fixture::ensure_images(&ImageSpec::still(777, W, H, "png")).unwrap();
    let seq = ImageSpec::sequence(1, 30, 4, W, H);
    let files = fixture::ensure_images(&seq).unwrap();
    let pattern = files[0].parent().unwrap().join("img_%04d.png");
    let picture = |path: &std::path::Path, image, from, frames| VideoPiece::Source {
        media: "i".into(),
        path: path.to_path_buf(),
        origin: 0.0,
        source_frame: from,
        frames,
        image: Some(image),
    };
    // a 0..20, the still for 15 frames, sequence pictures 5..25, a 40..50.
    let mut p = plan(&[clip(&a, 0, 20), gap(15), gap(20), clip(&a, 40, 10)], 0);
    p.video[1] = picture(&still[0], Image::Still, 0, 15);
    p.video[2] = picture(
        &files[0],
        Image::Sequence {
            pattern,
            first: 1,
            count: 30,
            rate: fps(),
        },
        5,
        20,
    );
    assert!(matches!(p.audio[0][1], AudioPiece::Silence { .. }));
    let expect: Vec<Option<u32>> = (0..65).map(|t| rendered(&p, t)).collect();
    assert_eq!(expect[25], Some(777));
    assert_eq!(expect[35], Some(5));
    let (player, out) = manual(p);
    for t in [19, 20, 27, 34, 35, 44, 54, 55, 64] {
        player.seek(t);
        assert_eq!(shown_at(&player, t), expect[t as usize], "seek to {t}");
    }
    for t in (0..64).rev() {
        player.step(-1);
        assert_eq!(shown_at(&player, t), expect[t as usize], "step back to {t}");
    }
    player.play(1.0);
    let mut shown = Vec::new();
    while player.status().playing {
        pull_full(&out, 480);
        let t = player.status().position;
        assert!(player.wait_ready(t, Duration::from_secs(10)), "frame {t}");
        let s = player.present(Instant::now()).unwrap();
        assert_eq!(
            code(&s.picture),
            expect[s.frame as usize],
            "frame {}",
            s.frame
        );
        if shown.last() != Some(&s.frame) {
            shown.push(s.frame);
        }
    }
    assert_eq!(shown, (0..65).collect::<Vec<_>>());
}

#[test]
fn playing_shows_every_frame_on_time_across_cuts_and_files() {
    let (a, b) = (pcm(), hevc());
    let p = plan(
        &[clip(&a, 0, 20), clip(&b, 40, 20), gap(3), clip(&a, 50, 20)],
        0,
    );
    let expect: Vec<Option<u32>> = (0..63).map(|t| rendered(&p, t)).collect();
    let (player, out) = manual(p.clone());
    player.seek(5);
    assert!(player.wait_ready(5, Duration::from_secs(10)));
    player.play(1.0);
    // 10 ms buffers: each frame (33 ms) is presented three or four times.
    let mut shown = Vec::new();
    let mut sound = Vec::new();
    while player.status().playing {
        sound.extend(pull_full(&out, 480));
        let t = player.status().position;
        assert!(player.wait_ready(t, Duration::from_secs(10)), "frame {t}");
        let s = player.present(Instant::now()).unwrap();
        assert!(s.exact);
        assert_eq!(
            code(&s.picture),
            expect[s.frame as usize],
            "frame {}",
            s.frame
        );
        if shown.last() != Some(&s.frame) {
            shown.push(s.frame);
        }
        if sound.len() > 10 * 48_000 * 2 {
            panic!("never reached the end: {:?}", player.status());
        }
    }
    assert_eq!(shown, (5..63).collect::<Vec<_>>(), "every frame, in order");
    assert_eq!(player.status().position, 62, "paused on the last frame");

    // The sound heard from frame 5 on: the beeps at source seconds 0 and 1
    // of the PCM file sit where the plan puts them. a's 0..20 holds no
    // beep after frame 5; a's 50..70 has source frame 60 (a beep) at
    // timeline 53.
    let mono: Vec<f32> = sound.chunks(2).map(|c| c[0]).collect();
    let onsets = beep::onsets(&mono, 0.1, 4_800);
    let start = fps().frame_to_sample(5, RATE);
    let at: Vec<i64> = onsets.iter().map(|&i| start + i as i64).collect();
    assert_eq!(at.len(), 1, "one beep: {at:?}");
    let expected = fps().frame_to_sample(53, RATE);
    assert!(
        (at[0] - expected).abs() <= 1,
        "beep at sample {} for frame 53's {expected}",
        at[0]
    );
}

#[test]
fn pictures_follow_the_sound_as_heard_through_a_device_with_latency() {
    // A real-time device stand-in, 10 ms buffers heard 40 ms after they
    // are pulled. The beep at each source second must be heard while its
    // frame is shown.
    let a = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mov, W, H, (30, 1), 120),
    );
    let p = plan(&[clip(&a, 0, 120)], 0);
    let (out, recording) =
        PacedOutput::recording(Duration::from_millis(10), Duration::from_millis(40));
    let player = Player::new(p, Config::new(Output::rgba(W, H)), Box::new(out)).unwrap();
    assert!(player.wait_ready(0, Duration::from_secs(10)));
    player.play(1.0);
    // The display: presents every 2 ms, noting when each frame first shows.
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
    assert!(!status.playing, "played to the end: {status:?}");
    for (f, _, c) in &first_shown {
        assert_eq!(*c, Some(*f as u32), "frame {f}'s picture");
    }
    let rec = recording.lock().unwrap();
    let mono: Vec<f32> = rec.samples.chunks(2).map(|c| c[0]).collect();
    let onsets = beep::onsets(&mono, 0.1, 4_800);
    assert_eq!(onsets.len(), 4, "a beep a second: {onsets:?}");
    // When each beep's frame took over from the frame before it, against
    // when the beep was heard. Frame 0 shows from before play starts: its
    // beep is measured by frame 1, a frame later.
    let mut offsets = Vec::new();
    for (k, &i) in onsets.iter().enumerate() {
        let heard = rec.heard_at(i, RATE).unwrap();
        let (frame, after) = match k {
            0 => (1, Duration::from_secs_f64(1.0 / 30.0)),
            _ => (30 * k as i64, Duration::ZERO),
        };
        let heard = heard + after;
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
    eprintln!(
        "picture after its beep (ms): {offsets:.1?}; late presents {}",
        status.late
    );
    // The display looks every 2 ms (and the scheduler may add a few).
    for ms in &offsets {
        assert!(
            (-1.0..=8.0).contains(ms),
            "picture vs. sound: {offsets:.1?} ms"
        );
    }
    // Nothing skipped while every frame was decoded in time.
    assert_eq!(
        first_shown.iter().map(|(f, ..)| *f).collect::<Vec<_>>(),
        (0..120).collect::<Vec<_>>()
    );
}

#[test]
fn speeds_and_reverse_play() {
    let a = h264();
    // Keyframes 120 frames apart, with B-frames: going back must decode
    // windows from keyframes.
    let long = src(
        "m4",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 300)
            .gop(120)
            .bframes(2),
    );
    let p = plan(&[clip(&a, 0, 90), clip(&long, 0, 300)], 0);
    let expect: Vec<Option<u32>> = (0..390).map(|t| rendered(&p, t)).collect();
    let (player, out) = manual(p);

    // Twice as fast: a second of sound moves the clock two seconds.
    player.seek(10);
    assert!(player.wait_ready(10, Duration::from_secs(10)));
    player.play(2.0);
    for _ in 0..100 {
        pull_full(&out, 480);
    }
    let t = player.status().position;
    assert!((68..=72).contains(&t), "2x for 1 s from 10 is at {t}");
    assert!(player.wait_ready(t, Duration::from_secs(10)));
    let s = player.present(Instant::now()).unwrap();
    assert_eq!(code(&s.picture), expect[s.frame as usize]);

    // Backwards at normal speed, across the cut back into the first file:
    // every frame shown, in order, each the export's.
    player.pause();
    player.seek(200);
    player.play(-1.0);
    let mut shown = Vec::new();
    let started = Instant::now();
    while player.status().position > 60 {
        assert!(started.elapsed() < Duration::from_secs(60), "too slow");
        pull_full(&out, 480);
        let t = player.status().position;
        assert!(player.wait_ready(t, Duration::from_secs(10)), "frame {t}");
        let s = player.present(Instant::now()).unwrap();
        assert_eq!(
            code(&s.picture),
            expect[s.frame as usize],
            "frame {}",
            s.frame
        );
        if shown.last() != Some(&s.frame) {
            shown.push(s.frame);
        }
    }
    assert_eq!(shown, (60..=200).rev().collect::<Vec<_>>());

    // Backwards to the start: play stops on the first frame.
    player.seek(5);
    player.play(-4.0);
    let started = Instant::now();
    while player.status().playing {
        assert!(started.elapsed() < Duration::from_secs(10));
        pull_full(&out, 480);
    }
    assert_eq!(player.status().position, 0);
    assert_eq!(shown_at(&player, 0), expect[0]);
}

#[test]
fn a_fader_and_the_mute_change_the_sound_while_it_plays() {
    let tone = src(
        "m5",
        FixtureSpec::new(VideoCodec::H264, Container::Mov, W, H, (30, 1), 90).sound(Sound::Tone),
    );
    let p = plan(&[clip(&tone, 0, 90)], 0);
    let (player, out) = manual(p);
    player.play(1.0);
    let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    let take = |ms: usize| {
        let mut all = Vec::new();
        for _ in 0..ms / 10 {
            all.extend(pull_full(&out, 480));
        }
        all
    };
    let loud = rms(&take(200));
    assert!(loud > 0.05, "the tone plays: {loud}");
    player.set_lane_gain(0, -20.0);
    // What was queued before plays out first (a tenth of a second).
    take(150);
    let quiet = rms(&take(100));
    assert!(
        (quiet / loud - 0.1).abs() < 0.01,
        "−20 dB: {quiet} after {loud}"
    );
    player.set_muted(true);
    take(150);
    assert_eq!(rms(&take(100)), 0.0, "muted");
    let pos = player.status().position;
    assert!(pos > 10, "the clock ran on: {pos}");
}

#[test]
fn requests_return_at_once_and_seeks_merge() {
    // 1080p frames that take a while to decode; the caller never waits.
    let big = src(
        "m6",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, 1920, 1080, (30, 1), 900),
    );
    let p = plan(&[clip(&big, 0, 900)], 0);
    let out = ManualOutput::new();
    let player = Player::new(p, Config::new(Output::rgba(960, 540)), Box::new(out)).unwrap();
    let mut slowest = Duration::ZERO;
    for k in 0..40 {
        let t = (k * 137) % 900;
        let start = Instant::now();
        player.seek(t);
        let _ = player.status();
        let _ = player.present(Instant::now());
        slowest = slowest.max(start.elapsed());
    }
    player.seek(333);
    let start = Instant::now();
    assert!(player.wait_ready(333, Duration::from_secs(30)));
    eprintln!(
        "40 seeks: the slowest call took {:.2} ms; the last seek's frame came {:.0} ms after it",
        slowest.as_secs_f64() * 1e3,
        start.elapsed().as_secs_f64() * 1e3
    );
    assert!(slowest < Duration::from_millis(5), "{slowest:?}");
    let s = player.present(Instant::now()).unwrap();
    assert_eq!(s.frame, 333);
    assert_eq!(code(&s.picture), Some(333));
}
