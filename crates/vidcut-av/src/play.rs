//! Playback of a render plan for the viewer.
//!
//! The picture is the frame the sampling rule picks (the same rule as
//! export). Its colors are the software painter's for now; a later path
//! keeps the frame on the GPU, and the colors may then differ. The clock is
//! the mixed audio: [`Player::pull`] returns the next samples and, when
//! those samples cross a frame, the picture that goes with them.
//!
//! Reverse is stepped seeks. Forward speeds other than 1 repeat or skip
//! samples (nearest neighbour) so the picture still advances with the audio.

use std::sync::Arc;

use vidcut_core::render::{RenderPlan, VideoPiece};
use vidcut_core::time::Rational;

use crate::error::{Error, Result};
use crate::export::audio::AudioMix;
use crate::export::renderer::StillRenderer;
use crate::thumbs::Image;

/// Samples returned by [`Player::pull`], and the frame those samples moved
/// onto when they did.
pub struct Pull {
    pub samples: Vec<f32>,
    pub frame: Option<i64>,
    /// This pull consumed the plan. The picture stays on the last frame.
    pub ended: bool,
}

/// Plays one render plan. Software frames, held by the still renderer so
/// stepping does not decode a kept frame again.
pub struct Player {
    plan: RenderPlan,
    /// Timeline frame now showing.
    frame: i64,
    /// Plan samples already consumed (the start of `frame`'s span, after a seek).
    samples: i64,
    /// Fractional plan sample left over when `speed` is not 1.
    residual: f64,
    speed: f64,
    picture: Arc<Image>,
    pictures: StillRenderer,
    audio: AudioMix,
}

impl Player {
    /// Paused at the plan's first frame, with its picture decoded.
    pub fn new(plan: RenderPlan) -> Result<Self> {
        let audio = AudioMix::new(&plan.audio, plan.sample_rate, plan.channels, 0);
        let mut player = Self {
            frame: plan.range.0,
            samples: 0,
            residual: 0.0,
            speed: 0.0,
            picture: Arc::new(Image {
                width: plan.width.max(1),
                height: plan.height.max(1),
                rgba: vec![0; plan.width.max(1) as usize * plan.height.max(1) as usize * 4],
            }),
            pictures: StillRenderer::with_budget(crate::export::renderer::DEFAULT_BUDGET),
            audio,
            plan,
        };
        player.show(player.frame)?;
        Ok(player)
    }

