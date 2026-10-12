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
use vidcut_av::player::{Config, ManualOutput, PacedOutput, Player};

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

/// A player on a manual output. Without audio scrubbing: a seek before
/// play would otherwise queue its frame's sound ahead of the play's, and
/// the tests that count samples from play's start would hear it first.
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
fn pictures_are_yuv_as_decoded_and_render_as_frame_render() {
    use vidcut_av::color::Matrix;
    use vidcut_av::fixture::{self, ImageSpec};
    use vidcut_av::player::{Layout, Memory};
    use vidcut_core::render::{Image, VideoPiece};
    let a = h264();
    let b = hevc();
    // Stored on its side, shown upright by its display matrix.
    let turned = src(
        "m4",
        FixtureSpec::new(VideoCodec::H264, Container::Mp4, W, H, (30, 1), 30).rotation(90),
    );
    // 4:2:2 10-bit: not a layout a display takes, so made one.
    let prores = src(
        "m5",
        FixtureSpec::new(VideoCodec::ProRes422Hq, Container::Mov, W, H, (25, 1), 50),
    );
    let mut p = plan(
        &[
            clip(&a, 0, 10),
            clip(&b, 0, 10),
            clip(&turned, 0, 10),
            clip(&prores, 0, 10),
            gap(5),
            gap(10),
        ],
        0,
    );
    let still = fixture::ensure_images(&ImageSpec::still(777, W, H, "png")).unwrap();
    p.video[5] = VideoPiece::Source {
        media: "i".into(),
        path: still[0].clone(),
        origin: 0.0,
        source_frame: 0,
        frames: 10,
        image: Some(Image::Still),
    };
    let (player, _out) = manual(p.clone());
    let picture = |t: i64| {
        player.seek(t);
        assert!(player.wait_ready(t, Duration::from_secs(10)));
        player.present(Instant::now()).unwrap().picture
    };
    // 8-bit H.264: its own planes, by reference, and the export's own
    // conversion of them is frame render's picture to the byte.
    let pic = picture(5);
    assert_eq!(pic.layout(), Layout::Yuv420p);
    assert_eq!(pic.memory(), Memory::Cpu);
    assert_eq!((pic.width(), pic.height(), pic.rotation()), (W, H, 0));
    assert_eq!(pic.bytes(), (W * H * 3 / 2) as usize);
    let rendered_rgba = vidcut_av::export::still::rgba(&piece_at(&p, 5), p.fps, W, H).unwrap();
    assert_eq!(pic.to_rgba(W, H).unwrap().rgba, rendered_rgba.rgba);
    // 10-bit HEVC: P010.
    let pic = picture(15);
    assert_eq!(pic.layout(), Layout::P010);
    assert_eq!(code(&pic), Some(15 - 10));
    // Turned: stored 180×320, upright by a quarter turn, fitted whole.
    let pic = picture(25);
    assert_eq!((pic.width(), pic.height(), pic.rotation()), (H, W, 90));
    assert_eq!(pic.placement(W, H), (0, 0, W, H));
    let img = pic.to_rgba(W, H).unwrap();
    assert_eq!(code_rgba(&img.rgba, W, H), rendered(&p, 25));
    // ProRes 4:2:2 10-bit, made P010.
    let pic = picture(35);
    assert_eq!(pic.layout(), Layout::P010);
    let img = pic.to_rgba(W, H).unwrap();
    assert_eq!(code_rgba(&img.rgba, W, H), rendered(&p, 35));
    // Black: the plan's size, 8-bit 4:2:0.
    let pic = picture(42);
    assert_eq!(
        (pic.layout(), pic.width(), pic.height()),
        (Layout::Yuv420p, W, H)
    );
    // A still (RGB in its file): coded BT.709 video range.
    let pic = picture(50);
    assert_eq!(pic.layout(), Layout::Yuv420p);
    let colors = pic.colors();
    assert_eq!((colors.matrix, colors.full_range), (Matrix::Bt709, false));
    let img = pic.to_rgba(W, H).unwrap();
    assert_eq!(code_rgba(&img.rgba, W, H), Some(777));
}

