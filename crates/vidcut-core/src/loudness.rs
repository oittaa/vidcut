use std::collections::{BTreeMap, VecDeque};
use std::f64::consts::PI;

use serde::Serialize;

use crate::analysis::Loudness;

pub const TARGET: f64 = -23.0;

pub const NO_LEVEL: f64 = -70.0;

pub const SURROUND: f64 = 1.41;

pub const INTEGRATED_TOLERANCE: f64 = 1.0;

pub const SHORT_LIMIT: f64 = 3.0;

const RELATIVE_GATE: f64 = -10.0;

const RANGE_GATE: f64 = -20.0;

const RANGE_LOW: f64 = 0.10;

const RANGE_HIGH: f64 = 0.95;

const STEPS_PER_SECOND: f64 = 10.0;

const MOMENTARY_STEPS: usize = 4;

const SHORT_STEPS: usize = 30;

const BINS_PER_LU: f64 = 100.0;

const HIGHEST: f64 = 30.0;

const TAPS: usize = 32;

const LANES: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reading {
    pub running: bool,
    pub momentary: Option<f64>,
    pub channels: Vec<Option<f64>>,
    pub momentary_max: Option<f64>,
    pub short: Option<f64>,
    pub short_max: Option<f64>,
    pub range: f64,
    pub integrated: Option<f64>,
    pub true_peak: Option<f64>,
}

pub fn of_energy(energy: f64) -> f64 {
    -0.691 + 10.0 * energy.log10()
}

fn level(lufs: f64) -> Option<f64> {
    (lufs > NO_LEVEL).then_some(lufs)
}

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn run(&self, x: f64, s: &mut [f64; 2]) -> f64 {
        let y = self.b[0] * x + s[0];
        s[0] = self.b[1] * x - self.a[0] * y + s[1];
        s[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

fn k_weighting(rate: f64) -> [Biquad; 2] {
    let k = (PI * 1681.974450955533 / rate).tan();
    let q = 0.7071752369554196;
    let vh = 10f64.powf(3.999843853973347 / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    };
    let k = (PI * 38.13547087602444 / rate).tan();
    let q = 0.5003270373238773;
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    };
    [shelf, high_pass]
}

fn oversampling(rate: u32) -> usize {
    match rate {
        0..96_000 => 4,
        96_000..192_000 => 2,
        _ => 1,
    }
}

fn sinc(u: f64) -> f64 {
    if u == 0.0 {
        1.0
    } else {
        (PI * u).sin() / (PI * u)
    }
}

fn phase(p: usize, factor: usize) -> [f32; TAPS] {
    let half = (TAPS / 2) as f64;
    let mut taps = [0f64; TAPS];
    for (j, t) in taps.iter_mut().enumerate() {
        let u = half - 1.0 - j as f64 + p as f64 / factor as f64;
        let x = 2.0 * PI * u / TAPS as f64;
        *t = sinc(u) * (0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos());
    }
    let sum: f64 = taps.iter().sum();
    taps.map(|t| (t / sum) as f32)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; LANES];
    for (x, y) in a
        .as_chunks::<LANES>()
        .0
        .iter()
        .zip(b.as_chunks::<LANES>().0)
    {
        for i in 0..LANES {
            acc[i] += x[i] * y[i];
        }
    }
    acc.iter().sum()
}

#[derive(Debug, Clone)]
struct TruePeak {
    phases: Vec<[f32; TAPS]>,
    history: Vec<[f32; 2 * TAPS]>,
    at: usize,
}

impl TruePeak {
    fn new(rate: u32, channels: usize) -> Self {
        let factor = oversampling(rate);
        Self {
            phases: (1..factor).map(|p| phase(p, factor)).collect(),
            history: vec![[0.0; 2 * TAPS]; channels],
            at: 0,
        }
    }

    fn push(&mut self, frame: &[f32]) -> f32 {
        let mut peak = 0f32;
        for (h, &x) in self.history.iter_mut().zip(frame) {
            h[self.at] = x;
            h[self.at + TAPS] = x;
            peak = peak.max(x.abs());
            let recent = &h[self.at + 1..=self.at + TAPS];
            for p in &self.phases {
                peak = peak.max(dot(p, recent).abs());
            }
        }
        self.at = (self.at + 1) % TAPS;
        peak
    }

