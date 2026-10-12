//! The viewer's player, headless: the picture is the sampling rule's frame,
//! and the mixed audio stays with it across seeks. Software frames, no
//! display and no GPU.

use vidcut_av::decode::read_barcode;
use vidcut_av::ffmpeg::{format::Pixel, frame};
use vidcut_av::fixture::{self, AudioCodec, Container, FixtureSpec, Sound, VideoCodec};
use vidcut_av::play::Player;
use vidcut_av::thumbs::Image;
use vidcut_core::render::{AudioPiece, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

struct Clip {
    path: std::path::PathBuf,
    origin: f64,
    frames: i64,
    rate: u32,
    channels: u16,
}

fn clip(spec: FixtureSpec) -> Clip {
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Clip {
        path,
        origin,
        frames: i64::from(spec.frames),
        rate: spec.audio.map(|a| a.rate).unwrap_or(48_000),
        channels: spec.audio.map(|a| a.channels).unwrap_or(2),
    }
}

fn source(c: &Clip, source_frame: i64, frames: i64) -> VideoPiece {
    VideoPiece::Source {
        media: "m".into(),
        path: c.path.clone(),
        origin: c.origin,
        source_frame,
        frames,
        image: None,
    }
}

fn audio(c: &Clip, source_sample: i64, samples: i64, gain_db: f64) -> AudioPiece {
    AudioPiece::Source {
        media: "m".into(),
        path: c.path.clone(),
        stream: 0,
        origin: c.origin,
        source_sample,
        samples,
        gain_db,
    }
}

fn plan(c: &Clip, video: Vec<VideoPiece>, audio_lane: Vec<AudioPiece>) -> RenderPlan {
    let frames = video.iter().map(VideoPiece::frames).sum();
    let fps = Rational::new(30, 1).unwrap();
    RenderPlan {
        fps,
        width: 320,
        height: 180,
        sample_rate: c.rate,
        channels: c.channels,
        frames,
        video,
        audio: vec![audio_lane],
        range: (0, frames),
        name: "play".into(),
    }
}

fn code(img: &Image) -> Option<u32> {
    let mut f = frame::Video::new(Pixel::RGBA, img.width, img.height);
    let stride = f.stride(0);
    let row = img.width as usize * 4;
    for y in 0..img.height as usize {
        f.data_mut(0)[y * stride..y * stride + row]
            .copy_from_slice(&img.rgba[y * row..(y + 1) * row]);
    }
    read_barcode(&f).unwrap()
}

fn black(img: &Image) -> bool {
    img.rgba.chunks(4).all(|p| p[0] < 8 && p[1] < 8 && p[2] < 8)
}

fn pcm() -> Clip {
    clip(
        FixtureSpec::new(VideoCodec::H264, Container::Mov, 320, 180, (30, 1), 90).with_audio(
            AudioCodec::PcmS16,
            48_000,
            2,
        ),
    )
}

#[test]
fn seeks_and_steps_show_the_source_frame() {
    let c = pcm();
    let fps = Rational::new(30, 1).unwrap();
    let samples = fps.frame_to_sample(c.frames, c.rate);
    let p = plan(
        &c,
        vec![
            source(&c, 0, 30),
            VideoPiece::Black { frames: 10 },
            source(&c, 40, 20),
        ],
        vec![audio(&c, 0, samples, 0.0)],
    );
    let mut player = Player::new(p).unwrap();
    assert_eq!(code(player.picture()), Some(0));

    player.seek(29).unwrap();
    assert_eq!(player.frame(), 29);
    assert_eq!(code(player.picture()), Some(29));

    player.seek(35).unwrap();
    assert!(black(player.picture()), "a gap is black");

    player.seek(40).unwrap();
    assert_eq!(code(player.picture()), Some(40));
    player.step(-1).unwrap();
    assert_eq!(player.frame(), 39);
    assert!(
        black(player.picture()),
        "the frame before the cut is the gap"
    );

    player.seek(0).unwrap();
    for n in 1..=5 {
        player.step(1).unwrap();
        assert_eq!(code(player.picture()), Some(n));
    }
}

#[test]
fn the_beep_stays_with_its_frame_across_a_seek() {
    let c = pcm();
    let fps = Rational::new(30, 1).unwrap();
    let p = plan(
        &c,
        vec![source(&c, 0, c.frames)],
        vec![audio(&c, 0, fps.frame_to_sample(c.frames, c.rate), 0.0)],
    );
    let mut player = Player::new(p).unwrap();
    player.set_speed(1.0).unwrap();

    let start = player.pull(1_000).unwrap();
    assert!(start.frame.is_none(), "the first samples are still frame 0");
    assert!(
        start.samples.iter().any(|s| s.abs() > 0.2),
        "frame 0 starts on the beep"
    );

    // 100 ms is past the 10 ms beep: the rest of the second is quiet.
    let later = player.pull(c.rate as usize / 10).unwrap();
    assert!(
        later.samples.iter().all(|s| s.abs() < 0.05),
        "after the beep the second is quiet"
    );

    player.seek(30).unwrap();
    assert_eq!(code(player.picture()), Some(30));
    let again = player.pull(1_000).unwrap();
    assert!(
        again.samples.iter().any(|s| s.abs() > 0.2),
        "one second in, the next beep plays"
    );
}

#[test]
fn a_fader_changes_the_level_without_restarting() {
    let c = clip(
        FixtureSpec::new(VideoCodec::H264, Container::Mov, 320, 180, (30, 1), 60)
            .with_audio(AudioCodec::PcmS16, 48_000, 2)
            .sound(Sound::Tone),
    );
    let fps = Rational::new(30, 1).unwrap();
    let p = plan(
        &c,
        vec![source(&c, 0, c.frames)],
        vec![audio(&c, 0, fps.frame_to_sample(c.frames, c.rate), 0.0)],
    );
    let mut player = Player::new(p).unwrap();
    player.set_speed(1.0).unwrap();
    let loud = player.pull(4_800).unwrap();
    let loud_rms = rms(&loud.samples);
    assert!(loud_rms > 0.05, "the tone is audible ({loud_rms})");

    player.set_lane_db(0, -60.0).unwrap();
    let quiet = player.pull(4_800).unwrap();
    let quiet_rms = rms(&quiet.samples);
    assert!(
        quiet_rms < loud_rms / 100.0,
        "the fader dropped the level ({quiet_rms} from {loud_rms})"
    );
    assert!(
        player.frame() > 0,
        "playback kept its place at frame {}",
        player.frame()
    );
    assert!(
        code(player.picture()).unwrap() > 0,
        "the picture kept moving"
    );
}

fn rms(samples: &[f32]) -> f32 {
    let n = samples.len().max(1) as f32;
    (samples.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
}
