mod player_support;

use std::time::{Duration, Instant};

use player_support::*;
use vidcut_av::fixture::{Container, FixtureSpec, VideoCodec};
use vidcut_av::player::{Config, ManualOutput, Player};
use vidcut_core::render::{Over, Ramp, VideoPiece};

#[test]
fn playing_backward_through_a_dissolve_decodes_each_frame_about_once() {
    let spec =
        |frames| FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), frames).gop(120);
    let (a, b) = (src("m1", spec(300)), src("m2", spec(330)));
    let mut p = plan(&[clip(&a, 0, 300)], 0);
    let over = plan(&[clip(&b, 0, 300)], 0).video.remove(0);
    let under = p.video.remove(0);
    p.video = vec![VideoPiece::Mix {
        under: Box::new(under),
        over: vec![Over {
            piece: over,
            ramp: Ramp {
                at: 0,
                of: 300,
                falling: false,
            },
        }],
    }];
    let out = ManualOutput::new();
    let player = Player::new(p, Config::default(), Box::new(out.clone())).unwrap();
    player.seek(299);
    assert!(player.wait_ready(299, Duration::from_secs(10)));
    let before = vidcut_av::decode::frames_decoded();
    player.play(-1.0);
    let started = Instant::now();
    while player.status().playing {
        assert!(started.elapsed() < Duration::from_secs(60), "too slow");
        pull_full(&out, 480);
        let t = player.status().position;
        assert!(player.wait_ready(t, Duration::from_secs(10)), "frame {t}");
    }
    let decoded = vidcut_av::decode::frames_decoded() - before;
    assert!(
        decoded <= 840,
        "{decoded} frames decoded to play 300 of a dissolve backward"
    );
}
