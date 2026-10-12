//! The audio thread: the plan's lanes mixed, with their faders, into the
//! ring a tenth of a second ahead of the device, each run of samples marked
//! with where it is on the timeline.
//!
//! At other speeds the mix goes through FFmpeg's `atempo`, which keeps the
//! pitch. Backward the sound plays reversed: the mix is read forward a
//! block at a time, from further and further back, and each block reversed.
//! Paused, steps play their frame's sound ([`super::scrub`]).

use std::sync::Arc;
use std::time::Duration;

use super::clock::Marker;
use super::plan::Plan;
use super::scrub::{Waiting, snippet};
use super::{AudioSnap, Mode, Shared};
use crate::error::{Context, Error, Result};
use crate::export::DECLICK_MS;
use crate::export::audio::AudioMix;
use crate::ffmpeg::{self, filter, format, frame};

/// Sound kept queued ahead of the device.
const AHEAD_SECS: f64 = 0.1;

/// Sound mixed at a time.
const CHUNK_SECS: f64 = 0.01;

/// Room in the ring a chunk needs, with what the tempo filter may give at
/// once.
const MIN_ROOM_SECS: f64 = 0.1;

/// Sound read at a time going backward, in seconds of play: each block
/// costs a seek of every lane's stream.
const BLOCK_SECS: f64 = 0.2;

/// What is being played.
struct Run {
    epoch: u64,
    speed: f64,
    /// The next timeline sample to mix (from the plan's start); backward,
    /// the next to play, the mix read up to it.
    cursor: i64,
    tempo: Option<Tempo>,
    /// Backward: a block mixed and reversed (and through the tempo filter),
    /// and how much of it is queued.
    block: Vec<f32>,
    queued: usize,
    /// Backward: the tempo filter gave what it held at the start.
    flushed: bool,
    /// The end (or the start, backwards) was reached and marked.
    ended: bool,
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

