mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::beep;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player};

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

fn pcm() -> Src {
    src(
        "m3",
        FixtureSpec::new(VideoCodec::H264, Container::Mov, W, H, (30, 1), 90),
    )
}

fn manual(plan: vidcut_core::render::RenderPlan) -> (Player, ManualOutput) {
    let out = ManualOutput::new();
    let config = Config {
        audio_scrubbing: false,
        ..Config::default()
    };
    let player = Player::new(plan, config, Box::new(out.clone())).unwrap();
    (player, out)
}

#[test]
fn seeks_and_steps_show_what_frame_render_shows() {
    let (a, b) = (h264(), hevc());
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

    for t in [
        1000, 1029, 1030, 1031, 1059, 1060, 1064, 1065, 1094, 1095, 1104, 1050, 1001,
    ] {
        player.seek(t);
        assert_eq!(shown_at(&player, t), at(t), "seek to {t}");
    }
    player.seek(1000);
    for t in 1001..1105 {
        player.step(1);
        assert_eq!(shown_at(&player, t), at(t), "step forward to {t}");
    }
    for t in (1000..1104).rev() {
        player.step(-1);
        assert_eq!(shown_at(&player, t), at(t), "step back to {t}");
    }
    player.seek(5000);
    assert_eq!(shown_at(&player, 1104), at(1104));
    player.step(-10_000);
    assert_eq!(shown_at(&player, 1000), at(1000));
    assert_eq!(player.status().error, None);
}

#[test]
fn stills_and_image_sequences_show_their_pictures() {
    use vidcut_av::fixture::{self, ImageSpec};
    use vidcut_core::render::{Image, VideoPiece};
    let a = h264();
    let still = fixture::ensure_images(&ImageSpec::still(777, W, H, "png")).unwrap();
    let seq = ImageSpec::sequence(1, 30, 4, W, H);
    let files = fixture::ensure_images(&seq).unwrap();
    let pattern = files[0].parent().unwrap().join("img_%04d.png");
    let picture = |path: &std::path::Path, image, from, frames| VideoPiece::Source {
        pace: Default::default(),
        media: "i".into(),
        path: path.to_path_buf(),
        origin: 0.0,
        source_frame: from,
        frames,
        image: Some(image),
    };
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
fn speeds_and_reverse_play() {
    let a = h264();
    let long = src(
        "m4",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 300)
            .gop(120)
            .bframes(2),
    );
    let p = plan(&[clip(&a, 0, 90), clip(&long, 0, 300)], 0);
    let expect: Vec<Option<u32>> = (0..390).map(|t| rendered(&p, t)).collect();
    let (player, out) = manual(p);

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
fn pieces_at_other_speeds_show_their_source_frames() {
    use vidcut_core::render::{Pace, VideoPiece};

    let long = src(
        "m5",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 300)
            .gop(30)
            .bframes(2),
    );
    let mut p = plan(
        &[
            clip(&long, 10, 10),
            clip(&long, 100, 10),
            clip(&long, 200, 40),
            clip(&long, 50, 5),
            clip(&long, 260, 5),
        ],
        0,
    );
    for (piece, speed) in p.video.iter_mut().zip([2.0, 0.5, -1.0, 0.0, 1.0]) {
        if let VideoPiece::Source { pace, .. } = piece {
            *pace = Pace::of(speed, 0);
        }
    }
    let expect: Vec<Option<u32>> = [
        (0..10).map(|k| 10 + 2 * k).collect::<Vec<_>>(),
        (0..10).map(|k| 100 + k / 2).collect(),
        (0..40).map(|k| 200 - k).collect(),
        vec![50; 5],
        (260..265).collect(),
    ]
    .concat()
    .into_iter()
    .map(|f| Some(f as u32))
    .collect();
    let n = expect.len() as i64;
    for t in 0..n {
        assert_eq!(rendered(&p, t), expect[t as usize], "frame render at {t}");
    }
    let (player, out) = manual(p);

    for t in [0, 9, 10, 15, 19, 20, 35, 59, 60, 64, 65, 30] {
        player.seek(t);
        assert_eq!(shown_at(&player, t), expect[t as usize], "seek to {t}");
    }
    player.seek(0);
    for t in 1..n {
        player.step(1);
        assert_eq!(shown_at(&player, t), expect[t as usize], "step to {t}");
    }
    for t in (0..n - 1).rev() {
        player.step(-1);
        assert_eq!(shown_at(&player, t), expect[t as usize], "step back to {t}");
    }

    for (from, speed) in [(0, 1.0), (n - 1, -1.0)] {
        player.pause();
        player.seek(from);
        assert!(player.wait_ready(from, Duration::from_secs(10)));
        player.play(speed);
        let mut shown = Vec::new();
        let started = Instant::now();
        while player.status().playing {
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
        let all: Vec<i64> = if speed > 0.0 {
            (from..n).collect()
        } else {
            (0..=from).rev().collect()
        };
        assert_eq!(shown, all, "playing at {speed}");
    }
    assert_eq!(player.status().error, None);
}

#[test]
fn a_device_that_stops_pulling_holds_nothing_up() {
    let a = pcm();
    let (player, out) = manual(plan(&[clip(&a, 0, 90)], 0));
    player.play(1.0);
    for k in 0..40 {
        player.seek(k * 2);
        std::thread::sleep(Duration::from_millis(5));
    }
    player.seek(60);
    let mut sound = out.pull(480, Instant::now() - Duration::from_millis(10));
    sound.extend(pull_full(&out, 480));
    let mono: Vec<f32> = sound.chunks(2).map(|c| c[0]).collect();
    let onsets = beep::onsets(&mono, 0.1, 400);
    assert!(
        onsets == [0] || onsets == [480],
        "frame 60's beep: {onsets:?}"
    );
    assert_eq!(player.status().position, 60, "in frame 60");
}
