use std::f64::consts::PI;
use std::ops::RangeInclusive;

use super::{BANDS, Biquad, Curve, FREQUENCY, in_range, index_of, label, warped};
use crate::error::{Error, Result};
use crate::model::{EqBand, EqKind, EqType, TrackEqualizer};
use crate::request::{EqBandSet, TrackEqualizerSet};

pub const GAIN: RangeInclusive<f64> = -20.0..=20.0;
pub const Q: RangeInclusive<f64> = 0.3..=10.3;
pub const SLOPES: [u32; 4] = [6, 12, 18, 24];
pub const TYPES: [EqType; 4] = [EqType::Earth, EqType::Air, EqType::Ice, EqType::Fire];
const FIRST_FREQUENCY: [f64; BANDS] = [70.0, 118.0, 240.0, 1223.0, 6048.0, 22000.0];
const FIRST_KIND: [EqKind; BANDS] = [
    EqKind::HiPass,
    EqKind::LoShelf,
    EqKind::Bell,
    EqKind::Bell,
    EqKind::HiShelf,
    EqKind::LoPass,
];
const BELL_Q: f64 = 16.0 / 15.0;
const SHELF_Q: f64 = 7.0 / 3.0;
const SLOPE: u32 = 12;
const STEP: f32 = 15.0;
const LOWEST_STEP: f32 = 15.0;
const ZERO_STEP: f64 = 10.0;
const NOTCH_DEPTH: f64 = 20.0;
const NOTCH_WIDTH: f64 = 30.0;
const FIRE_SHELF_Q: (f64, f64) = (0.43, 0.004667);

#[derive(Debug, Clone, Copy)]
struct Shelf {
    gain: f64,
    zeros: f64,
}

#[derive(Debug, Clone, Copy)]
struct Response {
    bell_gain: f64,
    bell_steps: f64,
    shelf_q: Option<f64>,
    lo_shelf: Shelf,
    hi_shelf: Shelf,
    hi_pass_q: f64,
    lo_pass_q: f64,
    slopes: bool,
}

fn response(kind: EqType) -> Response {
    let shelf = |gain, zeros| Shelf { gain, zeros };
    match kind {
        EqType::Earth => Response {
            bell_gain: 1.0,
            bell_steps: 30.0,
            shelf_q: Some(0.707),
            lo_shelf: shelf(1.0, 1.0),
            hi_shelf: shelf(1.0, 1.0),
            hi_pass_q: 0.707,
            lo_pass_q: 0.707,
            slopes: false,
        },
        EqType::Air => Response {
            bell_gain: 1.0,
            bell_steps: 30.0,
            shelf_q: Some(0.5),
            lo_shelf: shelf(1.0, 1.0),
            hi_shelf: shelf(1.0, 7.0 / 8.0),
            hi_pass_q: 0.85,
            lo_pass_q: 0.4,
            slopes: false,
        },
        EqType::Ice => Response {
            bell_gain: 10.0 / 9.0,
            bell_steps: 24.0,
            shelf_q: Some(0.5),
            lo_shelf: shelf(20.0 / 17.0, 1.15),
            hi_shelf: shelf(20.0 / 17.0, 1.0 / 1.15),
            hi_pass_q: 0.95,
            lo_pass_q: 0.9,
            slopes: false,
        },
        EqType::Fire => Response {
            bell_gain: 50.0 / 37.0,
            bell_steps: 30.0,
            shelf_q: None,
            lo_shelf: shelf(40.0 / 33.0, 1.0),
            hi_shelf: shelf(8.0 / 7.0, 1.0),
            hi_pass_q: 0.95,
            lo_pass_q: 0.707,
            slopes: true,
        },
    }
}

fn step_of(q: f64) -> f64 {
    let above = (q as f32 - *Q.start() as f32) * STEP;
    f64::from((above + LOWEST_STEP).floor())
}

pub fn stepped(q: f64) -> f64 {
    ((step_of(q) - ZERO_STEP) / f64::from(STEP)).min(*Q.end())
}