#[test]
fn a_picture_brought_to_a_display_layout_keeps_its_colors() {
    use vidcut_av::color::Matrix;
    use vidcut_av::fixture::Picture as Content;
    use vidcut_av::player::Layout;
    // HD ProRes 4:2:2 in colored stripes, read as BT.709: brought to 4:2:0
    // it must keep its matrix and range, or the stripes would come out tens
    // of levels off what frame render makes of them.
    let prores = src(
        "m1",
        FixtureSpec::new(
            VideoCodec::ProRes422Hq,
            Container::Mov,
            1280,
            720,
            (30, 1),
            10,
        )
        .picture(Content::Colors),
    );
    let p = plan(&[clip(&prores, 0, 10)], 0);
    let (player, _out) = manual(p.clone());
    player.seek(5);
    assert!(player.wait_ready(5, Duration::from_secs(10)));
    let pic = player.present(Instant::now()).unwrap().picture;
    assert_eq!(pic.layout(), Layout::P010);
    assert_eq!(pic.colors().matrix, Matrix::Bt709);
    let ours = pic.to_rgba(W, H).unwrap();
    let theirs = vidcut_av::export::still::rgba(&piece_at(&p, 5), p.fps, W, H).unwrap();
    let diff: Vec<u32> = ours
        .rgba
        .iter()
        .zip(&theirs.rgba)
        .map(|(a, b)| u32::from(a.abs_diff(*b)))
        .collect();
    let mean = diff.iter().sum::<u32>() as f64 / diff.len() as f64;
    // 4:2:0 instead of 4:2:2 blurs the chroma at the stripes' edges.
    assert!(mean < 1.5, "{mean:.2} levels apart on average");
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
    let player = Player::new(p, Config::default(), Box::new(out)).unwrap();
    assert!(player.wait_ready(0, Duration::from_secs(10)));
    // The clock is read at exact instants half a millisecond apart, so the
    // scheduler running this thread late changes nothing; a display
    // presents meanwhile, and what it shows is the frame's own picture.
    let mut trace = ClockTrace::new(Instant::now());
    player.play(1.0);
    let started = Instant::now();
    while player.status().playing && started.elapsed() < Duration::from_secs(8) {
        if let Some(s) = player.present(Instant::now())
            && s.exact
        {
            assert_eq!(code(&s.picture), Some(s.frame as u32), "frame {}", s.frame);
        }
        trace.read(&player, Instant::now());
        std::thread::sleep(Duration::from_millis(2));
    }
    trace.read(&player, Instant::now());
    let status = player.status();
    drop(player);
    assert!(!status.playing, "played to the end: {status:?}");
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
        let shown = trace
            .first_at(frame)
            .unwrap_or_else(|| panic!("frame {frame} never shown"));
        offsets.push(ms_between(heard, shown));
    }
    eprintln!(
        "picture after its beep (ms): {offsets:.1?}; late presents {}",
        status.late
    );
    // The bound the polling display of before needed (it looked every
    // 2 ms), kept.
    for ms in &offsets {
        assert!(
            (-1.0..=8.0).contains(ms),
            "picture vs. sound: {offsets:.1?} ms"
        );
    }
    // The clock went through every frame, in order.
    assert_eq!(
        trace.moves.iter().map(|(f, _)| *f).collect::<Vec<_>>(),
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
fn a_device_that_stops_pulling_holds_nothing_up() {
    // Seeks while playing drop what was queued; a device that pulls
    // nothing leaves the dropped sound in the ring. The player must neither
    // block on it nor play the sound from before the last seek.
    let a = pcm();
    let (player, out) = manual(plan(&[clip(&a, 0, 90)], 0));
    player.play(1.0);
    for k in 0..40 {
        player.seek(k * 2);
        std::thread::sleep(Duration::from_millis(5));
    }
    player.seek(60);
    // The device comes back. Its first buffer passes over the dropped sound
    // and may find nothing yet: the ring was full of it. Then the sound is
    // the last seek's, from the beep that starts source second 2, and the
    // clock waited for it.
    let mut sound = out.pull(480, Instant::now() - Duration::from_millis(10));
    sound.extend(pull_full(&out, 480));
    let mono: Vec<f32> = sound.chunks(2).map(|c| c[0]).collect();
    let onsets = beep::onsets(&mono, 0.1, 400);
    assert!(
        onsets == [0] || onsets == [480],
        "frame 60's beep: {onsets:?}"
    );
    assert_eq!(player.status().position, 60, "in frame 60");
    let start = Instant::now();
    drop(player);
    assert!(start.elapsed() < Duration::from_secs(2), "closing waited");
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
    let player = Player::new(p, Config::default(), Box::new(out)).unwrap();
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
