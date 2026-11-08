use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use vidcut_core::model::NoiseReduction;
use vidcut_core::noise_reduction::gain_of;

use crate::error::Result;

const FRAME_AT_48K: usize = 1024;
const SMOOTHING_BINS: f64 = 8.0;
const NOISE_RISE_DB: f64 = 3.0;
const ATTACK_TO_RISE: f64 = 5.0;
const TIME_SMOOTHING: f64 = 0.7;

fn frame_size(rate: u32) -> usize {
    let wanted = (FRAME_AT_48K as f64 * f64::from(rate) / 48000.0).round() as usize;
    wanted.max(256).next_power_of_two()
}

struct Band {
    tail: VecDeque<f32>,
    overlap: Vec<f32>,
    signal: Vec<f64>,
    noise: Vec<f64>,
    gain: Vec<f64>,
    begun: bool,
}

impl Band {
    fn new(size: usize, hop: usize) -> Self {
        let bins = size / 2 + 1;
        Self {
            tail: std::iter::repeat_n(0.0, size - hop).collect(),
            overlap: vec![0.0; size],
            signal: vec![0.0; bins],
            noise: vec![0.0; bins],
            gain: vec![1.0; bins],
            begun: false,
        }
    }
}

pub(crate) struct Denoiser {
    settings: NoiseReduction,
    channels: usize,
    size: usize,
    hop: usize,
    window: Vec<f32>,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    bands: Vec<Band>,
    spectrum: Vec<Complex32>,
    work: Vec<Complex32>,
    smoothed: Vec<f64>,
    signal_pole: f64,
    noise_rise: f64,
    pulled: Vec<f32>,
    ready: VecDeque<f32>,
    skip: usize,
}

impl Denoiser {
    pub(crate) fn new(settings: NoiseReduction, rate: u32, channels: u16) -> Self {
        let size = frame_size(rate);
        let hop = size / 2;
        let channels = usize::from(channels).max(1);
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        let scratch = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        let work = vec![Complex32::default(); scratch];
        let window = (0..size)
            .map(|i| (PI * (i as f32 + 0.5) / size as f32).sin())
            .collect();
        let hop_ms = hop as f64 * 1000.0 / f64::from(rate);
        let signal_ms = settings.attack_ms / settings.ratio;
        let rise_ms = settings.attack_ms * ATTACK_TO_RISE;
        Self {
            settings,
            channels,
            size,
            hop,
            window,
            forward,
            inverse,
            bands: (0..channels).map(|_| Band::new(size, hop)).collect(),
            spectrum: vec![Complex32::default(); size],
            work,
            smoothed: vec![1.0; size / 2 + 1],
            signal_pole: 1.0 - (-hop_ms / signal_ms).exp(),
            noise_rise: 10f64.powf(NOISE_RISE_DB * hop_ms / (rise_ms * 10.0)),
            pulled: Vec::new(),
            ready: VecDeque::new(),
            skip: (size - hop) * channels,
        }
    }

    fn band_step(&mut self, c: usize) {
        let (size, hop, spread) = (
            self.size,
            self.hop,
            (self.settings.frequency_smoothing / 100.0 * SMOOTHING_BINS).round() as usize,
        );
        let band = &mut self.bands[c];
        for (i, slot) in self.spectrum.iter_mut().enumerate().take(size) {
            let sample = band.tail.get(i).copied().unwrap_or(0.0);
            *slot = Complex32::new(sample * self.window[i], 0.0);
        }
        self.forward
            .process_with_scratch(&mut self.spectrum, &mut self.work);
        let bins = size / 2 + 1;
        for k in 0..bins {
            let power = f64::from(self.spectrum[k].norm_sqr());
            if band.begun {
                band.signal[k] += (power - band.signal[k]) * self.signal_pole;
                band.noise[k] = band.signal[k].min(band.noise[k] * self.noise_rise);
            } else {
                band.signal[k] = power;
                band.noise[k] = power;
            }
            self.smoothed[k] = gain_of(&self.settings, band.signal[k], band.noise[k]);
        }
        band.begun = true;
        let wet = self.settings.dry_wet / 100.0;
        let level = 10f64.powf(self.settings.level_db / 20.0);
        for k in 0..bins {
            let from = k.saturating_sub(spread);
            let to = (k + spread).min(bins - 1);
            let mean = self.smoothed[from..=to].iter().sum::<f64>() / (to - from + 1) as f64;
            let gain = match self.settings.time_smoothing {
                true => band.gain[k] * TIME_SMOOTHING + mean * (1.0 - TIME_SMOOTHING),
                false => mean,
            };
            band.gain[k] = gain;
            let mixed = (1.0 - wet + wet * level * gain) as f32;
            self.spectrum[k] *= mixed;
            if k > 0 && k < size / 2 {
                self.spectrum[size - k] *= mixed;
            }
        }
        self.inverse
            .process_with_scratch(&mut self.spectrum, &mut self.work);
        let scale = 1.0 / size as f32;
        for i in 0..size {
            band.overlap[i] += self.spectrum[i].re * scale * self.window[i];
        }
        band.tail.drain(..hop);
    }

