//! The audio thread: the plan's lanes mixed, with their faders, into the
//! ring a tenth of a second ahead of the device, each run of samples marked
//! with where it is on the timeline.
//!
//! Backward the sound plays reversed: the mix is read forward a block at a
//! time, from further and further back, and each block reversed. At other
//! speeds, either way, it goes through [`Stretch`], which keeps the pitch
//! and says where each of its blocks came from: each is marked as it is
//! queued, so the clock follows what is heard at every speed. Paused,
//! steps play their frame's sound ([`super::scrub`]).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use super::clock::Marker;
use super::plan::Plan;
use super::scrub::{Waiting, snippet};
use super::{AudioSnap, Mode, Shared};
use crate::error::Result;
use crate::export::DECLICK_MS;
use crate::export::audio::AudioMix;
use crate::stretch::Stretch;

/// Sound kept queued ahead of the device.
const AHEAD_SECS: f64 = 0.1;

/// Sound mixed at a time.
const CHUNK_SECS: f64 = 0.01;

/// Room in the ring a chunk needs, with blocks the stretch gives at once.
const MIN_ROOM_SECS: f64 = 0.1;

/// Sound read at a time going backward, in seconds of play: each block
/// costs a seek of every lane's stream.
const BLOCK_SECS: f64 = 0.2;

/// Sound ready to queue: samples, and where they are as stream positions
/// (frames from the run's start, in the direction of play) when they need
/// a mark of their own.
struct Piece {
    samples: Vec<f32>,
    /// Stream position of the first sample, and stream frames per sample.
    mark: Option<(f64, f64)>,
}

/// What is being played.
struct Run {
    epoch: u64,
    speed: f64,
    /// The timeline sample at stream position 0: where the run (or its
    /// restart after an edit) began.
    origin: i64,
    /// The next timeline sample to mix; backward, the next to play, the mix
    /// read up to it.
    cursor: i64,
    /// Other speeds than ±1.
    stretch: Option<Stretch>,
    pending: VecDeque<Piece>,
    /// All of the run was mixed (and the stretch emptied).
    exhausted: bool,
    /// The end (or the start, backward) was reached and marked.
    ended: bool,
    /// When the thread took the play request, and whether any of its sound
    /// is queued yet, for `VIDCUT_DEBUG`.
    taken: std::time::Instant,
    queued: bool,
    /// Each lane's peaks in the sound mixed last, `channels` a lane, for
    /// the meters ([`super::Levels`]).
    lanes: Vec<f32>,
}

impl Run {
    fn dir(&self) -> f64 {
        self.speed.signum()
    }
}

struct Producer {
    shared: Arc<Shared>,
    mix: AudioMix,
    plan: Arc<Plan>,
    plan_gen: u64,
    run: Option<Run>,
    trims_gen: Option<u64>,
    muted: bool,
    /// Paused: the frame's sound waiting for the one playing.
    waiting: Waiting,
}

pub(crate) fn run(shared: Arc<Shared>) {
    let Some(first) = shared.audio_snap() else {
        return;
    };
    let plan = &first.plan.plan;
    let mut p = Producer {
        mix: AudioMix::new(&plan.audio, plan.sample_rate, plan.channels, DECLICK_MS),
        plan: first.plan.clone(),
        plan_gen: first.plan_gen,
        run: None,
        trims_gen: None,
        muted: false,
        waiting: Waiting::default(),
        shared,
    };
    while let Some(snap) = p.shared.audio_snap() {
        p.sync(&snap);
        // A frame's sound waiting: look again as the one playing ends.
        let wait = if p.waiting.is_some() {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(5)
        };
        match p.produce() {
            Ok(true) => {}
            Ok(false) => p.shared.wait_any(wait),
            Err(e) => {
                p.shared.fail(e.to_string());
                p.shared.wait_any(wait);
            }
        }
    }
}

