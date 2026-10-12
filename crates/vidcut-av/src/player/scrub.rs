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
//! The player does it on its audio thread while paused; [`Scrubber`] does
//! it alone, for pictures shown by something else.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use vidcut_core::render::RenderPlan;

use super::clock::Heard;
use super::output::{AudioOutput, AudioSource};
use super::plan::Plan;
use super::ring::Ring;
use crate::error::{Error, Result};
use crate::export::DECLICK_MS;
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

/// Plays the sound of single frames of a plan, as frame steps and
/// scrubbing do, without a player: for a viewer whose pictures come from
/// elsewhere. Requests return at once; mixing runs on a thread of its own,
/// the sound goes to the output. Dropping it stops both.
pub struct Scrubber {
    shared: Arc<ScrubShared>,
    thread: Option<JoinHandle<()>>,
    output: Mutex<Option<Box<dyn AudioOutput>>>,
}

struct ScrubShared {
    state: Mutex<ScrubState>,
    changed: Condvar,
    ring: Arc<Ring>,
}

struct ScrubState {
    plan: Option<Arc<Plan>>,
    plan_gen: u64,
    /// The frame asked for, numbered.
    frame: Option<(i64, u64)>,
    trims: Vec<f64>,
    trims_gen: u64,
    closed: bool,
    error: Option<String>,
}

impl Scrubber {
    /// A scrubber whose sound goes to `output`, opened at `rate` Hz in
    /// `channels` channels: plans given to it must have them.
    pub fn new(rate: u32, channels: u16, mut output: Box<dyn AudioOutput>) -> Result<Self> {
        crate::init();
        let ring = Arc::new(Ring::new(rate as usize, channels));
        let heard = Arc::new(Heard::new(Instant::now(), rate));
        output.start(AudioSource::new(ring.clone(), heard, rate, channels))?;
        let shared = Arc::new(ScrubShared {
            state: Mutex::new(ScrubState {
                plan: None,
                plan_gen: 0,
                frame: None,
                trims: Vec::new(),
                trims_gen: 0,
                closed: false,
                error: None,
            }),
            changed: Condvar::new(),
            ring,
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("vidcut-scrub".into())
            .spawn(move || run(s, rate, channels))
            .map_err(Error::Io)?;
        Ok(Self {
            shared,
            thread: Some(thread),
            output: Mutex::new(Some(output)),
        })
    }

    fn state(&self) -> MutexGuard<'_, ScrubState> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Plays the sound of `plan` from now on: an edit. Lane gains start
    /// over.
    pub fn set_plan(&self, plan: RenderPlan) {
        let mut s = self.state();
        s.plan = Some(Arc::new(Plan::new(plan)));
        s.plan_gen += 1;
        s.trims.clear();
        s.trims_gen += 1;
        drop(s);
        self.shared.changed.notify_all();
    }

    /// Plays the sound of timeline frame `frame` of the plan: at once when
    /// nothing plays, else when the sound playing ends, replacing a frame
    /// still waiting.
    pub fn play(&self, frame: i64) {
        let mut s = self.state();
        let n = s.frame.map_or(0, |(_, n)| n) + 1;
        s.frame = Some((frame, n));
        drop(s);
        self.shared.changed.notify_all();
    }

    /// A gain on top of audio lane `lane`'s own, in dB (a fader being
    /// moved).
    pub fn set_lane_gain(&self, lane: usize, db: f64) {
        let mut s = self.state();
        if s.trims.len() <= lane {
            s.trims.resize(lane + 1, 0.0);
        }
        s.trims[lane] = db;
        s.trims_gen += 1;
        drop(s);
        self.shared.changed.notify_all();
    }

    /// The newest failure to mix a frame's sound.
    pub fn error(&self) -> Option<String> {
        self.state().error.clone()
    }
}

impl Drop for Scrubber {
    fn drop(&mut self) {
        self.state().closed = true;
        self.shared.changed.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.output.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// The scrubber's thread: mixes each frame asked for and hands it to the
/// device.
fn run(shared: Arc<ScrubShared>, rate: u32, channels: u16) {
    let mut mix: Option<(u64, AudioMix)> = None;
    let mut trims_gen = None;
    let mut waiting = Waiting::default();
    loop {
        let (plan, plan_gen, frame, trims, gen_now) = {
            let s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.closed {
                return;
            }
            (
                s.plan.clone(),
                s.plan_gen,
                s.frame,
                s.trims.clone(),
                s.trims_gen,
            )
        };
        if let Some(plan) = &plan {
            if mix.as_ref().is_none_or(|(g, _)| *g != plan_gen) {
                let lanes = &plan.plan.audio;
                match mix.as_mut() {
                    Some((g, m)) => {
                        m.replan(lanes, 0);
                        *g = plan_gen;
                    }
                    None => {
                        mix = Some((plan_gen, AudioMix::new(lanes, rate, channels, DECLICK_MS)))
                    }
                }
                trims_gen = None;
            }
            let (_, m) = mix.as_mut().expect("made");
            if trims_gen != Some(gen_now) {
                for lane in 0..m.lanes() {
                    let _ = m.set_lane_db(lane, trims.get(lane).copied().unwrap_or(0.0));
                }
                trims_gen = Some(gen_now);
            }
            if let Some((t, n)) = frame
                && waiting.served != Some(n)
            {
                match snippet(m, plan, t) {
                    Ok(sound) => waiting.replace(sound, n),
                    Err(e) => {
                        waiting.served = Some(n);
                        shared.state.lock().unwrap_or_else(|e| e.into_inner()).error =
                            Some(e.to_string());
                    }
                }
            }
        }
        waiting.offer(&shared.ring, rate);
        // Waiting sound: look again as the one playing ends.
        let wait = if waiting.is_some() {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(50)
        };
        let s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.closed && s.frame == frame && s.plan_gen == plan_gen && s.trims_gen == gen_now {
            let _ = shared.changed.wait_timeout(s, wait);
        }
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
