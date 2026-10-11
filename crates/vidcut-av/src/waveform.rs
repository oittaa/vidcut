//! Audio peak envelopes for drawing waveforms (spec §5: clips show their
//! audio as a waveform band).

use std::path::Path;

use crate::error::{Context, Error, Result};
use crate::export::audio::{Resampler, discard_others};
use crate::ffmpeg::{codec, format, frame, media};

/// Peaks of one audio stream, mixed to mono: for each bucket of
/// `1 / per_second` seconds, the lowest and highest sample (scaled to ±127).
#[derive(Debug, Clone, PartialEq)]
pub struct Waveform {
    pub per_second: u32,
    /// Raw timestamp (s) of the first bucket's start.
    pub start: f64,
    pub peaks: Vec<(i8, i8)>,
}

impl Waveform {
    /// Peak range over `[a, b)` seconds (raw time), as fractions of full scale.
    pub fn range(&self, a: f64, b: f64) -> Option<(f32, f32)> {
        let rate = f64::from(self.per_second);
        let i = ((a - self.start) * rate).floor().max(0.0) as usize;
        let j = (((b - self.start) * rate).ceil().max(0.0) as usize).min(self.peaks.len());
        if i >= j {
            return None;
        }
        let (lo, hi) = self.peaks[i..j]
            .iter()
            .fold((i8::MAX, i8::MIN), |(lo, hi), &(a, b)| {
                (lo.min(a), hi.max(b))
            });
        Some((f32::from(lo) / 127.0, f32::from(hi) / 127.0))
    }

    /// A compact binary form for caching: `per_second`, `start`, peaks.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.peaks.len() * 2);
        out.extend_from_slice(&self.per_second.to_le_bytes());
        out.extend_from_slice(&self.start.to_le_bytes());
        for &(lo, hi) in &self.peaks {
            out.push(lo as u8);
            out.push(hi as u8);
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 12 || !(b.len() - 12).is_multiple_of(2) {
            return None;
        }
        Some(Self {
            per_second: u32::from_le_bytes(b[0..4].try_into().ok()?),
            start: f64::from_le_bytes(b[4..12].try_into().ok()?),
            peaks: b[12..]
                .chunks_exact(2)
                .map(|p| (p[0] as i8, p[1] as i8))
                .collect(),
        })
    }
}

/// Decodes audio stream `ordinal` of `path` (counting audio streams only)
/// and returns its peaks at `per_second` buckets per second.
pub fn waveform(path: &Path, ordinal: u32, per_second: u32) -> Result<Waveform> {
    crate::init();
    let mut input = format::input(path).ctx(|| format!("opening {}", path.display()))?;
    let stream = input
        .streams()
        .filter(|s| s.parameters().medium() == media::Type::Audio)
        .nth(ordinal as usize)
        .ok_or_else(|| Error::Invalid(format!("{}: no audio stream {ordinal}", path.display())))?;
    let (index, tb) = (stream.index(), stream.time_base());
    let tick = f64::from(tb);
    discard_others(&input, index);
    let mut decoder = codec::context::Context::from_parameters(stream.parameters())
        .and_then(|mut c| {
            unsafe { (*c.as_mut_ptr()).pkt_timebase = tb.into() };
            c.decoder().audio()
        })
        .ctx(|| "opening audio decoder".into())?;
    let mut resampler: Option<Resampler> = None;
    let mut mono = Vec::new();
    let mut acc = Accumulator::new(per_second);
    let mut start: Option<f64> = None;
    let mut handle = |f: &frame::Audio, acc: &mut Accumulator| -> Result<()> {
        if start.is_none() {
            start = f.timestamp().map(|t| t as f64 * tick);
        }
        if resampler.as_ref().is_none_or(|r| !r.matches(f)) {
            resampler = Some(Resampler::new(f, f.rate(), 1)?);
            acc.rate = f.rate();
        }
        mono.clear();
        resampler
            .as_mut()
            .expect("set")
            .convert(Some(f), &mut mono)?;
        acc.push(&mono);
        Ok(())
    };
    let mut f = frame::Audio::empty();
    for (s, packet) in input.packets() {
        if s.index() != index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue; // a broken packet: skip it
        }
        while decoder.receive_frame(&mut f).is_ok() {
            handle(&f, &mut acc)?;
        }
    }
    let _ = decoder.send_eof();
    while decoder.receive_frame(&mut f).is_ok() {
        handle(&f, &mut acc)?;
    }
    Ok(Waveform {
        per_second,
        start: start.unwrap_or(0.0),
        peaks: acc.finish(),
    })
}

struct Accumulator {
    per_second: u32,
    rate: u32,
    /// Samples taken so far.
    n: u64,
    lo: f32,
    hi: f32,
    peaks: Vec<(i8, i8)>,
}

impl Accumulator {
    fn new(per_second: u32) -> Self {
        Self {
            per_second: per_second.max(1),
            rate: 48_000,
            n: 0,
            lo: 0.0,
            hi: 0.0,
            peaks: Vec::new(),
        }
    }

    fn bucket(&self, n: u64) -> usize {
        (n * u64::from(self.per_second) / u64::from(self.rate.max(1))) as usize
    }

    fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            let b = self.bucket(self.n);
            while self.peaks.len() < b {
                self.flush();
            }
            self.lo = self.lo.min(s);
            self.hi = self.hi.max(s);
            self.n += 1;
        }
    }

    fn flush(&mut self) {
        let q = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
        self.peaks.push((q(self.lo), q(self.hi)));
        self.lo = 0.0;
        self.hi = 0.0;
    }

    fn finish(mut self) -> Vec<(i8, i8)> {
        if self.n > 0 && self.peaks.len() <= self.bucket(self.n - 1) {
            self.flush();
        }
        self.peaks
    }
}
