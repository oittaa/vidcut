//! Audio peak envelopes for drawing waveforms (spec §5: clips show their
//! audio as a waveform band).

use std::path::Path;

use crate::decode::decode_stream;
use crate::error::{Context, Error, Result};
use crate::export::audio::{Resampler, discard_others};
use crate::ffmpeg::{codec, format, frame, media};

/// Peaks of one audio stream: a mono downmix for drawing, and one envelope
/// per meter channel (spec D28), the stream mixed to that many channels as
/// the export mixes it.
///
/// For each bucket of `1 / per_second` seconds, the lowest and highest
/// sample, scaled to ±127.
#[derive(Debug, Clone, PartialEq)]
pub struct Waveform {
    pub per_second: u32,
    /// Raw timestamp (s) of the first bucket's start.
    pub start: f64,
    /// Mono downmix, one `(min, max)` per bucket. The timeline draws this.
    pub peaks: Vec<(i8, i8)>,
    /// Channels stored in `channel_peaks`.
    pub channels: u16,
    /// Per-channel peaks, bucket-major: index `bucket * channels + channel`.
    pub channel_peaks: Vec<(i8, i8)>,
}

impl Waveform {
    /// Peak range over `[a, b)` seconds (raw time), as fractions of full scale.
    pub fn range(&self, a: f64, b: f64) -> Option<(f32, f32)> {
        let (i, j) = self.buckets(a, b, self.peaks.len());
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

    /// `(min, max)` of one channel over `[a, b)` seconds, as fractions of
    /// full scale. `None` when `channel` is out of range or the span holds
    /// no buckets.
    pub fn channel_range(&self, channel: usize, a: f64, b: f64) -> Option<(f32, f32)> {
        let ch = usize::from(self.channels);
        if ch == 0 || channel >= ch {
            return None;
        }
        let buckets = self.channel_peaks.len() / ch;
        let (i, j) = self.buckets(a, b, buckets);
        if i >= j {
            return None;
        }
        let (lo, hi) = (i..j).fold((i8::MAX, i8::MIN), |(lo, hi), bucket| {
            let (a, b) = self.channel_peaks[bucket * ch + channel];
            (lo.min(a), hi.max(b))
        });
        Some((f32::from(lo) / 127.0, f32::from(hi) / 127.0))
    }

    /// Linear peak (0..1) of one channel over `[a, b)`. Silence when that
    /// span has no buckets.
    pub fn channel_peak(&self, channel: usize, a: f64, b: f64) -> f32 {
        self.channel_range(channel, a, b)
            .map(|(lo, hi)| lo.abs().max(hi.abs()))
            .unwrap_or(0.0)
    }

    fn buckets(&self, a: f64, b: f64, len: usize) -> (usize, usize) {
        let rate = f64::from(self.per_second);
        let i = ((a - self.start) * rate).floor().max(0.0) as usize;
        let j = (((b - self.start) * rate).ceil().max(0.0) as usize).min(len);
        (i, j)
    }

    /// A compact binary form for caching. A leading `VCW2` marks per-channel
    /// peaks; older mono caches fail here and are measured again.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + (self.peaks.len() + self.channel_peaks.len()) * 2);
        out.extend_from_slice(b"VCW2");
        out.extend_from_slice(&self.per_second.to_le_bytes());
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.start.to_le_bytes());
        for &(lo, hi) in self.peaks.iter().chain(&self.channel_peaks) {
            out.push(lo as u8);
            out.push(hi as u8);
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let b = b.strip_prefix(b"VCW2")?;
        if b.len() < 16 || !(b.len() - 16).is_multiple_of(2) {
            return None;
        }
        let channels = u16::from_le_bytes(b[4..6].try_into().ok()?);
        let ch = usize::from(channels);
        let pairs: Vec<(i8, i8)> = b[16..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[lo, hi]| (lo as i8, hi as i8))
            .collect();
        if ch == 0 || !pairs.len().is_multiple_of(1 + ch) {
            return None;
        }
        let buckets = pairs.len() / (1 + ch);
        let (peaks, channel_peaks) = pairs.split_at(buckets);
        Some(Self {
            per_second: u32::from_le_bytes(b[0..4].try_into().ok()?),
            start: f64::from_le_bytes(b[8..16].try_into().ok()?),
            peaks: peaks.to_vec(),
            channels,
            channel_peaks: channel_peaks.to_vec(),
        })
    }
}