    pub fn frame(&self) -> i64 {
        self.frame
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// The picture of [`frame`](Self::frame).
    pub fn picture(&self) -> &Image {
        &self.picture
    }

    /// Forward speed. `0` pauses. Reverse is [`step`](Self::step) with a
    /// negative count: a negative speed is rejected.
    pub fn set_speed(&mut self, speed: f64) -> Result<()> {
        if speed < 0.0 {
            return Err(Error::Invalid(
                "reverse playback steps back; it does not play at a negative speed".into(),
            ));
        }
        self.speed = speed;
        self.residual = 0.0;
        Ok(())
    }

    /// Extra gain on an audio lane, in dB, on top of the plan. Heard on the
    /// next [`pull`](Self::pull).
    pub fn set_lane_db(&mut self, lane: usize, db: f64) -> Result<()> {
        self.audio.set_lane_db(lane, db)
    }

    /// Shows `frame` (clamped to the plan) and continues the audio from there.
    pub fn seek(&mut self, frame: i64) -> Result<()> {
        let frame = frame.clamp(self.plan.range.0, self.end_frame());
        self.samples = self
            .plan
            .fps
            .frame_to_sample(frame - self.plan.range.0, self.plan.sample_rate);
        self.residual = 0.0;
        self.audio.seek(self.samples);
        self.show(frame)
    }

    /// Seeks by `frames` (negative steps back).
    pub fn step(&mut self, frames: i64) -> Result<()> {
        self.seek(self.frame.saturating_add(frames))
    }

    /// The next `samples` of the mix, at the output rate.
    ///
    /// At speed 1 this is that many plan samples, and `frame` in the result
    /// is set when they cross onto a later frame. While paused, the audio
    /// does not advance.
    pub fn pull(&mut self, samples: usize) -> Result<Pull> {
        let channels = usize::from(self.plan.channels.max(1));
        if self.speed == 0.0 || samples == 0 {
            return Ok(Pull {
                samples: vec![0.0; samples * channels],
                frame: None,
                ended: false,
            });
        }
        let span = self.residual + samples as f64 * self.speed;
        let take = span.floor() as usize;
        self.residual = span - take as f64;
        let read = self.audio.read(take)?;
        self.samples += take as i64;
        let out = fit(&read, channels, samples);
        let rel = frame_of(self.plan.fps, self.samples, self.plan.sample_rate);
        let frame = self.plan.range.0 + rel.min(self.plan.frames.saturating_sub(1).max(0));
        let frame = frame.min(self.end_frame());
        let crossed = if frame != self.frame {
            self.show(frame)?;
            Some(frame)
        } else {
            None
        };
        let total = self
            .plan
            .fps
            .frame_to_sample(self.plan.frames.max(0), self.plan.sample_rate);
        let ended = self.plan.frames > 0 && self.samples >= total;
        Ok(Pull {
            samples: out,
            frame: crossed,
            ended,
        })
    }

    fn end_frame(&self) -> i64 {
        self.plan.range.0 + self.plan.frames.saturating_sub(1).max(0)
    }

    fn show(&mut self, frame: i64) -> Result<()> {
        let (piece, floor) = piece_at(&self.plan, frame)?;
        self.picture = self.pictures.rgba(
            &piece,
            floor,
            self.plan.fps,
            self.plan.width,
            self.plan.height,
        )?;
        self.frame = frame;
        Ok(())
    }
}

/// The one-frame piece at timeline `frame`, and the first source position of
/// the run it belongs to (nothing before that is worth keeping).
fn piece_at(plan: &RenderPlan, frame: i64) -> Result<(VideoPiece, i64)> {
    let mut at = frame - plan.range.0;
    if at < 0 || (plan.frames > 0 && at >= plan.frames) {
        return Err(Error::Invalid(format!("frame {frame} is outside the plan")));
    }
    for piece in &plan.video {
        let n = piece.frames();
        if at < n {
            return Ok(match piece.clone() {
                VideoPiece::Source {
                    source_frame,
                    image,
                    media,
                    path,
                    origin,
                    ..
                } => (
                    VideoPiece::Source {
                        media,
                        path,
                        origin,
                        source_frame: source_frame + at,
                        frames: 1,
                        image,
                    },
                    source_frame,
                ),
                VideoPiece::Black { .. } => (VideoPiece::Black { frames: 1 }, 0),
            });
        }
        at -= n;
    }
    Ok((VideoPiece::Black { frames: 1 }, 0))
}

/// The frame whose sample span contains `sample` (0 when `sample` is at or
/// before the start).
fn frame_of(fps: Rational, sample: i64, rate: u32) -> i64 {
    if sample <= 0 || rate == 0 {
        return 0;
    }
    let mut frame = (i128::from(sample) * i128::from(fps.num())
        / (i128::from(rate) * i128::from(fps.den()))) as i64;
    if frame < 0 {
        frame = 0;
    }
    while frame > 0 && fps.frame_to_sample(frame, rate) > sample {
        frame -= 1;
    }
    while fps.frame_to_sample(frame + 1, rate) <= sample {
        frame += 1;
    }
    frame
}

/// `out_n` interleaved frames from `src`, nearest neighbour. Equal lengths
/// are copied.
fn fit(src: &[f32], channels: usize, out_n: usize) -> Vec<f32> {
    let channels = channels.max(1);
    let src_n = src.len() / channels;
    if src_n == out_n {
        return src.to_vec();
    }
    if src_n == 0 || out_n == 0 {
        return vec![0.0; out_n * channels];
    }
    let mut out = vec![0.0; out_n * channels];
    for i in 0..out_n {
        let j = (i as u64 * src_n as u64 / out_n as u64) as usize;
        let j = j.min(src_n - 1);
        let from = j * channels;
        let to = i * channels;
        out[to..to + channels].copy_from_slice(&src[from..from + channels]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_past_the_plan_reports_the_end() {
        use vidcut_core::render::{RenderPlan, VideoPiece};

        let fps = Rational::new(30, 1).unwrap();
        let plan = RenderPlan {
            fps,
            width: 16,
            height: 16,
            sample_rate: 48_000,
            channels: 2,
            frames: 2,
            video: vec![VideoPiece::Black { frames: 2 }],
            audio: Vec::new(),
            range: (0, 2),
            name: "end".into(),
        };
        let mut player = Player::new(plan).unwrap();
        player.set_speed(1.0).unwrap();
        let early = player.pull(100).unwrap();
        assert!(!early.ended, "a short pull finished the plan");
        assert_eq!(player.frame(), 0);
        let rest = player.pull(48_000).unwrap();
        assert!(rest.ended, "the plan was not finished");
        assert_eq!(player.frame(), 1, "the picture stays on the last frame");
    }

    #[test]
    fn a_sample_belongs_to_the_frame_that_starts_at_or_before_it() {
        let fps = Rational::new(30, 1).unwrap();
        let rate = 48_000;
        assert_eq!(frame_of(fps, 0, rate), 0);
        assert_eq!(frame_of(fps, fps.frame_to_sample(1, rate) - 1, rate), 0);
        assert_eq!(frame_of(fps, fps.frame_to_sample(1, rate), rate), 1);
        assert_eq!(frame_of(fps, fps.frame_to_sample(30, rate), rate), 30);
        assert_eq!(frame_of(fps, -5, rate), 0);
    }
}
