//! What the player decodes: each frame about once, playing forward and
//! backward; nothing again for pictures it keeps across an edit; nothing
//! after switching to a prepared plan; and seeks that arrive while one
//! decodes merge. One test, so the process-wide
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

    // Switching to the plan played next, after a step back in this one:
    // its prepared frame shows, and nothing more decodes there. The step
    // back was in the other plan, so no window behind is wanted in this
    // one (its frame 60 is lower than 199, which a step back would be).
    drop(player);
    let player = new_player();
    player.seek(200);
    assert_eq!(shown_at(&player, 200), Some(200));
    player.step(-1);
    assert_eq!(shown_at(&player, 199), Some(199));
    let next = plan(&[clip(&other, 0, 90)], 0);
    player.prepare(Some((next.clone(), 60)));
    // Prepared once nothing else is left to decode: the window behind 199.
    let started = Instant::now();
    while !player.status().prepared {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "never prepared"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let expect = rendered(&next, 60);
    let before = frames_decoded();
    player.set_plan_at(next.clone(), 60).unwrap();
    assert!(player.status().ready, "the prepared frame");
    assert_eq!(shown_at(&player, 60), expect);
    std::thread::sleep(Duration::from_millis(300));
    let switched = frames_decoded() - before;
    eprintln!("switching to a prepared frame: {switched} frames decoded");
    assert_eq!(switched, 0, "{switched} frames decoded after switching");

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