/// Decodes audio stream `ordinal` of `path` (counting audio streams only)
/// and returns its peaks at `per_second` buckets per second, with an
/// envelope for each of `channels` (the project's; at most 8).
pub fn waveform(path: &Path, ordinal: u32, per_second: u32, channels: u16) -> Result<Waveform> {
    // The export's resampler, so a 5.1 source's centre and surrounds land in
    // a stereo meter's left and right as they do in the export.
    let want = channels.clamp(1, 8);
    crate::init();
    let mut input = format::input(path).ctx(|| format!("opening {}", path.display()))?;
    let stream = input
        .streams()
        .filter(|s| s.parameters().medium() == media::Type::Audio)
        .nth(ordinal as usize)
        .ok_or_else(|| Error::Invalid(format!("{}: no audio stream {ordinal}", path.display())))?;
    let (index, tb, start_pts) = (stream.index(), stream.time_base(), stream.start_time());
    let params = stream.parameters();
    let tick = f64::from(tb);
    discard_others(&input, index);
    // Same end as export: a past-end timestamp does not stretch the peaks.
    let file_end = crate::decode::container_end(&input, start_pts, tick);
    let mut decoder = codec::context::Context::from_parameters(params)
        .and_then(|mut c| {
            unsafe { (*c.as_mut_ptr()).pkt_timebase = tb.into() };
            c.decoder().audio()
        })
        .ctx(|| "opening audio decoder".into())?;
    let mut resampler: Option<Resampler> = None;
    let mut ch_resampler: Option<Resampler> = None;
    let mut mono = Vec::new();
    let mut multi = Vec::new();
    let mut acc = Accumulator::new(per_second);
    let mut ch_acc: Option<ChannelAccum> = None;
    let mut start: Option<f64> = None;
    let mut next_in: Option<f64> = None;
    let handle = |f: &frame::Audio| -> Result<()> {
        let t = f.timestamp().map(|t| t as f64 * tick);
        if start.is_none() {
            start = t;
        }
        if resampler.as_ref().is_none_or(|r| !r.matches(f)) {
            resampler = Some(Resampler::new(f, f.rate(), 1)?);
            acc.rate = f.rate();
        }
        if ch_resampler.as_ref().is_none_or(|r| !r.matches(f)) {
            ch_resampler = Some(Resampler::new(f, f.rate(), want)?);
            if ch_acc.is_none() {
                ch_acc = Some(ChannelAccum::new(per_second, usize::from(want)));
            }
            ch_acc.as_mut().expect("set").rate = f.rate();
        }
        // A gap lasts as its timestamps say, as in export: not past the
        // file's end, and not for a far jump when that end is unknown.
        let frame_end = t.map(|t| t + f.samples() as f64 / f64::from(f.rate().max(1)));
        if let (Some(t), Some(next)) = (t, std::mem::replace(&mut next_in, frame_end)) {
            let gap = crate::decode::audio_gap(next, t, file_end);
            if gap > 0.0 {
                let missing = (gap * f64::from(acc.rate)).round() as u64;
                acc.n += missing;
                ch_acc.as_mut().expect("set").n += missing;
            }
        }
        mono.clear();
        resampler
            .as_mut()
            .expect("set")
            .convert(Some(f), &mut mono)?;
        acc.push(&mono);
        multi.clear();
        ch_resampler
            .as_mut()
            .expect("set")
            .convert(Some(f), &mut multi)?;
        ch_acc.as_mut().expect("set").push(&multi);
        Ok(())
    };
    decode_stream(
        &mut input,
        index,
        &mut decoder,
        &mut frame::Audio::empty(),
        handle,
    )?;
    let channels = want;
    let peaks = acc.finish();
    let mut channel_peaks = ch_acc.map(|c| c.finish()).unwrap_or_default();
    // The two resamplers can disagree by a bucket at the tail. The cache
    // format stores `channels` envelopes of the same length as the mono one.
    let need = peaks.len() * usize::from(channels);
    channel_peaks.resize(need, (0, 0));
    Ok(Waveform {
        per_second,
        start: start.unwrap_or(0.0),
        peaks,
        channels,
        channel_peaks,
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

/// Per-channel peaks, bucketed on the same sample clock as [`Accumulator`].
struct ChannelAccum {
    channels: usize,
    per_second: u32,
    rate: u32,
    n: u64,
    lo: Vec<f32>,
    hi: Vec<f32>,
    peaks: Vec<(i8, i8)>,
}

impl ChannelAccum {
    fn new(per_second: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            channels,
            per_second: per_second.max(1),
            rate: 48_000,
            n: 0,
            lo: vec![0.0; channels],
            hi: vec![0.0; channels],
            peaks: Vec::new(),
        }
    }

    fn bucket(&self, n: u64) -> usize {
        (n * u64::from(self.per_second) / u64::from(self.rate.max(1))) as usize
    }

    fn push(&mut self, samples: &[f32]) {
        let ch = self.channels;
        for frame in samples.chunks_exact(ch) {
            let b = self.bucket(self.n);
            while self.peaks.len() < b * ch {
                self.flush();
            }
            for (c, &s) in frame.iter().enumerate() {
                self.lo[c] = self.lo[c].min(s);
                self.hi[c] = self.hi[c].max(s);
            }
            self.n += 1;
        }
    }

    fn flush(&mut self) {
        let q = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
        for c in 0..self.channels {
            self.peaks.push((q(self.lo[c]), q(self.hi[c])));
            self.lo[c] = 0.0;
            self.hi[c] = 0.0;
        }
    }

    fn finish(mut self) -> Vec<(i8, i8)> {
        if self.n > 0 && self.peaks.len() <= self.bucket(self.n - 1) * self.channels {
            self.flush();
        }
        self.peaks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo() -> Waveform {
        Waveform {
            per_second: 100,
            start: 0.0,
            peaks: vec![(0, 64), (0, 0)],
            channels: 2,
            // Bucket 0: L ±64, R silent. Bucket 1: L quiet, R −40.
            channel_peaks: vec![(-64, 64), (0, 0), (0, 5), (-40, 0)],
        }
    }

    #[test]
    fn channel_peaks_round_trip_and_differ() {
        let w = stereo();
        assert_eq!(Waveform::from_bytes(&w.to_bytes()), Some(w.clone()));
        let (lo, hi) = w.channel_range(0, 0.0, 0.01).unwrap();
        assert!(lo < -0.5 && hi > 0.5, "{lo} {hi}");
        assert_eq!(w.channel_range(1, 0.0, 0.01), Some((0.0, 0.0)));
        assert!(w.channel_peak(1, 0.01, 0.02) > 0.3);
        assert!(w.channel_peak(0, 0.01, 0.02) < 0.1);
        assert!(w.channel_range(2, 0.0, 0.01).is_none());
        assert!(Waveform::from_bytes(&[0, 1, 2, 3, 4, 5, 6, 7]).is_none());
    }
}
