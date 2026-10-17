#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use vidcut_av::decode::read_barcode;
use vidcut_av::fixture::{self, FixtureSpec};
use vidcut_av::player::{ManualOutput, Picture, Player};
use vidcut_core::render::{AudioPiece, RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

pub const W: u32 = 320;
pub const H: u32 = 180;
pub const RATE: u32 = 48_000;

pub fn fps() -> Rational {
    Rational::new(30, 1).unwrap()
}

#[derive(Clone)]
pub struct Src {
    pub path: PathBuf,
    pub origin: f64,
    pub media: String,
}

pub fn src(media: &str, spec: FixtureSpec) -> Src {
    let path = fixture::ensure(&spec).unwrap();
    let info = vidcut_av::probe::probe(&path, None).unwrap();
    let origin = info
        .video
        .as_ref()
        .and_then(|v| v.first_frame_time)
        .unwrap_or(0.0);
    Src {
        path,
        origin,
        media: media.into(),
    }
}

pub struct Clip<'a> {
    pub src: Option<&'a Src>,
    pub from: i64,
    pub frames: i64,
}

pub fn clip(src: &Src, from: i64, frames: i64) -> Clip<'_> {
    Clip {
        src: Some(src),
        from,
        frames,
    }
}

pub fn gap<'a>(frames: i64) -> Clip<'a> {
    Clip {
        src: None,
        from: 0,
        frames,
    }
}

pub fn plan(clips: &[Clip], start: i64) -> RenderPlan {
    let fps = fps();
    let sample = |t: i64| fps.frame_to_sample(t, RATE);
    let (mut video, mut lane) = (Vec::new(), Vec::new());
    let mut t = 0;
    for c in clips {
        let samples = sample(t + c.frames) - sample(t);
        match c.src {
            Some(s) => {
                video.push(VideoPiece::Source {
                    pace: Default::default(),
                    media: s.media.clone(),
                    path: s.path.clone(),
                    origin: s.origin,
                    source_frame: c.from,
                    frames: c.frames,
                    image: None,
                });
                lane.push(AudioPiece::Source {
                    speed: 1.0,
                    media: s.media.clone(),
                    path: s.path.clone(),
                    stream: 0,
                    origin: s.origin,
                    source_sample: sample(c.from),
                    samples,
                    gain_db: 0.0,
                    fade: None,
                });
            }
            None => {
                video.push(VideoPiece::Black { frames: c.frames });
                lane.push(AudioPiece::Silence { samples });
            }
        }
        t += c.frames;
    }
    RenderPlan {
        fps,
        width: W,
        height: H,
        sample_rate: RATE,
        channels: 2,
        frames: t,
        video,
        audio: vec![lane],
        range: (start, start + t),
        name: "test".into(),
    }
}

pub fn piece_at(plan: &RenderPlan, t: i64) -> VideoPiece {
    let mut k = t - plan.range.0;
    for p in &plan.video {
        if k < p.frames() {
            return p.part(k, 1);
        }
        k -= p.frames();
    }
    panic!("frame {t} is outside the plan");
}

pub fn rendered(plan: &RenderPlan, t: i64) -> Option<u32> {
    let img = vidcut_av::export::still::rgba(&piece_at(plan, t), plan.fps, W, H).unwrap();
    code_rgba(&img.rgba, img.width, img.height)
}

pub fn code_rgba(rgba: &[u8], w: u32, h: u32) -> Option<u32> {
    use vidcut_av::ffmpeg::{format::Pixel, frame};
    let mut f = frame::Video::new(Pixel::RGBA, w, h);
    let stride = f.stride(0);
    let row = w as usize * 4;
    for y in 0..h as usize {
        f.data_mut(0)[y * stride..y * stride + row].copy_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    read_barcode(&f).unwrap()
}

pub fn code(p: &Picture) -> Option<u32> {
    read_barcode(p.frame()).unwrap()
}

pub fn shown_at(player: &Player, t: i64) -> Option<u32> {
    assert!(
        player.wait_ready(t, Duration::from_secs(10)),
        "frame {t} never decoded: {:?}",
        player.status()
    );
    let shown = player.present(Instant::now()).expect("a picture");
    assert_eq!(shown.frame, t, "the viewer is at another frame");
    assert!(shown.exact, "frame {t}'s picture isn't shown");
    code(&shown.picture)
}

pub fn pull_full(out: &ManualOutput, frames: usize) -> Vec<f32> {
    let (mut queued, mut since) = (out.queued(), Instant::now());
    while queued < frames as u64 && since.elapsed() < Duration::from_millis(300) {
        std::thread::sleep(Duration::from_millis(1));
        let now = out.queued();
        if now != queued {
            (queued, since) = (now, Instant::now());
        }
    }
    let length = Duration::from_secs_f64(frames as f64 / f64::from(RATE));
    out.pull(frames, Instant::now() - length)
}

pub fn left(stereo: &[f32]) -> Vec<f32> {
    stereo.chunks(2).map(|c| c[0]).collect()
}

pub fn burst_ends(mono: &[f32], threshold: f32, min_gap: usize) -> Vec<usize> {
    let reversed: Vec<f32> = mono.iter().rev().copied().collect();
    let mut ends: Vec<usize> = vidcut_av::beep::onsets(&reversed, threshold, min_gap)
        .into_iter()
        .map(|i| mono.len() - 1 - i)
        .collect();
    ends.reverse();
    ends
}