    fn flush(&mut self) -> f32 {
        let silence = vec![0f32; self.history.len()];
        let peak = (0..TAPS / 2).fold(0f32, |m, _| m.max(self.push(&silence)));
        self.clear();
        peak
    }

    fn clear(&mut self) {
        for h in &mut self.history {
            h.fill(0.0);
        }
        self.at = 0;
    }
}

#[derive(Debug, Clone, Default)]
struct Bins(BTreeMap<i32, (u64, f64)>);

impl Bins {
    fn add(&mut self, energy: f64) {
        let lufs = of_energy(energy);
        if lufs <= NO_LEVEL {
            return;
        }
        let key = (lufs.min(HIGHEST) * BINS_PER_LU).round() as i32;
        let bin = self.0.entry(key).or_insert((0, 0.0));
        bin.0 += 1;
        bin.1 += energy;
    }

    fn above(&self, gate: f64) -> impl Iterator<Item = (i32, u64, f64)> + '_ {
        self.0
            .iter()
            .filter(move |&(&k, _)| f64::from(k) / BINS_PER_LU > gate)
            .map(|(&k, &(n, e))| (k, n, e))
    }

    fn mean(&self, gate: f64) -> Option<f64> {
        let (n, e) = self
            .above(gate)
            .fold((0u64, 0.0), |(n, e), (_, m, f)| (n + m, e + f));
        (n > 0).then(|| of_energy(e / n as f64))
    }

    fn gated(&self, relative: f64) -> Option<(f64, f64)> {
        let mean = self.mean(f64::NEG_INFINITY)?;
        Some((mean + relative, mean))
    }

    fn integrated(&self) -> Option<f64> {
        let (gate, _) = self.gated(RELATIVE_GATE)?;
        self.mean(gate)
    }

    fn range(&self) -> f64 {
        let Some((gate, _)) = self.gated(RANGE_GATE) else {
            return 0.0;
        };
        let kept: Vec<(i32, u64)> = self.above(gate).map(|(k, n, _)| (k, n)).collect();
        let total: u64 = kept.iter().map(|b| b.1).sum();
        if total == 0 {
            return 0.0;
        }
        let rank = |share: f64| ((total - 1) as f64 * share).round() as u64;
        let at = |rank: u64| {
            let mut seen = 0;
            for &(k, n) in &kept {
                seen += n;
                if seen > rank {
                    return f64::from(k) / BINS_PER_LU;
                }
            }
            0.0
        };
        at(rank(RANGE_HIGH)) - at(rank(RANGE_LOW))
    }
}

#[derive(Debug, Clone)]
pub struct Meter {
    rate: u32,
    weights: Vec<f64>,
    filters: [Biquad; 2],
    states: Vec<[[f64; 2]; 2]>,
    peaks: TruePeak,
    step: usize,
    filled: usize,
    sums: Vec<f64>,
    steps: VecDeque<Vec<f64>>,
    played: usize,
    running: bool,
    momentary: f64,
    channels: Vec<f64>,
    short: f64,
    momentary_max: f64,
    short_max: f64,
    true_peak: f32,
    sample_peak: f32,
    blocks: Bins,
    shorts: Bins,
}

impl Meter {
    pub fn new(rate: u32, weights: Vec<f64>) -> Self {
        let rate = rate.max(1);
        let weights = if weights.is_empty() {
            vec![1.0]
        } else {
            weights
        };
        let n = weights.len();
        Self {
            rate,
            filters: k_weighting(f64::from(rate)),
            states: vec![[[0.0; 2]; 2]; n],
            peaks: TruePeak::new(rate, n),
            step: ((f64::from(rate) / STEPS_PER_SECOND).round() as usize).max(1),
            filled: 0,
            sums: vec![0.0; n],
            steps: VecDeque::with_capacity(SHORT_STEPS + 1),
            played: 0,
            running: true,
            momentary: f64::NEG_INFINITY,
            channels: vec![f64::NEG_INFINITY; n],
            short: f64::NEG_INFINITY,
            momentary_max: f64::NEG_INFINITY,
            short_max: f64::NEG_INFINITY,
            true_peak: 0.0,
            sample_peak: 0.0,
            blocks: Bins::default(),
            shorts: Bins::default(),
            weights,
        }
    }