    /// Starts play request `epoch`: what was queued for the device is
    /// dropped, and the new sound marked from sample `from` on.
    fn start(&mut self, from: i64, speed: f64, epoch: u64) {
        let tempo = (speed.abs() != 1.0)
            .then(|| Tempo::new(speed.abs(), self.plan.rate(), self.plan.plan.channels).ok())
            .flatten();
        self.run = Some(Run {
            epoch,
            speed,
            cursor: from,
            tempo,
            block: Vec::new(),
            queued: 0,
            flushed: false,
            ended: false,
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
        if run.speed > 0.0 {
            self.mix.seek(at);
        }
        run.cursor = at;
        run.ended = false;
        run.block.clear();
        run.queued = 0;
        run.flushed = false;
        if let Some(t) = run.tempo.as_mut()
            && let Ok(fresh) = Tempo::new(t.speed, t.rate, t.channels as u16)
        {
            *t = fresh;
        }
        self.shared.markers.push(Marker {
            counter: ring.written(),
            sample: at,
            speed: run.speed,
            epoch: run.epoch,
        });
    }

    /// Mixes the next chunk when the device's queue is short; whether it
    /// did.
    fn produce(&mut self) -> Result<bool> {
        let ring = self.shared.ring.clone();
        if self.run.as_ref().is_some_and(|r| r.speed < 0.0) {
            return self.produce_backward();
        }
        let Some(run) = self.run.as_mut() else {
            return Ok(self.waiting.offer(&ring, self.plan.rate()));
        };
        let rate = f64::from(self.plan.rate());
        // Room for the chunk, whatever the tempo makes of it: a device that
        // stopped pulling leaves none, and the thread waits for it rather
        // than for room.
        let short = ring.queued() as f64 >= AHEAD_SECS * rate;
        if run.ended || short || (ring.room() as f64) < MIN_ROOM_SECS * rate {
            return Ok(false);
        }
        let chunk = (CHUNK_SECS * rate).round() as i64;
        let channels = ring.channels();
        let total = self.plan.samples;
        let mut failed = None;
        let mut samples = if run.speed == 1.0 || run.tempo.is_some() {
            let n = ((chunk as f64 * run.speed).round() as i64).min(total - run.cursor);
            let tempo = run.tempo.as_mut();
            if n <= 0 {
                let tail = match tempo {
                    Some(t) => t.flush()?,
                    None => Vec::new(),
                };
                push(&ring, &tail);
                end(&self.shared, run, total, ring.written());
                return Ok(true);
            }
            let mixed = match self.mix.read(n as usize) {
                Ok(s) => s,
                // A broken file is silent; the clock goes on.
                Err(e) => {
                    failed = Some(e);
                    vec![0.0; n as usize * channels]
                }
            };
            run.cursor += n;
            match tempo {
                Some(t) => t.process(&mixed)?,
                None => mixed,
            }
        } else {
            // No tempo filter: silence, moving the clock.
            let n = chunk.min(((total - run.cursor) as f64 / run.speed).ceil() as i64);
            if n <= 0 {
                end(&self.shared, run, total, ring.written());
                return Ok(true);
            }
            run.cursor = (run.cursor + (n as f64 * run.speed).round() as i64).min(total);
            vec![0.0; n as usize * channels]
        };
        if self.muted {
            samples.fill(0.0);
        }
        push(&ring, &samples);
        match failed {
            Some(e) => Err(e),
            None => Ok(true),
        }
    }

    /// Backward: the next chunk of a reversed block, a new block mixed and
    /// reversed when one is used up; whether there was one.
    fn produce_backward(&mut self) -> Result<bool> {
        let ring = self.shared.ring.clone();
        let rate = f64::from(self.plan.rate());
        let Some(run) = self.run.as_mut() else {
            return Ok(false);
        };
        let short = ring.queued() as f64 >= AHEAD_SECS * rate;
        if run.ended || short || (ring.room() as f64) < MIN_ROOM_SECS * rate {
            return Ok(false);
        }
        let channels = ring.channels();
        let mut failed = None;
        if run.queued * channels >= run.block.len() {
            run.queued = 0;
            run.block.clear();
            if run.cursor < 0 {
                // The start: what the tempo filter still holds, then the end.
                if let (Some(t), false) = (run.tempo.as_mut(), run.flushed) {
                    run.block = t.flush()?;
                    run.flushed = true;
                }
                if run.block.is_empty() {
                    end(&self.shared, run, 0, ring.written());
                    return Ok(true);
                }
            } else {
                // Samples `lo..=cursor` of the mix, read forward, reversed.
                let n = ((BLOCK_SECS * rate * run.speed.abs()).round() as i64).max(1);
                let lo = (run.cursor + 1 - n).max(0);
                self.mix.seek(lo);
                let len = (run.cursor + 1 - lo) as usize;
                let mut mixed = match self.mix.read(len) {
                    Ok(s) => s,
                    // A broken file is silent; the clock goes on.
                    Err(e) => {
                        failed = Some(e);
                        vec![0.0; len * channels]
                    }
                };
                reverse(&mut mixed, channels);
                run.cursor = lo - 1;
                run.block = match run.tempo.as_mut() {
                    Some(t) => t.process(&mixed)?,
                    None => mixed,
                };
            }
        }
        let chunk = (CHUNK_SECS * rate).round() as usize;
        let from = run.queued * channels;
        let take = (run.block.len() - from).min(chunk * channels);
        let mut samples = run.block[from..from + take].to_vec();
        run.queued += take / channels;
        if self.muted {
            samples.fill(0.0);
        }
        push(&ring, &samples);
        match failed {
            Some(e) => Err(e),
            None => Ok(true),
        }
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

/// Queues all of `samples`, never dropping any: `produce` made sure of the
/// room.
fn push(ring: &super::ring::Ring, samples: &[f32]) {
    let pushed = ring.push(samples) * ring.channels();
    debug_assert_eq!(pushed, samples.len(), "no room for a chunk");
}

/// Marks the run's end at ring position `counter`: the clock holds at
/// sample `edge` once it is heard.
fn end(shared: &Shared, run: &mut Run, edge: i64, counter: u64) {
    shared.markers.push(Marker {
        counter,
        sample: edge,
        speed: 0.0,
        epoch: run.epoch,
    });
    run.ended = true;
}

/// Plays sound faster or slower keeping its pitch: FFmpeg's `atempo`,
/// chained so each stage stays within 0.5 to 2, where it blends rather
/// than skips.
struct Tempo {
    graph: filter::Graph,
    speed: f64,
    rate: u32,
    channels: usize,
    layout: ffmpeg::ChannelLayout,
    pts: i64,
}

impl Tempo {
    fn new(speed: f64, rate: u32, channels: u16) -> Result<Self> {
        let layout = ffmpeg::ChannelLayout::default(i32::from(channels));
        let find = |name: &str| {
            filter::find(name).ok_or_else(|| Error::Invalid(format!("no {name} filter")))
        };
        let mut graph = filter::Graph::new();
        let args = format!(
            "time_base=1/{rate}:sample_rate={rate}:sample_fmt=flt:channel_layout={}",
            layout_name(channels)
        );
        graph
            .add(&find("abuffer")?, "in", &args)
            .ctx(|| "creating tempo source".into())?;
        graph
            .add(&find("abuffersink")?, "out", "")
            .ctx(|| "creating tempo sink".into())?;
        let mut stages = Vec::new();
        let mut s = speed;
        while s > 2.0 {
            stages.push(2.0);
            s /= 2.0;
        }
        while s < 0.5 {
            stages.push(0.5);
            s /= 0.5;
        }
        stages.push(s);
        let spec = stages
            .iter()
            .map(|f| format!("atempo={f}"))
            .chain(std::iter::once("aformat=sample_fmts=flt".into()))
            .collect::<Vec<_>>()
            .join(",");
        graph
            .output("in", 0)
            .and_then(|p| p.input("out", 0))
            .and_then(|p| p.parse(&spec))
            .ctx(|| format!("building {spec}"))?;
        graph.validate().ctx(|| format!("configuring {spec}"))?;
        Ok(Self {
            graph,
            speed,
            rate,
            channels: usize::from(channels),
            layout,
            pts: 0,
        })
    }

    /// Takes `input` (interleaved) and gives what is ready.
    fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        let n = input.len() / self.channels;
        let mut f = frame::Audio::new(
            format::Sample::F32(format::sample::Type::Packed),
            n,
            self.layout,
        );
        f.set_rate(self.rate);
        f.set_pts(Some(self.pts));
        self.pts += n as i64;
        let data = f.data_mut(0);
        for (i, s) in input.iter().enumerate() {
            data[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        self.graph
            .get("in")
            .expect("source")
            .source()
            .add(&f)
            .ctx(|| "changing the tempo".into())?;
        self.pull()
    }

    /// The rest, at the end.
    fn flush(&mut self) -> Result<Vec<f32>> {
        self.graph
            .get("in")
            .expect("source")
            .source()
            .flush()
            .ctx(|| "changing the tempo".into())?;
        self.pull()
    }

    fn pull(&mut self) -> Result<Vec<f32>> {
        let mut out = Vec::new();
        let mut sink = self.graph.get("out").expect("sink");
        loop {
            let mut f = frame::Audio::empty();
            match sink.sink().frame(&mut f) {
                Ok(()) => {
                    let n = f.samples() * self.channels;
                    let data = f.data(0);
                    out.extend(
                        data[..n * 4]
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|b| f32::from_le_bytes(*b)),
                    );
                }
                Err(ffmpeg::Error::Eof)
                | Err(ffmpeg::Error::Other {
                    errno: ffmpeg::util::error::EAGAIN,
                }) => return Ok(out),
                Err(e) => {
                    return Err(Error::Ffmpeg {
                        context: "changing the tempo".into(),
                        source: e,
                    });
                }
            }
        }
    }
}

/// FFmpeg's name for the usual layout of `channels` channels (`stereo`,
/// `5.1`).
fn layout_name(channels: u16) -> String {
    use ffmpeg::ffi;
    // SAFETY: a layout made, described into a buffer large enough for any
    // name, and freed.
    unsafe {
        let mut layout: ffi::AVChannelLayout = std::mem::zeroed();
        ffi::av_channel_layout_default(&mut layout, i32::from(channels));
        let mut buf = [0 as std::ffi::c_char; 128];
        ffi::av_channel_layout_describe(&layout, buf.as_mut_ptr(), buf.len());
        ffi::av_channel_layout_uninit(&mut layout);
        std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{Tempo, reverse};

    #[test]
    fn reversing_keeps_each_frames_channels_in_order() {
        let mut s = vec![1.0, -1.0, 2.0, -2.0, 3.0, -3.0];
        reverse(&mut s, 2);
        assert_eq!(s, vec![3.0, -3.0, 2.0, -2.0, 1.0, -1.0]);
    }

    #[test]
    fn tempo_plays_faster_and_slower_keeping_length_in_step() {
        crate::init();
        let rate = 48_000;
        // One second of a 1 kHz tone, in 10 ms chunks.
        let tone: Vec<f32> = (0..rate)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate as f32).sin();
                [v, v]
            })
            .collect();
        for speed in [2.0, 0.5, 8.0] {
            let mut t = Tempo::new(speed, rate as u32, 2).unwrap();
            let mut out = Vec::new();
            for chunk in tone.chunks(960) {
                out.extend(t.process(chunk).unwrap());
            }
            out.extend(t.flush().unwrap());
            let frames = out.len() as f64 / 2.0;
            let expected = f64::from(rate) / speed;
            assert!(
                (frames - expected).abs() < expected * 0.05,
                "{speed}x made {frames} frames of {expected}"
            );
        }
    }
}