    pub(crate) fn read(
        &mut self,
        n: usize,
        out: &mut Vec<f32>,
        mut pull: impl FnMut(usize, &mut Vec<f32>) -> Result<()>,
    ) -> Result<()> {
        let (ch, hop) = (self.channels, self.hop);
        while self.ready.len() < n * ch {
            self.pulled.clear();
            pull(hop, &mut self.pulled)?;
            self.pulled.resize(hop * ch, 0.0);
            for c in 0..ch {
                let new = self.pulled.iter().skip(c).step_by(ch).copied();
                self.bands[c].tail.extend(new);
                self.band_step(c);
            }
            for i in 0..hop {
                for c in 0..ch {
                    if self.skip > 0 {
                        self.skip -= 1;
                    } else {
                        self.ready.push_back(self.bands[c].overlap[i]);
                    }
                }
            }
            for band in &mut self.bands {
                band.overlap.copy_within(hop.., 0);
                let size = band.overlap.len();
                band.overlap[size - hop..].fill(0.0);
            }
        }
        out.extend(self.ready.drain(..n * ch));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48000;

    fn run(settings: NoiseReduction, input: &[f32]) -> Vec<f32> {
        let mut d = Denoiser::new(settings, RATE, 1);
        let mut at = 0;
        let mut out = Vec::new();
        d.read(
            input.len() / 2,
            &mut out,
            |n: usize, o: &mut Vec<f32>| -> Result<()> {
                let end = (at + n).min(input.len());
                o.extend_from_slice(&input[at..end]);
                at = end;
                Ok(())
            },
        )
        .unwrap();
        out
    }

    fn noise(count: usize) -> Vec<f32> {
        let mut state = 12345u32;
        (0..count)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 8) as f32 / (1u32 << 24) as f32 * 0.02 - 0.01
            })
            .collect()
    }

    fn rms_db(x: &[f32]) -> f64 {
        let p = x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64;
        10.0 * p.log10()
    }

    fn tenths(v: f64) -> f64 {
        (v * 10.0).round() / 10.0
    }

    fn first() -> NoiseReduction {
        vidcut_core::noise_reduction::first()
    }

    #[test]
    fn the_frame_of_44100_is_1024() {
        assert_eq!(frame_size(44100), 1024);
    }

    #[test]
    fn the_frame_of_96000_is_2048() {
        assert_eq!(frame_size(96000), 2048);
    }

    #[test]
    fn dry_sound_passes_unchanged() {
        let input = noise(96000);
        let mut n = first();
        n.dry_wet = 0.0;
        let out = run(n, &input);
        assert_eq!(
            tenths(rms_db(&out[8192..]) - rms_db(&input[8192..48000])),
            0.0
        );
    }

    #[test]
    fn steady_noise_is_lowered() {
        let input = noise(192000);
        let out = run(first(), &input);
        let change = rms_db(&out[48000..]) - rms_db(&input[48000..96000]);
        assert_eq!(tenths(change), -9.8);
    }

    #[test]
    fn level_raises_the_wet_sound() {
        let input = noise(192000);
        let mut n = first();
        n.level_db = 6.0;
        let out = run(n, &input);
        let change = rms_db(&out[48000..]) - rms_db(&input[48000..96000]);
        assert_eq!(tenths(change), -3.8);
    }
}
