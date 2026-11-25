use std::ops::RangeInclusive;

use crate::compressor;
use crate::error::{Error, Result};
use crate::model::{ClipCompressor, Compressor, Expander, Limiter, TrackDynamics};
use crate::request::{ExpanderSet, LimiterSet, TrackDynamicsSet};

pub const EXPANDER_THRESHOLD_DB: RangeInclusive<f64> = -50.0..=0.0;
pub const RANGE_DB: RangeInclusive<f64> = 0.0..=60.0;
pub const EXPANDER_RATIO: RangeInclusive<f64> = 1.1..=3.0;
pub const EXPANDER_ATTACK_MS: RangeInclusive<f64> = 0.5..=100.0;
pub const LIMITER_THRESHOLD_DB: RangeInclusive<f64> = -30.0..=0.0;
pub const LIMITER_ATTACK_MS: RangeInclusive<f64> = 0.7..=30.0;
pub const MAKEUP_DB: RangeInclusive<f64> = 0.0..=20.0;
pub const LIMITER_RATIO: f64 = 20.0;
pub const GATE_SLOPE: f64 = 4.0;
pub const OPEN_GATE_DB: f64 = -0.1;

const TICK: u32 = 3;
const FLOOR_DB: f64 = -100.0;
const KNEE_TOP: f64 = 0.1;
const KNEE_REACH: f64 = 0.13;
const KNEE_BEND: f64 = 0.153451;
const KNEE_POWER: f64 = 1.4;

pub fn first() -> TrackDynamics {
    TrackDynamics {
        on: true,
        expander: Expander {
            on: false,
            gate: false,
            threshold_db: -35.0,
            range_db: 18.0,
            ratio: 1.1,
            attack_ms: 1.4,
            hold_ms: 0.0,
            release_ms: 93.0,
        },
        compressor: compressor::module(),
        limiter: Limiter {
            on: false,
            threshold_db: -21.0,
            attack_ms: 0.71,
            hold_ms: 0.0,
            release_ms: 93.0,
        },
        makeup_db: 0.0,
    }
}

pub(crate) fn in_range(of: &str, name: &str, v: f64, range: &RangeInclusive<f64>) -> Result<()> {
    if range.contains(&v) {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "{of} {name} {v} is outside {}..{}",
        range.start(),
        range.end()
    )))
}

fn checked(d: &TrackDynamics) -> Result<()> {
    let e = &d.expander;
    in_range(
        "expander",
        "threshold",
        e.threshold_db,
        &EXPANDER_THRESHOLD_DB,
    )?;
    in_range("expander", "range", e.range_db, &RANGE_DB)?;
    in_range("expander", "ratio", e.ratio, &EXPANDER_RATIO)?;
    in_range("expander", "attack", e.attack_ms, &EXPANDER_ATTACK_MS)?;
    in_range("expander", "hold", e.hold_ms, &compressor::HOLD_MS)?;
    in_range("expander", "release", e.release_ms, &compressor::RELEASE_MS)?;
    compressor::checked(&d.compressor)?;
    let l = &d.limiter;
    in_range(
        "limiter",
        "threshold",
        l.threshold_db,
        &LIMITER_THRESHOLD_DB,
    )?;
    in_range("limiter", "attack", l.attack_ms, &LIMITER_ATTACK_MS)?;
    in_range("limiter", "hold", l.hold_ms, &compressor::HOLD_MS)?;
    in_range("limiter", "release", l.release_ms, &compressor::RELEASE_MS)?;
    in_range("dynamics", "make up", d.makeup_db, &MAKEUP_DB)
}

fn expanded(mut e: Expander, set: &ExpanderSet) -> Expander {
    e.on = set.on.unwrap_or(e.on);
    e.gate = set.gate.unwrap_or(e.gate);
    e.threshold_db = set.threshold_db.unwrap_or(e.threshold_db);
    e.range_db = set.range_db.unwrap_or(e.range_db);
    e.ratio = set.ratio.unwrap_or(e.ratio);
    e.attack_ms = set.attack_ms.unwrap_or(e.attack_ms);
    e.hold_ms = set.hold_ms.unwrap_or(e.hold_ms);
    e.release_ms = set.release_ms.unwrap_or(e.release_ms);
    e
}

fn limited(mut l: Limiter, set: &LimiterSet) -> Limiter {
    l.on = set.on.unwrap_or(l.on);
    l.threshold_db = set.threshold_db.unwrap_or(l.threshold_db);
    l.attack_ms = set.attack_ms.unwrap_or(l.attack_ms);
    l.hold_ms = set.hold_ms.unwrap_or(l.hold_ms);
    l.release_ms = set.release_ms.unwrap_or(l.release_ms);
    l
}

