use std::ops::RangeInclusive;

use crate::analysis::Loudness;
use crate::error::{Error, Result};
use crate::request::NormalizeMode;
use crate::validate::GAIN;

pub const LOUDNESS: RangeInclusive<f64> = -60.0..=-5.0;
pub const PEAKS: RangeInclusive<f64> = -60.0..=0.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub target: f64,
    pub free_target: bool,
    pub limit: Option<f64>,
    pub free_limit: bool,
}

pub fn rule(mode: NormalizeMode) -> Rule {
    use NormalizeMode as M;
    let (target, free_target, limit, free_limit) = match mode {
        M::SamplePeak | M::TruePeak => (-1.0, true, None, false),
        M::Bs1770v1 => (-23.0, true, None, false),
        M::Bs1770v4 => (-23.0, true, Some(-2.0), true),
        M::AtscA85 | M::Op59 | M::Agcom => (-24.0, false, Some(-2.0), false),
        M::EbuR128 => (-23.0, false, Some(-1.0), true),
        M::TrB32 => (-24.0, false, Some(-1.0), false),
        M::Netflix | M::DisneySurround => (-27.0, false, Some(-2.0), false),
        M::Youtube => (-14.0, false, Some(-1.0), false),
        M::DisneyStereo => (-24.0, false, None, false),
        M::TencentVideo => (-16.0, true, Some(-2.0), true),
    };
    Rule {
        target,
        free_target,
        limit,
        free_limit,
    }
}

pub fn is_peak(mode: NormalizeMode) -> bool {
    matches!(mode, NormalizeMode::SamplePeak | NormalizeMode::TruePeak)
}

pub fn targets(mode: NormalizeMode) -> RangeInclusive<f64> {
    if is_peak(mode) { PEAKS } else { LOUDNESS }
}

pub fn unit(mode: NormalizeMode) -> &'static str {
    match mode {
        NormalizeMode::SamplePeak => "dBFS",
        NormalizeMode::TruePeak => "dBTP",
        _ => "LUFS",
    }
}

fn name(mode: NormalizeMode) -> String {
    serde_json::to_value(mode)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

pub fn checked(
    mode: NormalizeMode,
    target: Option<f64>,
    limit: Option<f64>,
) -> Result<(f64, Option<f64>)> {
    let r = rule(mode);
    let t = match target {
        Some(t) if !r.free_target && t != r.target => {
            return Err(Error::usage(format!(
                "{} fixes the target at {} {}",
                name(mode),
                r.target,
                unit(mode)
            )));
        }
        Some(t) => t,
        None => r.target,
    };
    let range = targets(mode);
    if !range.contains(&t) {
        return Err(Error::invalid(format!(
            "target {t} {} is outside {}..{}",
            unit(mode),
            range.start(),
            range.end()
        )));
    }
    let l = match (limit, r.limit) {
        (Some(_), None) => {
            return Err(Error::usage(format!("{} has no peak limit", name(mode))));
        }
        (Some(x), Some(f)) if !r.free_limit && x != f => {
            return Err(Error::usage(format!(
                "{} fixes the peak limit at {f} dBTP",
                name(mode)
            )));
        }
        (Some(x), Some(_)) => Some(x),
        (None, f) => f,
    };
    if let Some(x) = l
        && !PEAKS.contains(&x)
    {
        return Err(Error::invalid(format!(
            "peak limit {x} dBTP is outside {}..{}",
            PEAKS.start(),
            PEAKS.end()
        )));
    }
    Ok((t, l))
}

pub fn level(mode: NormalizeMode, l: &Loudness) -> Option<f64> {
    let v = match mode {
        NormalizeMode::SamplePeak => l.sample_peak,
        NormalizeMode::TruePeak => l.true_peak,
        _ => l.integrated,
    };
    v.is_finite().then_some(v)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub id: String,
    pub gain_db: f64,
    pub level: Option<f64>,
    pub peak: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Normalized {
    pub id: String,
    pub level: Option<f64>,
    pub from_db: f64,
    pub gain_db: f64,
    pub held: bool,
    pub limited: bool,
}

pub fn gains(
    clips: &[Measured],
    target: f64,
    limit: Option<f64>,
    relative: bool,
) -> Vec<Normalized> {
    let top = |of: fn(&Measured) -> Option<f64>| {
        clips
            .iter()
            .filter_map(|c| of(c).map(|v| v + c.gain_db))
            .reduce(f64::max)
    };
    let (loudest, highest) = (top(|c| c.level), top(|c| c.peak));
    clips
        .iter()
        .map(|c| {
            let (aimed, room) = if relative {
                (
                    loudest.map(|l| c.gain_db + target - l),
                    limit.zip(highest).map(|(x, h)| c.gain_db + x - h),
                )
            } else {
                (
                    c.level.map(|l| target - l),
                    limit.zip(c.peak).map(|(x, p)| x - p),
                )
            };
            let (want, limited) = match (aimed, room) {
                (Some(a), Some(r)) if r < a => (r, true),
                (Some(a), _) => (a, false),
                (None, _) => (c.gain_db, false),
            };
            let gain_db = want.clamp(*GAIN.start(), *GAIN.end());
            Normalized {
                id: c.id.clone(),
                level: c.level,
                from_db: c.gain_db,
                gain_db,
                held: gain_db != want,
                limited,
            }
        })
        .collect()
}