impl Producer {
    /// Follows what is wanted: a new plan, a play, a pause, faders.
    fn sync(&mut self, snap: &AudioSnap) {
        if snap.plan_gen != self.plan_gen {
            // The sound goes on from where the device reads now, in the
            // new plan.
            let at = match &self.run {
                Some(r) => self.read_sample(r.epoch).unwrap_or(r.cursor),
                None => 0,
            };
            self.plan = snap.plan.clone();
            self.plan_gen = snap.plan_gen;
            self.mix.replan(&self.plan.plan.audio, at);
            self.trims_gen = None;
            if self.run.is_some() {
                self.restart(at);
            }
        }
        match snap.mode {
            Mode::Paused { .. } => {
                if self.run.take().is_some() {
                    self.shared.ring.discard_all();
                }
            }
            Mode::Playing { from, speed, epoch } => {
                if self.run.as_ref().map(|r| r.epoch) != Some(epoch) {
                    self.waiting.clear();
                    self.start(from, speed, epoch);
                }
            }
        }
        if self.trims_gen != Some(snap.trims_gen) {
            for lane in 0..self.mix.lanes() {
                let db = snap.trims.get(lane).copied().unwrap_or(0.0);
                let _ = self.mix.set_lane_db(lane, db);
            }
            self.muted = snap.muted;
            self.trims_gen = Some(snap.trims_gen);
        }
        // A step or a paused seek: its frame's sound, unless muted.
        match snap.scrub {
            None => self.waiting.clear(),
            Some((t, n)) if self.waiting.served != Some(n) => {
                self.waiting.served = Some(n);
                if self.run.is_none() && !self.muted {
                    match snippet(&mut self.mix, &self.plan, t) {
                        Ok(sound) => self.waiting.replace(sound, n),
                        Err(e) => self.shared.fail(e.to_string()),
                    }
                }
            }
            Some(_) => {}
        }
    }

    /// The timeline sample the device reads next in play request `epoch`.
    fn read_sample(&self, epoch: u64) -> Option<i64> {
        let counter = self.shared.ring.read_position();
        let s = self.shared.markers.sample_at(counter, epoch)?;
        Some((s.round() as i64).clamp(0, self.plan.samples))
    }

    /// Starts play request `epoch` from sample `from`.
    fn start(&mut self, from: i64, speed: f64, epoch: u64) {
        crate::debug!(
            "audio",
            "play request {epoch}: from sample {from} at {speed}x"
        );
        self.run = Some(Run {
            epoch,
            speed,
            origin: from,
            cursor: from,
            stretch: None,
            pending: VecDeque::new(),
            exhausted: false,
            ended: false,
            taken: std::time::Instant::now(),
            queued: false,
            lanes: Vec::new(),
        });
        self.restart(from);
    }