fn apply(mut d: TrackDynamics, set: &TrackDynamicsSet) -> Result<TrackDynamics> {
    if set
        .compressor
        .as_ref()
        .is_some_and(|c| c.makeup_db.is_some())
    {
        return Err(Error::invalid(
            "a track's compressor takes no makeup_db: the Dynamics have makeup_db",
        ));
    }
    d.on = set.on.unwrap_or(d.on);
    if let Some(e) = &set.expander {
        d.expander = expanded(d.expander, e);
    }
    if let Some(c) = &set.compressor {
        d.compressor = compressor::apply(d.compressor, c);
    }
    if let Some(l) = &set.limiter {
        d.limiter = limited(d.limiter, l);
    }
    d.makeup_db = set.makeup_db.unwrap_or(d.makeup_db);
    Ok(d)
}

pub fn check(set: &TrackDynamicsSet) -> Result<()> {
    checked(&apply(first(), set)?)
}

pub fn patched(old: Option<&TrackDynamics>, set: &TrackDynamicsSet) -> Result<TrackDynamics> {
    let d = apply(old.copied().unwrap_or_else(first), set)?;
    checked(&d)?;
    Ok(d)
}

pub fn problem(d: &TrackDynamics) -> Option<String> {
    checked(d).err().map(|e| e.message)
}

impl TrackDynamics {
    pub fn heard(&self) -> bool {
        self.on
            && (self.expander.on || self.compressor.on || self.limiter.on || self.makeup_db != 0.0)
    }
}

pub fn over_db(above: f64, knee: f64) -> f64 {
    let top = KNEE_TOP * knee;
    if above >= top {
        above
    } else {
        (above + KNEE_BEND * (top - above).powf(KNEE_POWER)).max(0.0)
    }
}

pub fn compressed_db(c: &Compressor, level_db: f64) -> f64 {
    -(1.0 - c.ratio.recip()) * over_db(level_db - c.threshold_db, c.knee)
}

pub fn limited_db(l: &Limiter, level_db: f64) -> f64 {
    -(1.0 - LIMITER_RATIO.recip()) * (level_db - l.threshold_db).max(0.0)
}

pub fn expanded_db(e: &Expander, level_db: f64) -> f64 {
    let (slope, open) = expansion(e);
    (slope * (level_db - e.threshold_db)).clamp(-e.range_db, 0.0) + open
}

pub fn steady_db(d: &TrackDynamics, level_db: f64) -> f64 {
    let mut level = level_db;
    if d.expander.on {
        level += expanded_db(&d.expander, level);
    }
    if d.compressor.on {
        let (wet, dry) = mixed(d.compressor.mix);
        let gain = 10f64.powf(compressed_db(&d.compressor, level) / 20.0);
        level += 20.0 * (wet * gain + dry).log10();
    }
    if d.limiter.on {
        level += limited_db(&d.limiter, level);
    }
    level + d.makeup_db
}

fn mixed(mix: f64) -> (f64, f64) {
    let dry = mix / 100.0;
    (1.0 - dry, dry)
}

fn expansion(e: &Expander) -> (f64, f64) {
    match e.gate {
        true => (GATE_SLOPE, OPEN_GATE_DB),
        false => (e.ratio - 1.0, 0.0),
    }
}

fn pole(ms: f64, rate: f64) -> f64 {
    let ticks = ms * rate / (1000.0 * f64::from(TICK));
    match ticks > 0.0 {
        true => (-ticks.recip()).exp(),
        false => 0.0,
    }
}

