use std::f64::consts::PI;
use std::ops::RangeInclusive;

use crate::error::{Error, Result};
use crate::model::{EqBand, EqKind, Equalizer};
use crate::request::{EqBandSet, EqualizerSet};

pub const FREQUENCY: RangeInclusive<f64> = 20.0..=20000.0;
pub const GAIN: RangeInclusive<f64> = -20.0..=20.0;
pub const Q: RangeInclusive<f64> = 0.3..=10.3;
pub const BANDS: usize = 6;
pub const FIRST_Q: f64 = 1.0;
const PASS_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
const NOTCH_Q: f64 = 8.0;
const FIRST_FREQUENCY: [f64; BANDS] = [80.0, 200.0, 800.0, 2500.0, 8000.0, 16000.0];

pub fn kinds(band: usize) -> &'static [EqKind] {
    use EqKind::{Bell, HiPass, HiShelf, LoPass, LoShelf, Notch};
    match band {
        0 => &[LoShelf, Bell, Notch, HiShelf, HiPass],
        5 => &[LoPass, LoShelf, Bell, Notch, HiShelf],
        _ => &[LoShelf, Bell, Notch, HiShelf],
    }
}

pub fn label(kind: EqKind) -> &'static str {
    match kind {
        EqKind::Bell => "Bell",
        EqKind::LoShelf => "Lo-Shelf",
        EqKind::HiShelf => "Hi-Shelf",
        EqKind::Notch => "Notch",
        EqKind::HiPass => "Hi-Pass",
        EqKind::LoPass => "Lo-Pass",
    }
}

pub fn takes_gain(kind: EqKind) -> bool {
    matches!(kind, EqKind::Bell | EqKind::LoShelf | EqKind::HiShelf)
}

pub fn takes_q(kind: EqKind) -> bool {
    kind == EqKind::Bell
}

pub fn first() -> Equalizer {
    let band = |i: usize| EqBand {
        on: (1..BANDS - 1).contains(&i),
        kind: match i {
            0 => EqKind::HiPass,
            5 => EqKind::LoPass,
            _ => EqKind::Bell,
        },
        freq: FIRST_FREQUENCY[i],
        gain: 0.0,
        q: FIRST_Q,
    };
    Equalizer {
        on: true,
        bands: std::array::from_fn(band),
    }
}

fn in_range(name: &str, v: f64, range: &RangeInclusive<f64>) -> Result<()> {
    if range.contains(&v) {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "equalizer {name} {v} is outside {}..{}",
        range.start(),
        range.end()
    )))
}

