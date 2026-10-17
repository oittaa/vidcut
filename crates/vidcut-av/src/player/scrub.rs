//! Audio scrubbing (spec §4.5, decision D42): a frame step, or a seek
//! while paused, plays that frame's sound.
//!
//! A frame's sound is the mix's samples of it (every lane, with clip gains,
//! faders, mute and solo as the plan has them, lane gains on top), forward
//! whichever way the step went, faded in and out over a few milliseconds so
//! it doesn't click. One plays at a time: the newest waits while one plays
//! out, and a newer one replaces it there, so held keys play frame after
//! frame and steps never pile up.
//!
//! The player does it on its audio thread while paused.

use super::plan::Plan;
use super::ring::Ring;
use crate::error::Result;
use crate::export::audio::AudioMix;

/// The fades at a frame's sound's ends.
const FADE_MS: u32 = 3;

/// A waiting sound goes to the device once this little of the one playing
/// is left there: frame after frame without a gap.
const NEXT_SECS: f64 = 0.002;

/// The sound of timeline frame `t`: its samples of the mix, faded in and
/// out.
pub(crate) fn snippet(mix: &mut AudioMix, plan: &Plan, t: i64) -> Result<Vec<f32>> {
    let t = plan.clamp(t);
    let (a, b) = (plan.sample(t), plan.sample(t + 1));
    mix.seek(a);
    let mut samples = mix.read((b - a) as usize)?;
    fade(
        &mut samples,
        usize::from(mix.channels()),
        (plan.rate() * FADE_MS / 1000) as usize,
    );
    Ok(samples)
}

/// Fades `samples` (interleaved) in and out over `n` sample frames each.
fn fade(samples: &mut [f32], channels: usize, n: usize) {
    let frames = samples.len() / channels.max(1);
    let n = n.min(frames / 2).max(1);
    for i in 0..n {
        let g = (i + 1) as f32 / n as f32;
        for c in 0..channels {
            samples[i * channels + c] *= g;
            samples[(frames - 1 - i) * channels + c] *= g;
        }
    }
}

/// The frame's sound waiting for the one playing to end.
#[derive(Default)]
pub(crate) struct Waiting {
    sound: Option<Vec<f32>>,
    /// The request it answers.
    pub served: Option<u64>,
}

impl Waiting {
    /// `sound` waits next, replacing one that was waiting.
    pub(crate) fn replace(&mut self, sound: Vec<f32>, request: u64) {
        self.sound = Some(sound);
        self.served = Some(request);
    }

    pub(crate) fn clear(&mut self) {
        self.sound = None;
    }

    pub(crate) fn is_some(&self) -> bool {
        self.sound.is_some()
    }

    /// Gives the waiting sound to the device once the one playing has (all
    /// but) played out; whether it did.
    pub(crate) fn offer(&mut self, ring: &Ring, rate: u32) -> bool {
        let Some(sound) = &self.sound else {
            return false;
        };
        let frames = (sound.len() / ring.channels()) as u64;
        let left = ring.queued() as f64 / f64::from(rate);
        if left > NEXT_SECS || ring.room() < frames {
            return false;
        }
        ring.push(sound);
        self.sound = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frames_sound_fades_in_and_out() {
        let mut s = vec![1.0f32; 2 * 100];
        fade(&mut s, 2, 10);
        assert!((s[0] - 0.1).abs() < 1e-6 && (s[1] - 0.1).abs() < 1e-6);
        assert_eq!(s[2 * 10], 1.0, "full in the middle");
        assert!((s[2 * 99] - 0.1).abs() < 1e-6, "out at the end");
    }

    #[test]
    fn a_newer_sound_replaces_a_waiting_one_never_a_playing_one() {
        let ring = Ring::new(1000, 1);
        let mut w = Waiting::default();
        w.replace(vec![1.0; 100], 1);
        assert!(w.offer(&ring, 1000), "nothing playing: at once");
        w.replace(vec![2.0; 100], 2);
        w.replace(vec![3.0; 100], 3);
        assert!(!w.offer(&ring, 1000), "the first still plays");
        let mut out = vec![0.0; 100];
        ring.pop(&mut out);
        assert_eq!(out[99], 1.0, "the first played whole");
        assert!(w.offer(&ring, 1000));
        ring.pop(&mut out);
        assert_eq!(out[0], 3.0, "the newest, not the one it replaced");
        assert!(!w.is_some());
    }
}
