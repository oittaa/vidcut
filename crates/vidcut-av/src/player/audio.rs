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

const SILENT_WAIT: Duration = Duration::from_secs(1);

const AHEAD_SECS: f64 = 0.1;

const CHUNK_SECS: f64 = 0.01;

const MIN_ROOM_SECS: f64 = 0.1;

const BLOCK_SECS: f64 = 0.2;

struct Piece {
    samples: Vec<f32>,
    mark: Option<(f64, f64)>,
    lanes: Vec<f32>,
}

struct Run {
    epoch: u64,
    speed: f64,
    origin: i64,
    cursor: i64,
    fed: i64,
    base: (f64, i64),
    wraps: VecDeque<(f64, i64)>,
    stretch: Option<Stretch>,
    pending: VecDeque<Piece>,
    exhausted: bool,
    ended: bool,
    taken: std::time::Instant,
    queued: bool,
    metered: VecDeque<(f64, f64, Vec<f32>)>,
}

impl Run {
    fn dir(&self) -> f64 {
        self.speed.signum()
    }

    fn sample_at(&mut self, at: f64) -> i64 {
        while self.wraps.front().is_some_and(|&(w, _)| w <= at) {
            self.base = self.wraps.pop_front().expect("just seen");
        }
        (self.base.1 as f64 + self.dir() * (at - self.base.0)).round() as i64
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
    looping: Option<(i64, i64)>,
    waiting: Waiting,
}

pub(crate) fn run(shared: Arc<Shared>) {
    let Some(first) = shared.audio_snap() else {
        return;
    };
    let plan = &first.plan.plan;
    let mut p = Producer {
        mix: AudioMix::new(&plan.audio, plan.sample_rate, plan.channels, DECLICK_MS)
            .ducked(&plan.ducking),
        plan: first.plan.clone(),
        plan_gen: first.plan_gen,
        run: None,
        trims_gen: None,
        muted: false,
        looping: None,
        waiting: Waiting::default(),
        shared,
    };
    while let Some(snap) = p.shared.audio_snap() {
        p.sync(&snap);
        let wait = if p.waiting.is_some() {
            Duration::from_millis(1)
        } else if p.run.is_some() {
            Duration::from_millis(5)
        } else {
            SILENT_WAIT
        };
        match p.produce() {
            Ok(true) => {}
            Ok(false) => p.shared.wait_changes(snap.changes, wait),
            Err(e) => {
                p.shared.fail(e.to_string());
                p.shared.wait_changes(snap.changes, wait);
            }
        }
    }
}

impl Producer {
    fn sync(&mut self, snap: &AudioSnap) {
        if snap.plan_gen != self.plan_gen {
            let at = match &self.run {
                Some(r) => self.read_sample(r.epoch).unwrap_or(r.cursor),
                None => 0,
            };
            self.plan = snap.plan.clone();
            self.plan_gen = snap.plan_gen;
            self.mix
                .replan(&self.plan.plan.audio, &self.plan.plan.ducking, at);
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
        let plan = &self.plan;
        let looping = snap
            .looping
            .map(|(a, b)| {
                let (first, end) = (plan.first(), plan.first() + plan.plan.frames);
                (
                    plan.sample(a.clamp(first, end)),
                    plan.sample(b.clamp(first, end)),
                )
            })
            .filter(|(a, b)| b > a);
        let resume = looping.is_some() && self.looping.is_none();
        self.looping = looping;
        if resume && let Some(at) = self.run.as_ref().filter(|r| r.exhausted).map(|r| r.cursor) {
            self.restart(at);
        }
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

    fn read_sample(&self, epoch: u64) -> Option<i64> {
        let counter = self.shared.ring.read_position();
        let s = self.shared.markers.sample_at(counter, epoch)?;
        Some((s.round() as i64).clamp(0, self.plan.samples))
    }

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
            fed: 0,
            base: (0.0, from),
            wraps: VecDeque::new(),
            stretch: None,
            pending: VecDeque::new(),
            exhausted: false,
            ended: false,
            taken: std::time::Instant::now(),
            queued: false,
            metered: VecDeque::new(),
        });
        self.restart(from);
    }