fn checked_band(i: usize, kind: Option<EqKind>, set: &EqBandSet) -> Result<()> {
    if let Some(k) = kind.filter(|k| !kinds(i).contains(k)) {
        return Err(Error::invalid(format!(
            "equalizer band {} has no {} filter",
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

pub fn check(set: &EqualizerSet) -> Result<()> {
    for b in &set.bands {
        let i = index_of(b)?;
        checked_band(i, b.kind, b)?;
    }
    Ok(())
}

fn index_of(b: &EqBandSet) -> Result<usize> {
    (1..=BANDS as u32)
        .contains(&b.band)
        .then(|| b.band as usize - 1)
        .ok_or_else(|| Error::invalid(format!("no equalizer band {}", b.band)))
}

pub fn patched(old: Option<&Equalizer>, set: &EqualizerSet) -> Result<Equalizer> {
    let mut eq = old.cloned().unwrap_or_else(first);
    if let Some(on) = set.on {
        eq.on = on;
    }
    for b in &set.bands {
        let i = index_of(b)?;
        checked_band(i, b.kind, b)?;
        let band = &mut eq.bands[i];
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
            band.q = v;
        }
    }
    Ok(eq)
}

pub fn problem(eq: &Equalizer) -> Option<String> {
    eq.bands.iter().enumerate().find_map(|(i, b)| {
        let set = EqBandSet {
            band: i as u32 + 1,
            on: None,
            kind: Some(b.kind),
            freq: Some(b.freq),
            gain: Some(b.gain),
            q: Some(b.q),
        };
        checked_band(i, Some(b.kind), &set).err().map(|e| e.message)
    })
}

impl EqBand {
    fn changes_sound(&self) -> bool {
        self.on && (!takes_gain(self.kind) || self.gain != 0.0)
    }
}

impl Equalizer {
    pub fn heard(&self) -> bool {
        self.on && self.bands.iter().any(EqBand::changes_sound)
    }
}

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn of(band: &EqBand, rate: f64) -> Self {
        let w = 2.0 * PI * band.freq.min(rate * 0.49) / rate;
        let (sin, cos) = w.sin_cos();
        let boost = 10f64.powf(band.gain / 40.0);
        let shelf = sin * std::f64::consts::FRAC_1_SQRT_2 * 2.0 * boost.sqrt();
        let (b, a) = match band.kind {
            EqKind::Bell => {
                let alpha = sin / (2.0 * band.q);
                (
                    [1.0 + alpha * boost, -2.0 * cos, 1.0 - alpha * boost],
                    [1.0 + alpha / boost, -2.0 * cos, 1.0 - alpha / boost],
                )
            }
            EqKind::LoShelf => {
                let (up, down) = (boost + 1.0, boost - 1.0);
                (
                    [
                        boost * (up - down * cos + shelf),
                        2.0 * boost * (down - up * cos),
                        boost * (up - down * cos - shelf),
                    ],
                    [
                        up + down * cos + shelf,
                        -2.0 * (down + up * cos),
                        up + down * cos - shelf,
                    ],
                )
            }
            EqKind::HiShelf => {
                let (up, down) = (boost + 1.0, boost - 1.0);
                (
                    [
                        boost * (up + down * cos + shelf),
                        -2.0 * boost * (down + up * cos),
                        boost * (up + down * cos - shelf),
                    ],
                    [
                        up - down * cos + shelf,
                        2.0 * (down - up * cos),
                        up - down * cos - shelf,
                    ],
                )
            }
            EqKind::Notch => {
                let alpha = sin / (2.0 * NOTCH_Q);
                (
                    [1.0, -2.0 * cos, 1.0],
                    [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
                )
            }
            EqKind::LoPass => {
                let alpha = sin / (2.0 * PASS_Q);
                let side = (1.0 - cos) / 2.0;
                (
                    [side, 1.0 - cos, side],
                    [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
                )
            }
            EqKind::HiPass => {
                let alpha = sin / (2.0 * PASS_Q);
                let side = (1.0 + cos) / 2.0;
                (
                    [side, -(1.0 + cos), side],
                    [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
                )
            }
        };
        Self {
            b: [b[0] / a[0], b[1] / a[0], b[2] / a[0]],
            a: [a[1] / a[0], a[2] / a[0]],
        }
    }

    fn magnitude(&self, w: f64) -> f64 {
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let top =
            (self.b[0] + self.b[1] * c1 + self.b[2] * c2).hypot(self.b[1] * s1 + self.b[2] * s2);
        let bottom = (1.0 + self.a[0] * c1 + self.a[1] * c2).hypot(self.a[0] * s1 + self.a[1] * s2);
        top / bottom
    }

    fn step(&self, x: f64, memory: &mut [f64; 2]) -> f64 {
        let y = self.b[0] * x + memory[0];
        memory[0] = self.b[1] * x - self.a[0] * y + memory[1];
        memory[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

fn stages(eq: &Equalizer, rate: f64) -> Vec<Biquad> {
    if !eq.on {
        return Vec::new();
    }
    eq.bands
        .iter()
        .filter(|b| b.changes_sound())
        .map(|b| Biquad::of(b, rate))
        .collect()
}

pub fn response_db(eq: &Equalizer, freq: f64, rate: u32) -> f64 {
    let rate = f64::from(rate);
    let w = 2.0 * PI * freq / rate;
    let gain: f64 = stages(eq, rate).iter().map(|s| s.magnitude(w)).product();
    20.0 * gain.max(1e-6).log10()
}

#[derive(Debug, Clone)]
pub struct Filter {
    settings: Equalizer,
    stages: Vec<Biquad>,
    memory: Vec<[f64; 2]>,
    channels: usize,
}

impl Filter {
    pub fn new(eq: &Equalizer, rate: u32, channels: u16) -> Self {
        let stages = stages(eq, f64::from(rate));
        let channels = usize::from(channels).max(1);
        Self {
            settings: eq.clone(),
            memory: vec![[0.0; 2]; stages.len() * channels],
            stages,
            channels,
        }
    }

    pub fn settings(&self) -> &Equalizer {
        &self.settings
    }

    pub fn run(&mut self, samples: &mut [f32]) {
        for frame in samples.chunks_exact_mut(self.channels) {
            for (c, sample) in frame.iter_mut().enumerate() {
                let mut x = f64::from(*sample);
                for (i, stage) in self.stages.iter().enumerate() {
                    x = stage.step(x, &mut self.memory[i * self.channels + c]);
                }
                *sample = x as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(i: usize, kind: EqKind, freq: f64, gain: f64, q: f64) -> Equalizer {
        let mut eq = first();
        for b in &mut eq.bands {
            b.on = false;
        }
        eq.bands[i] = EqBand {
            on: true,
            kind,
            freq,
            gain,
            q,
        };
        eq
    }

    fn tenths(v: f64) -> f64 {
        (v * 10.0).round() / 10.0
    }

    fn set(band: u32) -> EqBandSet {
        EqBandSet {
            band,
            on: None,
            kind: None,
            freq: None,
            gain: None,
            q: None,
        }
    }

    #[test]
    fn a_new_equalizer_is_flat() {
        assert_eq!(response_db(&first(), 1000.0, 48000), 0.0);
    }

    #[test]
    fn a_new_equalizer_is_not_heard() {
        assert!(!first().heard());
    }

    #[test]
    fn band_one_starts_off() {
        assert!(!first().bands[0].on);
    }

    #[test]
    fn band_three_starts_on() {
        assert!(first().bands[2].on);
    }

    #[test]
    fn a_bell_has_its_gain_at_its_frequency() {
        let eq = band(2, EqKind::Bell, 1000.0, 6.0, 1.0);
        assert_eq!(tenths(response_db(&eq, 1000.0, 48000)), 6.0);
    }

    #[test]
    fn a_bell_leaves_a_far_frequency_alone() {
        let eq = band(2, EqKind::Bell, 1000.0, 6.0, 4.0);
        assert_eq!(tenths(response_db(&eq, 8000.0, 48000)), 0.0);
    }

    #[test]
    fn a_low_shelf_lifts_the_low_end_by_its_gain() {
        let eq = band(1, EqKind::LoShelf, 1000.0, 6.0, 1.0);
        assert_eq!(tenths(response_db(&eq, 25.0, 48000)), 6.0);
    }

    #[test]
    fn a_high_shelf_cuts_the_high_end_by_its_gain() {
        let eq = band(4, EqKind::HiShelf, 1000.0, -6.0, 1.0);
        assert_eq!(tenths(response_db(&eq, 20000.0, 48000)), -6.0);
    }

    #[test]
    fn a_high_pass_is_three_db_down_at_its_frequency() {
        let eq = band(0, EqKind::HiPass, 100.0, 0.0, 1.0);
        assert_eq!(tenths(response_db(&eq, 100.0, 48000)), -3.0);
    }

    #[test]
    fn a_low_pass_is_three_db_down_at_its_frequency() {
        let eq = band(5, EqKind::LoPass, 4000.0, 0.0, 1.0);
        assert_eq!(tenths(response_db(&eq, 4000.0, 48000)), -3.0);
    }

    #[test]
    fn a_notch_removes_its_frequency() {
        let eq = band(2, EqKind::Notch, 1000.0, 0.0, 1.0);
        assert_eq!(response_db(&eq, 1000.0, 48000), -120.0);
    }

    #[test]
    fn a_switched_off_equalizer_changes_nothing() {
        let mut eq = band(2, EqKind::Bell, 1000.0, 6.0, 1.0);
        eq.on = false;
        assert_eq!(response_db(&eq, 1000.0, 48000), 0.0);
    }

    #[test]
    fn a_band_at_zero_gain_is_not_heard() {
        assert!(!band(2, EqKind::Bell, 1000.0, 0.0, 1.0).heard());
    }

    #[test]
    fn a_notch_is_heard_at_any_gain() {
        assert!(band(2, EqKind::Notch, 1000.0, 0.0, 1.0).heard());
    }

    #[test]
    fn a_filter_with_no_band_leaves_the_samples_alone() {
        let mut samples = [0.25f32, -0.5];
        Filter::new(&first(), 48000, 2).run(&mut samples);
        assert_eq!(samples, [0.25, -0.5]);
    }

    #[test]
    fn a_filter_keeps_a_memory_for_each_channel() {
        let eq = band(2, EqKind::Bell, 1000.0, 6.0, 1.0);
        let mut f = Filter::new(&eq, 48000, 2);
        let mut samples = [1.0f32, 0.0, 0.0, 0.0];
        f.run(&mut samples);
        assert_eq!(samples[1], 0.0);
    }

    #[test]
    fn band_one_refuses_a_low_pass() {
        let mut b = set(1);
        b.kind = Some(EqKind::LoPass);
        let s = EqualizerSet {
            on: None,
            bands: vec![b],
        };
        assert!(check(&s).is_err());
    }

    #[test]
    fn band_three_refuses_a_high_pass() {
        let mut b = set(3);
        b.kind = Some(EqKind::HiPass);
        let s = EqualizerSet {
            on: None,
            bands: vec![b],
        };
        assert!(check(&s).is_err());
    }

    #[test]
    fn a_gain_past_twenty_db_is_refused() {
        let mut b = set(2);
        b.gain = Some(20.5);
        let s = EqualizerSet {
            on: None,
            bands: vec![b],
        };
        assert!(check(&s).is_err());
    }

    #[test]
    fn a_band_seven_is_refused() {
        let s = EqualizerSet {
            on: None,
            bands: vec![set(7)],
        };
        assert!(check(&s).is_err());
    }

    #[test]
    fn a_patch_changes_only_the_fields_given() {
        let mut b = set(2);
        b.gain = Some(-4.0);
        let s = EqualizerSet {
            on: None,
            bands: vec![b],
        };
        let eq = patched(Some(&first()), &s).unwrap();
        assert_eq!((eq.bands[1].gain, eq.bands[1].freq), (-4.0, 200.0));
    }

    #[test]
    fn a_first_patch_starts_on() {
        let s = EqualizerSet {
            on: None,
            bands: Vec::new(),
        };
        assert!(patched(None, &s).unwrap().on);
    }

    #[test]
    fn a_stored_band_outside_the_rules_is_a_problem() {
        let mut eq = first();
        eq.bands[2].kind = EqKind::HiPass;
        assert_eq!(
            problem(&eq),
            Some("equalizer band 3 has no Hi-Pass filter".to_string())
        );
    }
}
