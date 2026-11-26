use std::ops::RangeInclusive;

use crate::dynamics::in_range;
use crate::error::Result;
use crate::model::{ClipCompressor, Compressor};
use crate::request::CompressorSet;

pub const THRESHOLD_DB: RangeInclusive<f64> = -50.0..=0.0;
pub const RATIO: RangeInclusive<f64> = 1.2..=20.0;
pub const KNEE: RangeInclusive<f64> = 0.0..=100.0;
pub const MIX: RangeInclusive<f64> = 0.0..=100.0;
pub const ATTACK_MS: RangeInclusive<f64> = 0.7..=100.0;
pub const HOLD_MS: RangeInclusive<f64> = 0.0..=4000.0;
pub const RELEASE_MS: RangeInclusive<f64> = 50.0..=4000.0;
pub const MAKEUP_DB: RangeInclusive<f64> = 0.0..=24.0;

pub fn module() -> Compressor {
    Compressor {
        on: false,
        threshold_db: -15.0,
        ratio: 2.0,
        knee: 0.0,
        mix: 0.0,
        attack_ms: 1.4,
        hold_ms: 0.0,
        release_ms: 93.0,
    }
}

pub fn first() -> ClipCompressor {
    ClipCompressor {
        compressor: Compressor {
            on: true,
            ..module()
        },
        makeup_db: 0.0,
    }
}

pub(crate) fn checked(c: &Compressor) -> Result<()> {
    in_range("compressor", "threshold", c.threshold_db, &THRESHOLD_DB)?;
    in_range("compressor", "ratio", c.ratio, &RATIO)?;
    in_range("compressor", "knee", c.knee, &KNEE)?;
    in_range("compressor", "mix", c.mix, &MIX)?;
    in_range("compressor", "attack", c.attack_ms, &ATTACK_MS)?;
    in_range("compressor", "hold", c.hold_ms, &HOLD_MS)?;
    in_range("compressor", "release", c.release_ms, &RELEASE_MS)
}

fn clip_checked(c: &ClipCompressor) -> Result<()> {
    checked(&c.compressor)?;
    in_range("compressor", "makeup gain", c.makeup_db, &MAKEUP_DB)
}

pub(crate) fn apply(mut c: Compressor, set: &CompressorSet) -> Compressor {
    c.on = set.on.unwrap_or(c.on);
    c.threshold_db = set.threshold_db.unwrap_or(c.threshold_db);
    c.ratio = set.ratio.unwrap_or(c.ratio);
    c.knee = set.knee.unwrap_or(c.knee);
    c.mix = set.mix.unwrap_or(c.mix);
    c.attack_ms = set.attack_ms.unwrap_or(c.attack_ms);
    c.hold_ms = set.hold_ms.unwrap_or(c.hold_ms);
    c.release_ms = set.release_ms.unwrap_or(c.release_ms);
    c
}

fn clip_applied(c: ClipCompressor, set: &CompressorSet) -> ClipCompressor {
    ClipCompressor {
        compressor: apply(c.compressor, set),
        makeup_db: set.makeup_db.unwrap_or(c.makeup_db),
    }
}

pub fn check(set: &CompressorSet) -> Result<()> {
    clip_checked(&clip_applied(first(), set))
}

pub fn patched(old: Option<ClipCompressor>, set: &CompressorSet) -> Result<ClipCompressor> {
    let c = clip_applied(old.unwrap_or_else(first), set);
    clip_checked(&c)?;
    Ok(c)
}

pub fn problem(c: &ClipCompressor) -> Option<String> {
    clip_checked(c).err().map(|e| e.message)
}

impl ClipCompressor {
    pub fn heard(&self) -> bool {
        self.compressor.on && self.compressor.mix < 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamics::compressed_db;

    const RATE: u32 = 48000;

    fn with(change: impl FnOnce(&mut ClipCompressor)) -> ClipCompressor {
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

    fn change_db(c: ClipCompressor, amplitude: f32) -> f64 {
        let dry = tone(48000, amplitude);
        let mut wet = dry.clone();
        crate::dynamics::Dynamics::clip(&c, RATE, 1).run(&mut wet);
        tenths(rms_db(&wet[24000..]) - rms_db(&dry[24000..]))
    }

    #[test]
    fn a_level_below_the_threshold_is_not_reduced() {
        assert_eq!(compressed_db(&first().compressor, -20.0), 0.0);
    }

    #[test]
    fn a_level_above_the_threshold_is_reduced_by_the_ratio() {
        assert_eq!(compressed_db(&first().compressor, -5.0), -5.0);
    }

    #[test]
    fn a_soft_knee_reduces_the_level_at_the_threshold() {
        let soft = with(|c| c.compressor.knee = 50.0);
        assert!(compressed_db(&soft.compressor, -15.0) < 0.0);
    }

    #[test]
    fn a_new_compressor_is_heard() {
        assert!(first().heard());
    }

    #[test]
    fn an_off_compressor_is_not_heard() {
        assert!(!with(|c| c.compressor.on = false).heard());
    }

    #[test]
    fn a_mix_of_one_hundred_is_not_heard() {
        assert!(!with(|c| c.compressor.mix = 100.0).heard());
    }

    #[test]
    fn a_steady_tone_is_reduced_by_the_curve() {
        assert_eq!(change_db(with(|c| c.compressor.attack_ms = 0.0), 0.5), -4.5);
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
            c.compressor.attack_ms = 0.0;
            c.compressor.mix = 50.0;
        });
        assert_eq!(change_db(halved, 0.5), -2.0);
    }

    #[test]
    fn a_ratio_past_twenty_is_refused() {
        let set = CompressorSet {
            ratio: Some(21.0),
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
        assert_eq!(
            (c.compressor.ratio, c.compressor.threshold_db),
            (4.0, -15.0)
        );
    }
}
