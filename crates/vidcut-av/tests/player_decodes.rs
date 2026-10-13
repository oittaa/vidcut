//! What the player decodes: each frame about once, playing forward and
//! backward; nothing again for pictures it keeps across an edit; two
//! seconds behind a paused playhead while nothing else is wanted, so steps
//! back after a seek find their pictures, also after switching to a
//! prepared plan; and seeks that arrive while one decodes merge. One test, so the process-wide
//! decode count is this test's alone.

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::decode::frames_decoded;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player};

#[test]
fn frames_decode_about_once_and_edits_keep_them() {
    // Keyframes 120 frames apart, with B-frames.
    let long = src(
        "m1",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 300)
            .gop(120)
            .bframes(2),
    );
    let other = src(
        "m2",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 90),
    );
    let p = plan(&[clip(&long, 0, 300)], 0);
    let out = ManualOutput::new();
    let new_player = || Player::new(p.clone(), Config::default(), Box::new(out.clone())).unwrap();
    let play_until = |player: &Player, done: &dyn Fn(i64) -> bool| {
        let started = Instant::now();
        while player.status().playing {
            assert!(started.elapsed() < Duration::from_secs(60), "too slow");
            pull_full(&out, 480);
            let t = player.status().position;
            assert!(player.wait_ready(t, Duration::from_secs(10)), "frame {t}");
            let s = player.present(Instant::now()).unwrap();
            assert_eq!(code(&s.picture), Some(s.frame as u32));
            if done(t) {
                break;
            }
        }
    };

    // Forward: a frame a frame, and the few decoded ahead. An edit further
    // on while it plays keeps the open file where it is: no seek back to
    // the keyframe (frame 0), which would decode the first 75 again.
    let player = new_player();
    assert_eq!(shown_at(&player, 0), Some(0));
    let before = frames_decoded();
    player.play(1.0);
    play_until(&player, &|t| t >= 75);
    let later = plan(&[clip(&long, 0, 200), clip(&other, 0, 30)], 0);
    player.set_plan(later).unwrap();
    play_until(&player, &|t| t >= 150);
    player.pause();
    let forward = frames_decoded() - before;
    let at = player.status().position;
    eprintln!("playing 0..{at} with an edit on the way: {forward} frames decoded");
    assert!(
        forward <= at as u64 + 12,
        "{forward} frames decoded for {at}"
    );
    drop(player);

    // Backward from the end: windows from keyframes, each frame decoded
    // about once (exact seeks from a keyframe for each would be thousands).
    let player = new_player();
    player.seek(299);
    assert_eq!(shown_at(&player, 299), Some(299));
    let before = frames_decoded();
    player.play(-1.0);
    play_until(&player, &|_| false);
    assert_eq!(player.status().position, 0, "back to the start");
    let backward = frames_decoded() - before;
    eprintln!("playing 299..0 backwards: {backward} frames decoded");
    assert!(backward < 400, "{backward} frames decoded going back 299");

    // An edit that moves every frame 30 later: the pictures are kept by
    // what they show, so showing them again decodes nothing.
    let edited = plan(&[clip(&other, 0, 30), clip(&long, 0, 300)], 0);
    let frames = [40, 80, 130, 200, 329];
    let expect: Vec<_> = frames.iter().map(|&t| rendered(&edited, t)).collect();
    // Far from the edit, where the viewer stays.
    player.seek(150);
    assert_eq!(shown_at(&player, 150), Some(150));
    player.set_plan(edited).unwrap();
    let before = frames_decoded();
    for (&t, e) in frames.iter().zip(&expect) {
        player.seek(t);
        assert!(player.status().ready, "frame {t} is kept");
        assert_eq!(shown_at(&player, t), *e, "frame {t}");
    }
    assert_eq!(frames_decoded() - before, 0, "kept pictures decoded again");

    // Two seconds behind the playhead come while nothing else is wanted,
    // after a seek too, nearest first: each step back finds its picture
    // there at once. Pictures in memory come half a second at a time, each
    // chunk decoded from the keyframe on (in video memory a seek keeps all
    // it decodes on its way, tests/player_gpu.rs).
    let behind = |player: &Player, from: i64, to: i64| {
        for t in (from..to).rev() {
            assert!(
                player.wait_ready(t, Duration::from_secs(10)),
                "frame {t} behind"
            );
        }
    };
    let step_back = |player: &Player, from: i64, to: i64| {
        for t in (to..from).rev() {
            player.step(-1);
            assert!(player.status().ready, "step back to {t}: kept behind");
            assert_eq!(shown_at(player, t), rendered(&p, t), "step back to {t}");
        }
    };
    // Frame 200 is 80 past its keyframe (120). The seek keeps the last
    // half second it decodes on its way (frames 186..=200): the steps back
    // right after it find their pictures without waiting.
    drop(player);
    let player = new_player();
    player.seek(200);
    assert_eq!(shown_at(&player, 200), Some(200));
    for t in (186..200).rev() {
        player.step(-1);
        assert!(
            player.status().ready,
            "step back to {t}: kept from the seek"
        );
    }
    player.seek(200);
    assert_eq!(shown_at(&player, 200), Some(200));
    let before = frames_decoded();
    behind(&player, 140, 200);
    eprintln!(
        "two seconds behind a seek, half a second at a time: {} frames decoded",
        frames_decoded() - before
    );
    step_back(&player, 200, 160);
    // Just past a keyframe: the chunks behind reach back over it (frame
    // 120's sampling time is just before its keyframe's).
    drop(player);
    let player = new_player();
    player.seek(125);
    assert_eq!(shown_at(&player, 125), Some(125));
    behind(&player, 65, 125);
    step_back(&player, 125, 85);

    // Switching to the plan played next: its prepared frame shows at once,
    // and the two seconds behind it come while nothing else is wanted
    // (frames 0..60 of `other`, keyframes 30 apart), so the steps back
    // after it wait for nothing either.
    drop(player);
    let player = new_player();
    player.seek(200);
    assert_eq!(shown_at(&player, 200), Some(200));
    let next = plan(&[clip(&other, 0, 90)], 0);
    player.prepare(Some((next.clone(), 60)));
    let started = Instant::now();
    while !player.status().prepared {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "never prepared"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let expect = rendered(&next, 60);
    player.set_plan_at(next.clone(), 60).unwrap();
    assert!(player.status().ready, "the prepared frame");
    assert_eq!(shown_at(&player, 60), expect);
    for t in (1..60).rev() {
        assert!(
            player.wait_ready(t, Duration::from_secs(10)),
            "frame {t} behind"
        );
    }
    for t in (30..60).rev() {
        player.step(-1);
        assert!(player.status().ready, "step back to {t}");
    }

    // Seeks that arrive while one decodes merge into the last: a fresh
    // player, each seek far from the others.
    drop(player);
    let player = new_player();
    assert_eq!(shown_at(&player, 0), Some(0));
    let before = frames_decoded();
    for k in 1..=30 {
        player.seek((k * 97) % 300);
    }
    player.seek(250);
    assert_eq!(shown_at(&player, 250), Some(250));
    let merged = frames_decoded() - before;
    eprintln!("31 seeks in a row: {merged} frames decoded");
    // The seek decoding when they came, and the last: a GOP each at most,
    // and the few ahead.
    assert!(
        merged <= 2 * 120 + 20,
        "{merged} frames decoded for 31 seeks"
    );
}