    pub fn fit(&mut self, rate: u32, weights: &[f64]) {
        if self.rate != rate.max(1) || self.weights != weights {
            let running = self.running;
            *self = Self::new(rate, weights.to_vec());
            self.running = running;
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn start(&mut self) {
        self.running = true;
    }

    pub fn pause(&mut self) {
        self.running = false;
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.rate, std::mem::take(&mut self.weights));
        self.running = false;
    }

    pub fn cut(&mut self) {
        self.finish();
        self.states.fill([[0.0; 2]; 2]);
        self.sums.fill(0.0);
        self.filled = 0;
        self.steps.clear();
        self.played = 0;
    }

    pub fn finish(&mut self) {
        self.true_peak = self.true_peak.max(self.peaks.flush());
    }

    pub fn push(&mut self, samples: &[f32]) {
        let n = self.weights.len();
        for frame in samples.chunks_exact(n) {
            self.true_peak = self.true_peak.max(self.peaks.push(frame));
            for ((state, sum), &x) in self.states.iter_mut().zip(&mut self.sums).zip(frame) {
                self.sample_peak = self.sample_peak.max(x.abs());
                let shelved = self.filters[0].run(f64::from(x), &mut state[0]);
                let y = self.filters[1].run(shelved, &mut state[1]);
                *sum += y * y;
            }
            self.filled += 1;
            if self.filled == self.step {
                self.end_step();
            }
        }
    }

    fn window(&self, steps: usize) -> Vec<f64> {
        let mut out = vec![0.0; self.weights.len()];
        for s in self.steps.iter().rev().take(steps) {
            for (o, e) in out.iter_mut().zip(s) {
                *o += e;
            }
        }
        let samples = (steps * self.step) as f64;
        out.iter_mut().for_each(|o| *o /= samples);
        out
    }

    fn weighted(&self, means: &[f64]) -> f64 {
        means.iter().zip(&self.weights).map(|(z, g)| z * g).sum()
    }

    fn end_step(&mut self) {
        let sums = std::mem::replace(&mut self.sums, vec![0.0; self.weights.len()]);
        self.steps.push_back(sums);
        if self.steps.len() > SHORT_STEPS {
            self.steps.pop_front();
        }
        self.filled = 0;
        self.played = (self.played + 1).min(SHORT_STEPS);
        let momentary = self.window(MOMENTARY_STEPS);
        let block = self.weighted(&momentary);
        let short = self.weighted(&self.window(SHORT_STEPS));
        self.momentary = of_energy(block);
        self.short = of_energy(short);
        self.channels = momentary
            .iter()
            .zip(&self.weights)
            .map(|(z, g)| of_energy(z * g))
            .collect();
        if !self.running {
            return;
        }
        self.momentary_max = self.momentary_max.max(self.momentary);
        self.short_max = self.short_max.max(self.short);
        if self.played >= MOMENTARY_STEPS {
            self.blocks.add(block);
        }
        if self.played >= SHORT_STEPS {
            self.shorts.add(short);
        }
    }

    fn peak_db(peak: f32) -> Option<f64> {
        (peak > 0.0).then(|| 20.0 * f64::from(peak).log10())
    }

    pub fn reading(&self) -> Reading {
        Reading {
            running: self.running,
            momentary: level(self.momentary),
            channels: (self.channels.iter().zip(&self.weights))
                .filter(|(_, g)| **g > 0.0)
                .map(|(l, _)| level(*l))
                .collect(),
            momentary_max: level(self.momentary_max),
            short: level(self.short),
            short_max: level(self.short_max),
            range: self.shorts.range(),
            integrated: self.blocks.integrated(),
            true_peak: Self::peak_db(self.true_peak),
        }
    }

    pub fn loudness(&self) -> Loudness {
        let none = f64::NEG_INFINITY;
        Loudness {
            integrated: self.blocks.integrated().unwrap_or(none),
            range: self.shorts.range(),
            true_peak: Self::peak_db(self.true_peak).unwrap_or(none),
            sample_peak: Self::peak_db(self.sample_peak).unwrap_or(none),
            momentary_max: level(self.momentary_max).unwrap_or(none),
            short_max: level(self.short_max).unwrap_or(none),
        }
    }
}
