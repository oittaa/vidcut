use crate::analysis::Loudness;
use crate::error::{Error, Result};
use crate::request::NormalizeMode;
use crate::validate::GAIN;

pub fn usual_target(mode: NormalizeMode) -> f64 {
    match mode {
        NormalizeMode::Loudness => -14.0,
        NormalizeMode::SamplePeak | NormalizeMode::TruePeak => -1.0,
    }
}

pub fn targets(mode: NormalizeMode) -> std::ops::RangeInclusive<f64> {
    match mode {
        NormalizeMode::Loudness => -60.0..=-5.0,
        NormalizeMode::SamplePeak | NormalizeMode::TruePeak => -60.0..=0.0,
    }
}

pub fn unit(mode: NormalizeMode) -> &'static str {
    match mode {
        NormalizeMode::Loudness => "LUFS",
        NormalizeMode::SamplePeak => "dBFS",
        NormalizeMode::TruePeak => "dBTP",
    }
}

pub fn checked_target(mode: NormalizeMode, target: Option<f64>) -> Result<f64> {
    let t = target.unwrap_or_else(|| usual_target(mode));
    let range = targets(mode);
    if !range.contains(&t) {
        return Err(Error::invalid(format!(
            "target {t} {} is outside {}..{}",
            unit(mode),
            range.start(),
            range.end()
        )));
    }
    Ok(t)
}

pub fn level(mode: NormalizeMode, l: &Loudness) -> Option<f64> {
    let v = match mode {
        NormalizeMode::Loudness => l.integrated,
        NormalizeMode::SamplePeak => l.sample_peak,
        NormalizeMode::TruePeak => l.true_peak,
    };
    v.is_finite().then_some(v)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub id: String,
    pub gain_db: f64,
    pub level: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Normalized {
    pub id: String,
    pub level: Option<f64>,
    pub from_db: f64,
    pub gain_db: f64,
    pub held: bool,
}

pub fn gains(clips: &[Measured], target: f64, relative: bool) -> Vec<Normalized> {
    let loudest = clips
        .iter()
        .filter_map(|c| c.level.map(|l| l + c.gain_db))
        .reduce(f64::max);
    clips
        .iter()
        .map(|c| {
            let want = match (relative, loudest, c.level) {
                (true, Some(top), _) => c.gain_db + target - top,
                (false, _, Some(l)) => target - l,
                _ => c.gain_db,
            };
            let gain_db = want.clamp(*GAIN.start(), *GAIN.end());
            Normalized {
                id: c.id.clone(),
                level: c.level,
                from_db: c.gain_db,
                gain_db,
                held: gain_db != want,
            }
        })
        .collect()
}