    fn restart(&mut self, at: i64) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let ring = &self.shared.ring;
        ring.discard_all();
        self.shared.forget_levels();
        if run.speed > 0.0 {
            self.mix.seek(at);
        }
        run.origin = at;
        run.cursor = at;
        run.fed = 0;
        run.base = (0.0, at);
        run.wraps.clear();
        run.ended = false;
        run.exhausted = false;
        run.pending.clear();
        run.metered.clear();
        run.stretch = (run.speed.abs() != 1.0).then(|| {
            Stretch::new(
                run.speed.abs(),
                self.plan.rate(),
                usize::from(self.plan.plan.channels),
            )
        });
        if run.stretch.is_none() {
            self.shared.markers.push(Marker {
                counter: ring.written(),
                sample: at,
                speed: run.dir(),
                epoch: run.epoch,
            });
        }
    }

    fn produce(&mut self) -> Result<bool> {
        let ring = self.shared.ring.clone();
        let rate = self.plan.rate();
        let Some(run) = self.run.as_mut() else {
            return Ok(self.waiting.offer(&ring, rate));
        };
        let short = ring.queued() as f64 >= AHEAD_SECS * f64::from(rate);
        if run.ended || short || (ring.room() as f64) < MIN_ROOM_SECS * f64::from(rate) {
            return Ok(false);
        }
        let mut failed = None;
        while self
            .run
            .as_ref()
            .is_some_and(|r| r.pending.len() < 2 && !r.exhausted)
        {
            if let Some(e) = self.refill() {
                failed = Some(e);
            }
        }
        let Some(run) = self.run.as_mut() else {
            return Ok(false);
        };
        let Some(mut piece) = run.pending.pop_front() else {
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
                sample: run.sample_at(at),
                speed: dir * per,
                epoch: run.epoch,
            });
        } else {
            let chunk = (CHUNK_SECS * f64::from(rate)).round() as usize * channels;
            if piece.samples.len() > chunk {
                let rest = piece.samples.split_off(chunk);
                run.pending.push_front(Piece {
                    samples: rest,
                    mark: None,
                    lanes: piece.lanes.clone(),
                });
            }
        }
        let mut mix = vec![0.0f32; channels];
        for frame in piece.samples.chunks(channels) {
            for (m, s) in mix.iter_mut().zip(frame) {
                *m = m.max(s.abs());
            }
        }
        if self.muted {
            piece.samples.fill(0.0);
        }
        if let Some((at, per)) = run.pending.front_mut().and_then(|p| p.mark.take()) {
            let sample = run.sample_at(at);
            self.shared.markers.push(Marker {
                counter: ring.written() + (piece.samples.len() / channels) as u64,
                sample,
                speed: run.dir() * per,
                epoch: run.epoch,
            });
        }
        let at = ring.written();
        let pushed = ring.push(&piece.samples) * channels;
        self.shared.level(super::Level {
            at,
            frames: (pushed / channels.max(1)) as u64,
            mix,
            lanes: std::mem::take(&mut piece.lanes),
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
        if let Some(unheard) = self.mix.unheard() {
            self.shared.fail(unheard);
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(true),
        }
    }

    fn refill(&mut self) -> Option<crate::error::Error> {
        let rate = f64::from(self.plan.rate());
        let total = self.plan.samples;
        let run = self.run.as_mut()?;
        let channels = usize::from(self.mix.channels()).max(1);
        let chunk = ((CHUNK_SECS * rate).round() as usize).max(1);
        let mut failed = None;
        let mut read = |mix: &mut AudioMix, n: usize| -> Vec<(Vec<f32>, Vec<f32>)> {
            (0..n)
                .step_by(chunk)
                .map(|i| {
                    let m = chunk.min(n - i);
                    mix.read_metered(m).unwrap_or_else(|e| {
                        failed = Some(e);
                        (vec![0.0; m * channels], Vec::new())
                    })
                })
                .collect()
        };
        let mut parts = Vec::new();
        let mut wrapped = false;
        if run.speed > 0.0 {
            let end = |cursor: i64, fed: i64| match self.looping {
                Some((_, b)) if cursor < b || (cursor == b && fed > 0) => b,
                _ => total,
            };
            if let Some((a, _)) = self.looping
                && run.cursor >= end(run.cursor, run.fed)
            {
                run.wraps.push_back((run.fed as f64, a));
                run.cursor = a;
                self.mix.seek(a);
                wrapped = true;
            }
            let n = ((CHUNK_SECS * rate * run.speed).round() as i64)
                .max(1)
                .min(end(run.cursor, run.fed) - run.cursor);
            if n > 0 {
                let mut at = run.fed as f64;
                for (s, lanes) in read(&mut self.mix, n as usize) {
                    let frames = (s.len() / channels) as f64;
                    parts.push((at, s, lanes));
                    at += frames;
                }
                run.cursor += n;
                run.fed += n;
            }
        } else {
            let floor = |cursor: i64, fed: i64| match self.looping {
                Some((a, _)) if cursor >= a || (cursor == a - 1 && fed > 0) => a,
                _ => 0,
            };
            if let Some((_, b)) = self.looping
                && run.cursor < floor(run.cursor, run.fed)
            {
                run.wraps.push_back((run.fed as f64, b - 1));
                run.cursor = b - 1;
                wrapped = true;
            }
            if run.cursor >= 0 {
                let n = ((BLOCK_SECS * rate * run.speed.abs()).round() as i64).max(1);
                let lo = (run.cursor + 1 - n).max(floor(run.cursor, run.fed));
                self.mix.seek(lo);
                let n = (run.cursor + 1 - lo) as usize;
                let mut end = (run.fed + run.cursor - lo + 1) as f64;
                for (mut s, lanes) in read(&mut self.mix, n) {
                    reverse(&mut s, channels);
                    end -= (s.len() / channels) as f64;
                    parts.push((end, s, lanes));
                }
                parts.reverse();
                run.fed += run.cursor + 1 - lo;
                run.cursor = lo - 1;
            }
        }
        let exhausted = parts.is_empty();
        match run.stretch.as_mut() {
            None => run.pending.extend(parts.into_iter().enumerate().map(
                |(i, (at, samples, lanes))| Piece {
                    samples,
                    mark: (wrapped && i == 0).then_some((at, 1.0)),
                    lanes,
                },
            )),
            Some(stretch) => {
                let mut blocks = Vec::new();
                for (at, s, lanes) in parts {
                    let frames = (s.len() / channels) as f64;
                    run.metered.push_back((at, at + frames, lanes));
                    blocks.extend(stretch.push(&s));
                }
                if exhausted {
                    blocks.extend(stretch.finish());
                }
                for b in blocks {
                    let frames = b.samples.len() / channels;
                    let to = b.at + frames as f64 * b.rate;
                    let lanes = peaks_over(&run.metered, b.at, to);
                    while run.metered.front().is_some_and(|m| m.1 <= b.at) {
                        run.metered.pop_front();
                    }
                    let mut samples = b.samples;
                    let cut = run
                        .wraps
                        .iter()
                        .find(|&&(w, _)| w > b.at && w < to)
                        .map(|&(w, _)| ((w - b.at) / b.rate).ceil() as usize)
                        .filter(|&k| k < frames);
                    if let Some(k) = cut {
                        let rest = samples.split_off(k * channels);
                        run.pending.push_back(Piece {
                            samples,
                            mark: Some((b.at, b.rate)),
                            lanes: lanes.clone(),
                        });
                        samples = rest;
                    }
                    let at = b.at + cut.unwrap_or(0) as f64 * b.rate;
                    run.pending.push_back(Piece {
                        samples,
                        mark: Some((at, b.rate)),
                        lanes,
                    });
                }
            }
        }
        if exhausted {
            run.exhausted = true;
        }
        failed
    }
}

fn peaks_over(metered: &VecDeque<(f64, f64, Vec<f32>)>, from: f64, to: f64) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::new();
    for (a, b, peaks) in metered {
        if *b > from && *a < to {
            if out.len() < peaks.len() {
                out.resize(peaks.len(), 0.0);
            }
            for (o, p) in out.iter_mut().zip(peaks) {
                *o = o.max(*p);
            }
        }
    }
    out
}

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
    use super::{peaks_over, reverse};

    #[test]
    fn reversing_keeps_each_frames_channels_in_order() {
        let mut s = vec![1.0, -1.0, 2.0, -2.0, 3.0, -3.0];
        reverse(&mut s, 2);
        assert_eq!(s, vec![3.0, -3.0, 2.0, -2.0, 1.0, -1.0]);
    }

    #[test]
    fn a_block_takes_the_peaks_of_the_chunks_it_overlaps() {
        let metered = [
            (0.0, 10.0, vec![0.1, 0.2]),
            (10.0, 20.0, vec![0.5, 0.1]),
            (20.0, 30.0, vec![0.9, 0.9]),
        ]
        .into();
        assert_eq!(peaks_over(&metered, 5.0, 12.0), vec![0.5, 0.2]);
    }
}