    /// Mixes the current run from sample `at` on, dropping what was queued.
    fn restart(&mut self, at: i64) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let ring = &self.shared.ring;
        ring.discard_all();
        // What was queued won't be heard: neither are its levels.
        self.shared.forget_levels();
        if run.speed > 0.0 {
            self.mix.seek(at);
        }
        run.origin = at;
        run.cursor = at;
        run.ended = false;
        run.exhausted = false;
        run.pending.clear();
        run.stretch = (run.speed.abs() != 1.0).then(|| {
            Stretch::new(
                run.speed.abs(),
                self.plan.rate(),
                usize::from(self.plan.plan.channels),
            )
        });
        // At ±1 a stream frame is a timeline sample: one mark does. The
        // stretch marks each of its blocks.
        if run.stretch.is_none() {
            self.shared.markers.push(Marker {
                counter: ring.written(),
                sample: at,
                speed: run.dir(),
                epoch: run.epoch,
            });
        }
    }

    /// Queues the next sound when the device's queue is short; whether
    /// there was any.
    fn produce(&mut self) -> Result<bool> {
        let ring = self.shared.ring.clone();
        let rate = self.plan.rate();
        let Some(run) = self.run.as_mut() else {
            return Ok(self.waiting.offer(&ring, rate));
        };
        // Room for what comes, however much the stretch gives at once: a
        // device that stopped pulling leaves none, and the thread waits for
        // it rather than for room.
        let short = ring.queued() as f64 >= AHEAD_SECS * f64::from(rate);
        if run.ended || short || (ring.room() as f64) < MIN_ROOM_SECS * f64::from(rate) {
            return Ok(false);
        }
        // The stretch gives nothing until it holds a fragment's worth.
        let mut failed = None;
        while self
            .run
            .as_ref()
            .is_some_and(|r| r.pending.is_empty() && !r.exhausted)
        {
            if let Some(e) = self.refill() {
                failed = Some(e);
            }
        }
        let Some(run) = self.run.as_mut() else {
            return Ok(false);
        };
        let Some(mut piece) = run.pending.pop_front() else {
            // Nothing more: the end (or the start) heard last.
            let edge = if run.speed > 0.0 {
                self.plan.samples
            } else {
                0
            };
            self.shared.markers.push(Marker {
                counter: ring.written(),
                sample: edge,
                speed: 0.0,
                epoch: run.epoch,
            });
            run.ended = true;
            return Ok(true);
        };
        let channels = ring.channels();
        if let Some((at, per)) = piece.mark {
            let dir = run.dir();
            self.shared.markers.push(Marker {
                counter: ring.written(),
                sample: (run.origin as f64 + dir * at).round() as i64,
                speed: dir * per,
                epoch: run.epoch,
            });
        } else {
            // Unmarked sound goes a chunk at a time.
            let chunk = (CHUNK_SECS * f64::from(rate)).round() as usize * channels;
            if piece.samples.len() > chunk {
                let rest = piece.samples.split_off(chunk);
                run.pending.push_front(Piece {
                    samples: rest,
                    mark: None,
                });
            }
        }
        if self.muted {
            piece.samples.fill(0.0);
        }
        let at = ring.written();
        let pushed = ring.push(&piece.samples) * channels;
        let mut mix = vec![0.0f32; channels];
        for frame in piece.samples.chunks(channels) {
            for (m, s) in mix.iter_mut().zip(frame) {
                *m = m.max(s.abs());
            }
        }
        self.shared.level(super::Level {
            at,
            frames: (pushed / channels.max(1)) as u64,
            mix,
            lanes: run.lanes.clone(),
        });
        debug_assert_eq!(pushed, piece.samples.len(), "no room for a chunk");
        if !run.queued {
            run.queued = true;
            crate::debug!(
                "audio",
                "play request {}: its first sound queued {:.1} ms after the audio thread took it",
                run.epoch,
                run.taken.elapsed().as_secs_f64() * 1000.0
            );
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(true),
        }
    }

    /// Mixes more of the run into pieces: forward the next chunk, backward
    /// the next block reversed, through the stretch at other speeds; at the
    /// end what the stretch still holds. A broken file is silent, the clock
    /// going on, and its error is given back.
    fn refill(&mut self) -> Option<crate::error::Error> {
        let rate = f64::from(self.plan.rate());
        let total = self.plan.samples;
        let run = self.run.as_mut()?;
        let channels = usize::from(self.mix.channels());
        let read =
            |mix: &mut AudioMix, n: usize, failed: &mut Option<_>, lanes: &mut Vec<f32>| match mix
                .read_metered(n)
            {
                Ok((s, peaks)) => {
                    *lanes = peaks;
                    s
                }
                Err(e) => {
                    *failed = Some(e);
                    vec![0.0; n * channels]
                }
            };
        let mut failed = None;
        let mixed = if run.speed > 0.0 {
            let n = ((CHUNK_SECS * rate * run.speed).round() as i64)
                .max(1)
                .min(total - run.cursor);
            if n <= 0 {
                None
            } else {
                run.cursor += n;
                Some(read(&mut self.mix, n as usize, &mut failed, &mut run.lanes))
            }
        } else if run.cursor < 0 {
            None
        } else {
            // Samples `lo..=cursor`, read forward, reversed.
            let n = ((BLOCK_SECS * rate * run.speed.abs()).round() as i64).max(1);
            let lo = (run.cursor + 1 - n).max(0);
            self.mix.seek(lo);
            let n = (run.cursor + 1 - lo) as usize;
            let mut s = read(&mut self.mix, n, &mut failed, &mut run.lanes);
            reverse(&mut s, channels);
            run.cursor = lo - 1;
            Some(s)
        };
        match (mixed, run.stretch.as_mut()) {
            (Some(s), None) => run.pending.push_back(Piece {
                samples: s,
                mark: None,
            }),
            (Some(s), Some(stretch)) => {
                run.pending
                    .extend(stretch.push(&s).into_iter().map(|b| Piece {
                        samples: b.samples,
                        mark: Some((b.at, b.rate)),
                    }))
            }
            (None, Some(stretch)) => {
                run.pending
                    .extend(stretch.finish().into_iter().map(|b| Piece {
                        samples: b.samples,
                        mark: Some((b.at, b.rate)),
                    }));
                run.exhausted = true;
            }
            (None, None) => run.exhausted = true,
        }
        failed
    }
}

/// Reverses interleaved samples a sample frame at a time: the channels of
/// each frame stay in their order.
fn reverse(samples: &mut [f32], channels: usize) {
    let frames = samples.len() / channels.max(1);
    for i in 0..frames / 2 {
        let j = frames - 1 - i;
        for c in 0..channels {
            samples.swap(i * channels + c, j * channels + c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::reverse;

    #[test]
    fn reversing_keeps_each_frames_channels_in_order() {
        let mut s = vec![1.0, -1.0, 2.0, -2.0, 3.0, -3.0];
        reverse(&mut s, 2);
        assert_eq!(s, vec![3.0, -3.0, 2.0, -2.0, 1.0, -1.0]);
    }
}
