use std::ops::RangeInclusive;

use crate::error::{Error, Result};
use crate::model::Compressor;
use crate::request::CompressorSet;

pub const THRESHOLD_DB: RangeInclusive<f64> = -50.0..=0.0;
pub const RATIO: RangeInclusive<f64> = 1.0..=10.0;
pub const KNEE_DB: RangeInclusive<f64> = 0.0..=30.0;
pub const MIX: RangeInclusive<f64> = 0.0..=100.0;
pub const ATTACK_MS: RangeInclusive<f64> = 0.0..=100.0;
pub const HOLD_MS: RangeInclusive<f64> = 0.0..=4000.0;
pub const RELEASE_MS: RangeInclusive<f64> = 50.0..=4000.0;
pub const MAKEUP_DB: RangeInclusive<f64> = 0.0..=24.0;

pub fn first() -> Compressor {
    Compressor {
        on: true,
        threshold_db: -15.0,
        ratio: 2.0,
        knee_db: 0.0,
        mix: 0.0,
        attack_ms: 1.4,
        hold_ms: 0.0,
        release_ms: 93.0,
        makeup_db: 0.0,
    }
}

fn in_range(name: &str, v: f64, range: &RangeInclusive<f64>) -> Result<()> {
    if range.contains(&v) {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "compressor {name} {v} is outside {}..{}",
        range.start(),
        range.end()
    )))
}

fn checked(c: &Compressor) -> Result<()> {
    in_range("threshold", c.threshold_db, &THRESHOLD_DB)?;
    in_range("ratio", c.ratio, &RATIO)?;
    in_range("knee", c.knee_db, &KNEE_DB)?;
    in_range("mix", c.mix, &MIX)?;
    in_range("attack", c.attack_ms, &ATTACK_MS)?;
    in_range("hold", c.hold_ms, &HOLD_MS)?;
    in_range("release", c.release_ms, &RELEASE_MS)?;
    in_range("makeup gain", c.makeup_db, &MAKEUP_DB)
}

pub fn check(set: &CompressorSet) -> Result<()> {
    checked(&apply(first(), set))
}

fn apply(mut c: Compressor, set: &CompressorSet) -> Compressor {
    c.on = set.on.unwrap_or(c.on);
    c.threshold_db = set.threshold_db.unwrap_or(c.threshold_db);
    c.ratio = set.ratio.unwrap_or(c.ratio);
    c.knee_db = set.knee_db.unwrap_or(c.knee_db);
    c.mix = set.mix.unwrap_or(c.mix);
    c.attack_ms = set.attack_ms.unwrap_or(c.attack_ms);
    c.hold_ms = set.hold_ms.unwrap_or(c.hold_ms);
    c.release_ms = set.release_ms.unwrap_or(c.release_ms);
    c.makeup_db = set.makeup_db.unwrap_or(c.makeup_db);
    c
}

pub fn patched(old: Option<Compressor>, set: &CompressorSet) -> Result<Compressor> {
    let c = apply(old.unwrap_or_else(first), set);
    checked(&c)?;
    Ok(c)
}

pub fn problem(c: &Compressor) -> Option<String> {
    checked(c).err().map(|e| e.message)
}

impl Compressor {
    pub fn heard(&self) -> bool {
        self.on && self.mix < 100.0 && (self.ratio > 1.0 || self.makeup_db != 0.0)
    }
}