fn decibels(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Law {
    Expand {
        threshold: f64,
        slope: f64,
        open: f64,
    },
    Compress {
        threshold: f64,
        amount: f64,
        knee: f64,
    },
}

impl Law {
    fn gain_db(self, level: f64) -> f64 {
        match self {
            Law::Expand {
                threshold,
                slope,
                open,
            } => slope * (level - threshold) + open,
            Law::Compress {
                threshold,
                amount,
                knee,
            } => -amount * over_db(level - threshold, knee),
        }
    }
}

struct Times {
    attack_ms: f64,
    hold_ms: f64,
    release_ms: f64,
}

#[derive(Debug, Clone)]
struct Stage {
    law: Law,
    low: f64,
    high: f64,
    attack: f64,
    release: f64,
    hold: u64,
    held: u64,
    level: f64,
    peak: f32,
    count: u32,
    gain: f32,
    wet: f32,
    dry: f32,
}

impl Stage {
    fn new(law: Law, (low, high): (f64, f64), t: Times, (wet, dry): (f64, f64), rate: f64) -> Self {
        let level = low.max(FLOOR_DB).min(high);
        Self {
            law,
            low,
            high,
            attack: pole(t.attack_ms, rate),
            release: pole(t.release_ms, rate),
            hold: (t.hold_ms * rate / (1000.0 * f64::from(TICK))).round() as u64,
            held: 0,
            level,
            peak: 0.0,
            count: 0,
            gain: decibels(law.gain_db(level)),
            wet: wet as f32,
            dry: dry as f32,
        }
    }

    fn expander(e: &Expander, rate: f64) -> Self {
        let (slope, open) = expansion(e);
        let law = Law::Expand {
            threshold: e.threshold_db,
            slope,
            open,
        };
        let low = e.threshold_db - e.range_db / slope;
        let t = Times {
            attack_ms: e.attack_ms,
            hold_ms: e.hold_ms,
            release_ms: e.release_ms,
        };
        Self::new(law, (low, e.threshold_db), t, (1.0, 0.0), rate)
    }

    fn compressor(c: &Compressor, rate: f64, makeup_db: f64) -> Self {
        let law = Law::Compress {
            threshold: c.threshold_db,
            amount: 1.0 - c.ratio.recip(),
            knee: c.knee,
        };
        let low = c.threshold_db - KNEE_REACH * c.knee;
        let t = Times {
            attack_ms: c.attack_ms,
            hold_ms: c.hold_ms,
            release_ms: c.release_ms,
        };
        let (wet, dry) = mixed(c.mix);
        let wet = wet * 10f64.powf(makeup_db / 20.0);
        Self::new(law, (low, f64::INFINITY), t, (wet, dry), rate)
    }

    fn limiter(l: &Limiter, rate: f64) -> Self {
        let law = Law::Compress {
            threshold: l.threshold_db,
            amount: 1.0 - LIMITER_RATIO.recip(),
            knee: 0.0,
        };
        let t = Times {
            attack_ms: l.attack_ms,
            hold_ms: l.hold_ms,
            release_ms: l.release_ms,
        };
        Self::new(law, (l.threshold_db, f64::INFINITY), t, (1.0, 0.0), rate)
    }

    fn update(&mut self) {
        let input = match self.peak > 0.0 {
            true => 20.0 * f64::from(self.peak).log10(),
            false => FLOOR_DB,
        }
        .max(FLOOR_DB);
        if input > self.level {
            self.level = input + (self.level - input) * self.attack;
            self.held = self.hold;
        } else if self.held > 0 {
            self.held -= 1;
        } else {
            self.level = input + (self.level - input) * self.release;
        }
        self.level = self.level.clamp(self.low, self.high);
        self.gain = decibels(self.law.gain_db(self.level));
        self.peak = 0.0;
    }

    fn run(&mut self, samples: &mut [f32], channels: usize) {
        for frame in samples.chunks_exact_mut(channels) {
            self.peak = frame.iter().fold(self.peak, |a, v| a.max(v.abs()));
            self.count += 1;
            if self.count == TICK {
                self.count = 0;
                self.update();
            }
            let gain = self.gain * self.wet + self.dry;
            for sample in frame {
                *sample *= gain;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dynamics {
    stages: Vec<Stage>,
    makeup: f32,
    channels: usize,
}

impl Dynamics {
    pub fn track(d: &TrackDynamics, rate: u32, channels: u16) -> Self {
        let rate = f64::from(rate);
        let stages = [
            d.expander.on.then(|| Stage::expander(&d.expander, rate)),
            d.compressor
                .on
                .then(|| Stage::compressor(&d.compressor, rate, 0.0)),
            d.limiter.on.then(|| Stage::limiter(&d.limiter, rate)),
        ];
        Self {
            stages: stages.into_iter().flatten().collect(),
            makeup: decibels(d.makeup_db),
            channels: usize::from(channels).max(1),
        }
    }

    pub fn clip(c: &ClipCompressor, rate: u32, channels: u16) -> Self {
        Self {
            stages: vec![Stage::compressor(
                &c.compressor,
                f64::from(rate),
                c.makeup_db,
            )],
            makeup: 1.0,
            channels: usize::from(channels).max(1),
        }
    }

    pub fn run(&mut self, samples: &mut [f32]) {
        for stage in &mut self.stages {
            stage.run(samples, self.channels);
        }
        if self.makeup != 1.0 {
            for sample in samples {
                *sample *= self.makeup;
            }
        }
    }
}
