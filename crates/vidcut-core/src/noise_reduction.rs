use std::ops::RangeInclusive;

use crate::error::{Error, Result};
use crate::model::NoiseReduction;
use crate::request::NoiseReductionSet;

pub const THRESHOLD_DB: RangeInclusive<f64> = 0.0..=40.0;
pub const ATTACK_MS: RangeInclusive<f64> = 10.0..=1000.0;
pub const SENSITIVITY_DB: RangeInclusive<f64> = 0.0..=12.0;
pub const RATIO: RangeInclusive<f64> = 1.0..=10.0;
pub const FREQUENCY_SMOOTHING: RangeInclusive<f64> = 0.0..=100.0;
pub const DRY_WET: RangeInclusive<f64> = 0.0..=100.0;
pub const LEVEL_DB: RangeInclusive<f64> = -6.0..=18.0;

pub fn first() -> NoiseReduction {
    NoiseReduction {
        on: true,
        threshold_db: 12.0,
        attack_ms: 200.0,
        sensitivity_db: 3.0,
        ratio: 4.0,
        frequency_smoothing: 25.0,
        time_smoothing: true,
        dry_wet: 100.0,
        level_db: 0.0,
    }
}

fn in_range(name: &str, v: f64, range: &RangeInclusive<f64>) -> Result<()> {
    if range.contains(&v) {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "noise reduction {name} {v} is outside {}..{}",
        range.start(),
        range.end()
    )))
}

fn checked(n: &NoiseReduction) -> Result<()> {
    in_range("threshold", n.threshold_db, &THRESHOLD_DB)?;
    in_range("attack", n.attack_ms, &ATTACK_MS)?;
    in_range("sensitivity", n.sensitivity_db, &SENSITIVITY_DB)?;
    in_range("ratio", n.ratio, &RATIO)?;
    in_range(
        "frequency smoothing",
        n.frequency_smoothing,
        &FREQUENCY_SMOOTHING,
    )?;
    in_range("dry/wet", n.dry_wet, &DRY_WET)?;
    in_range("level", n.level_db, &LEVEL_DB)
}

fn apply(mut n: NoiseReduction, set: &NoiseReductionSet) -> NoiseReduction {
    n.on = set.on.unwrap_or(n.on);
    n.threshold_db = set.threshold_db.unwrap_or(n.threshold_db);
    n.attack_ms = set.attack_ms.unwrap_or(n.attack_ms);
    n.sensitivity_db = set.sensitivity_db.unwrap_or(n.sensitivity_db);
    n.ratio = set.ratio.unwrap_or(n.ratio);
    n.frequency_smoothing = set.frequency_smoothing.unwrap_or(n.frequency_smoothing);
    n.time_smoothing = set.time_smoothing.unwrap_or(n.time_smoothing);
    n.dry_wet = set.dry_wet.unwrap_or(n.dry_wet);
    n.level_db = set.level_db.unwrap_or(n.level_db);
    n
}

pub fn check(set: &NoiseReductionSet) -> Result<()> {
    checked(&apply(first(), set))
}

pub fn patched(old: Option<NoiseReduction>, set: &NoiseReductionSet) -> Result<NoiseReduction> {
    let n = apply(old.unwrap_or_else(first), set);
    checked(&n)?;
    Ok(n)
}

pub fn problem(n: &NoiseReduction) -> Option<String> {
    checked(n).err().map(|e| e.message)
}

impl NoiseReduction {
    pub fn heard(&self) -> bool {
        self.on && self.dry_wet > 0.0
    }
}

pub fn gain_of(n: &NoiseReduction, signal: f64, noise: f64) -> f64 {
    let sensitivity = 10f64.powf(n.sensitivity_db / 10.0);
    let threshold = 10f64.powf(n.threshold_db / 10.0);
    let snr = signal / (noise * sensitivity).max(1e-20);
    (snr / threshold).min(1.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenths(v: f64) -> f64 {
        (v * 10.0).round() / 10.0
    }

    fn db(gain: f64) -> f64 {
        tenths(20.0 * gain.log10())
    }

    #[test]
    fn a_new_noise_reduction_is_heard() {
        assert!(first().heard());
    }

    #[test]
    fn a_dry_mix_is_not_heard() {
        let mut n = first();
        n.dry_wet = 0.0;
        assert!(!n.heard());
    }

    #[test]
    fn a_bin_at_the_noise_level_loses_threshold_and_sensitivity() {
        assert_eq!(db(gain_of(&first(), 1.0, 1.0)), -15.0);
    }

    #[test]
    fn a_bin_far_above_the_noise_is_kept() {
        assert_eq!(gain_of(&first(), 1000.0, 1.0), 1.0);
    }

    #[test]
    fn a_higher_threshold_removes_more() {
        let mut n = first();
        n.threshold_db = 24.0;
        assert_eq!(db(gain_of(&n, 1.0, 1.0)), -27.0);
    }

    #[test]
    fn a_threshold_past_forty_db_is_refused() {
        let set = NoiseReductionSet {
            threshold_db: Some(41.0),
            ..NoiseReductionSet::default()
        };
        assert!(check(&set).is_err());
    }

    #[test]
    fn a_patch_changes_only_the_fields_given() {
        let set = NoiseReductionSet {
            ratio: Some(2.0),
            ..NoiseReductionSet::default()
        };
        let n = patched(None, &set).unwrap();
        assert_eq!((n.ratio, n.threshold_db), (2.0, 12.0));
    }
}