fn peak(freq: f64, rate: f64, a: f64, width: f64) -> Biquad {
    let x = PI * freq.min(rate * 0.49) / rate;
    let w = x.tan();
    let q = w / (2.0 * (w - (x * (1.0 - 0.5 / width)).tan()));
    Biquad::analog([1.0, a * w / q, w * w], [1.0, w / (a * q), w * w])
}

pub fn label_of(kind: EqType) -> &'static str {
    match kind {
        EqType::Earth => "Earth",
        EqType::Air => "Air",
        EqType::Ice => "Ice",
        EqType::Fire => "Fire",
    }
}

pub fn kinds(band: usize) -> &'static [EqKind] {
    match band {
        0 => &[
            EqKind::LoShelf,
            EqKind::Bell,
            EqKind::HiShelf,
            EqKind::HiPass,
        ],
        5 => &[
            EqKind::LoPass,
            EqKind::LoShelf,
            EqKind::Bell,
            EqKind::HiShelf,
        ],
        _ => &[
            EqKind::LoShelf,
            EqKind::Bell,
            EqKind::Notch,
            EqKind::HiShelf,
        ],
    }
}

pub fn takes_q(kind: EqKind, of: EqType) -> bool {
    match kind {
        EqKind::Bell => true,
        EqKind::LoShelf | EqKind::HiShelf => of == EqType::Fire,
        EqKind::Notch | EqKind::HiPass | EqKind::LoPass => false,
    }
}

pub fn takes_slope(kind: EqKind, of: EqType) -> bool {
    matches!(kind, EqKind::HiPass | EqKind::LoPass) && response(of).slopes
}

pub fn first() -> TrackEqualizer {
    TrackEqualizer {
        on: true,
        r#type: EqType::Earth,
        bands: std::array::from_fn(|i| EqBand {
            on: (1..BANDS - 1).contains(&i),
            kind: FIRST_KIND[i],
            freq: FIRST_FREQUENCY[i],
            gain: 0.0,
            q: match FIRST_KIND[i] {
                EqKind::LoShelf | EqKind::HiShelf => SHELF_Q,
                _ => BELL_Q,
            },
        }),
        hi_pass_slope: SLOPE,
        lo_pass_slope: SLOPE,
    }
}

pub fn usual() -> TrackEqualizer {
    TrackEqualizer {
        on: false,
        ..first()
    }
}

fn checked_band(i: usize, kind: Option<EqKind>, set: &EqBandSet) -> Result<()> {
    if let Some(k) = kind.filter(|k| !kinds(i).contains(k)) {
        return Err(Error::invalid(format!(
            "track equalizer band {} has no {} filter",
            i + 1,
            label(k)
        )));
    }
    if let Some(v) = set.freq {
        in_range("frequency", v, &FREQUENCY)?;
    }
    if let Some(v) = set.gain {
        in_range("gain", v, &GAIN)?;
    }
    if let Some(v) = set.q {
        in_range("Q", v, &Q)?;
    }
    Ok(())
}

fn checked_slope(name: &str, slope: Option<u32>) -> Result<()> {
    match slope {
        Some(v) if !SLOPES.contains(&v) => Err(Error::invalid(format!(
            "{name} is {v}: give 6, 12, 18 or 24 dB per octave"
        ))),
        _ => Ok(()),
    }
}

pub fn check(set: &TrackEqualizerSet) -> Result<()> {
    for b in &set.bands {
        checked_band(index_of(b)?, b.kind, b)?;
    }
    checked_slope("hi_pass_slope", set.hi_pass_slope)?;
    checked_slope("lo_pass_slope", set.lo_pass_slope)
}

pub fn patched(old: Option<&TrackEqualizer>, set: &TrackEqualizerSet) -> Result<TrackEqualizer> {
    check(set)?;
    let mut eq = old.cloned().unwrap_or_else(first);
    if let Some(on) = set.on {
        eq.on = on;
    }
    if let Some(kind) = set.r#type {
        eq.r#type = kind;
    }
    if let Some(v) = set.hi_pass_slope {
        eq.hi_pass_slope = v;
    }
    if let Some(v) = set.lo_pass_slope {
        eq.lo_pass_slope = v;
    }
    for b in &set.bands {
        let band = &mut eq.bands[index_of(b)?];
        if let Some(on) = b.on {
            band.on = on;
        }
        if let Some(kind) = b.kind {
            band.kind = kind;
        }
        if let Some(v) = b.freq {
            band.freq = v;
        }
        if let Some(v) = b.gain {
            band.gain = v;
        }
        if let Some(v) = b.q {
            band.q = stepped(v);
        }
    }
    Ok(eq)
}

