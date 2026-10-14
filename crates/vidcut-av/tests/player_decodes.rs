//! The player's pictures across edits and keyframes: playing on while the
//! plan changes ahead, seeking after an edit that moves every frame, and
//! stepping back over a keyframe.

mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player};

#[test]
fn edits_and_keyframes_show_the_right_frames() {
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

    // An edit further on while it plays: every frame is still its own.
    let player = new_player();
    assert_eq!(shown_at(&player, 0), Some(0));
    player.play(1.0);
    play_until(&player, &|t| t >= 75);
    let later = plan(&[clip(&long, 0, 200), clip(&other, 0, 30)], 0);
    player.set_plan(later).unwrap();
    play_until(&player, &|t| t >= 150);
    player.pause();

    // An edit that moves every frame 30 later: seeks show the moved frames.
    let edited = plan(&[clip(&other, 0, 30), clip(&long, 0, 300)], 0);
    player.seek(150);
    assert_eq!(shown_at(&player, 150), Some(150));
    player.set_plan(edited.clone()).unwrap();
    for t in [40, 80, 130, 200, 329] {
        player.seek(t);
        assert_eq!(shown_at(&player, t), rendered(&edited, t), "frame {t}");
    }

    // Steps back from just past a keyframe (120) over it.
    drop(player);
    let player = new_player();
    player.seek(125);
    assert_eq!(shown_at(&player, 125), Some(125));
    for t in (85..125).rev() {
        player.step(-1);
        assert_eq!(shown_at(&player, t), rendered(&p, t), "step back to {t}");
    }
}