pub fn reduction_db(c: &Compressor, input_db: f64) -> f64 {
    let over = input_db - c.threshold_db;
    let slope = 1.0 / c.ratio - 1.0;
    let half = c.knee_db / 2.0;
    if over.abs() < half {
        slope * (over + half).powi(2) / (2.0 * c.knee_db)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

fn pole(ms: f64, rate: f64) -> f64 {
    match ms > 0.0 {
        true => (-1000.0 / (ms * rate)).exp(),
        false => 0.0,
    }
}

#[derive(Debug, Clone)]
pub struct Dynamics {
    settings: Compressor,
    channels: usize,
    attack: f64,
    release: f64,
    hold: u64,
    held: u64,
    reduction: f64,
}

impl Dynamics {
    pub fn new(settings: Compressor, rate: u32, channels: u16) -> Self {
        let rate = f64::from(rate);
        Self {
            settings,
            channels: usize::from(channels).max(1),
            attack: pole(settings.attack_ms, rate),
            release: pole(settings.release_ms, rate),
            hold: (settings.hold_ms * rate / 1000.0).round() as u64,
            held: 0,
            reduction: 0.0,
        }
    }

    pub fn settings(&self) -> Compressor {
        self.settings
    }

    pub fn run(&mut self, samples: &mut [f32]) {
        let dry = self.settings.mix / 100.0;
        for frame in samples.chunks_exact_mut(self.channels) {
            let peak = frame.iter().fold(0f32, |a, v| a.max(v.abs()));
            let input = 20.0 * f64::from(peak).max(1e-9).log10();
            let target = reduction_db(&self.settings, input);
            if target < self.reduction {
                self.reduction = target + (self.reduction - target) * self.attack;
                self.held = self.hold;
            } else if self.held > 0 {
                self.held -= 1;
            } else {
                self.reduction = target + (self.reduction - target) * self.release;
            }
            let wet = 10f64.powf((self.reduction + self.settings.makeup_db) / 20.0);
            let gain = (dry + (1.0 - dry) * wet) as f32;
            for sample in frame {
                *sample *= gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;

    fn with(change: impl FnOnce(&mut Compressor)) -> Compressor {
        let mut c = first();
        change(&mut c);
        c
    }

    fn tone(count: usize, amplitude: f32) -> Vec<f32> {
        (0..count)
            .map(|i| amplitude * (i as f32 * 0.13).sin())
            .collect()
    }

    fn rms_db(x: &[f32]) -> f64 {
        let p = x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64;
        10.0 * p.log10()
    }

    fn tenths(v: f64) -> f64 {
        (v * 10.0).round() / 10.0
    }

    fn change_db(c: Compressor, amplitude: f32) -> f64 {
        let dry = tone(48000, amplitude);
        let mut wet = dry.clone();
        Dynamics::new(c, RATE, 1).run(&mut wet);
        tenths(rms_db(&wet[24000..]) - rms_db(&dry[24000..]))
    }

    #[test]
    fn a_level_below_the_threshold_is_not_reduced() {
        assert_eq!(reduction_db(&first(), -20.0), 0.0);
    }

    #[test]
    fn a_level_above_the_threshold_is_reduced_by_the_ratio() {
        assert_eq!(reduction_db(&first(), -5.0), -5.0);
    }

    #[test]
    fn a_soft_knee_starts_below_the_threshold() {
        assert_eq!(reduction_db(&with(|c| c.knee_db = 6.0), -15.0), -0.375);
    }

    #[test]
    fn a_new_compressor_is_heard() {
        assert!(first().heard());
    }

    #[test]
    fn a_ratio_of_one_is_not_heard() {
        assert!(!with(|c| c.ratio = 1.0).heard());
    }

    #[test]
    fn a_mix_of_one_hundred_is_not_heard() {
        assert!(!with(|c| c.mix = 100.0).heard());
    }

    #[test]
    fn a_steady_tone_is_reduced_by_the_curve() {
        assert_eq!(change_db(with(|c| c.attack_ms = 0.0), 0.5), -4.5);
    }

    #[test]
    fn a_quiet_tone_is_not_changed() {
        assert_eq!(change_db(first(), 0.05), 0.0);
    }

    #[test]
    fn makeup_gain_raises_the_result() {
        assert_eq!(change_db(with(|c| c.makeup_db = 6.0), 0.05), 6.0);
    }

    #[test]
    fn a_mix_of_fifty_is_half_of_each() {
        let halved = with(|c| {
            c.attack_ms = 0.0;
            c.mix = 50.0;
        });
        assert_eq!(change_db(halved, 0.5), -2.0);
    }

    #[test]
    fn a_ratio_past_ten_is_refused() {
        let set = CompressorSet {
            ratio: Some(11.0),
            ..CompressorSet::default()
        };
        assert!(check(&set).is_err());
    }

    #[test]
    fn a_release_below_fifty_ms_is_refused() {
        let set = CompressorSet {
            release_ms: Some(10.0),
            ..CompressorSet::default()
        };
        assert!(check(&set).is_err());
    }

    #[test]
    fn a_patch_changes_only_the_fields_given() {
        let set = CompressorSet {
            ratio: Some(4.0),
            ..CompressorSet::default()
        };
        let c = patched(None, &set).unwrap();
        assert_eq!((c.ratio, c.threshold_db), (4.0, -15.0));
    }
}