pub fn problem(eq: &TrackEqualizer) -> Option<String> {
    let set = TrackEqualizerSet {
        on: None,
        r#type: None,
        bands: eq
            .bands
            .iter()
            .enumerate()
            .map(|(i, b)| EqBandSet {
                band: i as u32 + 1,
                on: None,
                kind: Some(b.kind),
                freq: Some(b.freq),
                gain: Some(b.gain),
                q: Some(b.q),
            })
            .collect(),
        hi_pass_slope: Some(eq.hi_pass_slope),
        lo_pass_slope: Some(eq.lo_pass_slope),
    };
    check(&set).err().map(|e| e.message)
}

impl TrackEqualizer {
    pub fn heard(&self) -> bool {
        self.on && self.bands.iter().any(EqBand::changes_sound)
    }
}

fn passes(high: bool, w: f64, q: f64, slope: u32, out: &mut Vec<Biquad>) {
    for _ in 0..slope / 12 {
        out.push(match high {
            true => Biquad::analog([1.0, 0.0, 0.0], [1.0, w / q, w * w]),
            false => Biquad::analog([0.0, 0.0, w * w], [1.0, w / q, w * w]),
        });
    }
    if !slope.is_multiple_of(12) {
        out.push(match high {
            true => Biquad::analog_first([1.0, 0.0], [1.0, w]),
            false => Biquad::analog_first([0.0, w], [1.0, w]),
        });
    }
}

fn band_stages(eq: &TrackEqualizer, band: &EqBand, rate: f64, out: &mut Vec<Biquad>) {
    let r = response(eq.r#type);
    let slope = |slope| if r.slopes { slope } else { SLOPE };
    match band.kind {
        EqKind::Bell => {
            let a = 10f64.powf(band.gain * r.bell_gain / 40.0);
            out.push(peak(band.freq, rate, a, step_of(band.q) / r.bell_steps));
        }
        EqKind::Notch => out.push(peak(
            band.freq,
            rate,
            NOTCH_DEPTH.recip().sqrt(),
            NOTCH_WIDTH,
        )),
        EqKind::LoShelf | EqKind::HiShelf => {
            let low = band.kind == EqKind::LoShelf;
            let s = if low { r.lo_shelf } else { r.hi_shelf };
            let q = r
                .shelf_q
                .unwrap_or(FIRE_SHELF_Q.0 + FIRE_SHELF_Q.1 * step_of(band.q));
            let g = 10f64.powf(band.gain * s.gain / 20.0);
            let step = g.max(1.0 / g).sqrt();
            let w = warped(band.freq * s.zeros, rate);
            let (top, bottom, k) = match (low, g >= 1.0) {
                (true, true) => (w, w / step, 1.0),
                (true, false) => (w / step, w, 1.0),
                (false, true) => (w, w * step, g),
                (false, false) => (w * step, w, g),
            };
            out.push(Biquad::analog(
                [k, k * top / q, k * top * top],
                [1.0, bottom / q, bottom * bottom],
            ));
        }
        EqKind::HiPass => passes(
            true,
            warped(band.freq, rate),
            r.hi_pass_q,
            slope(eq.hi_pass_slope),
            out,
        ),
        EqKind::LoPass => passes(
            false,
            warped(band.freq, rate),
            r.lo_pass_q,
            slope(eq.lo_pass_slope),
            out,
        ),
    }
}

impl Curve for TrackEqualizer {
    fn stages(&self, rate: f64) -> Vec<Biquad> {
        let mut out = Vec::new();
        if self.on {
            for band in self.bands.iter().filter(|b| b.changes_sound()) {
                band_stages(self, band, rate, &mut out);
            }
        }
        out
    }
}
