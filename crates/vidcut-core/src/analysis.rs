//! Measurements of media and the edits made from them: silences (Ripple
//! Delete Silence), shot changes (Detect Scene Cuts), black
//! stretches, loudness, and clip lengths (the Boring Detector).
//!
//! The media layer measures, through [`Env`](crate::editor::Env); everything
//! here is arithmetic on those measurements, so it is tested without media.
//! Positions are project frames: a clip's frame `t` shows source frame
//! `source_in + (t - start)`, whose audio starts at raw time
//! `origin + source_frame / fps`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Clip, Project, TrackKind};
use crate::time::Rational;

/// Peak levels of one audio stream: the largest absolute sample of any
/// channel in each `1 / per_second` of a second (linear, 0..1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioLevels {
    pub per_second: u32,
    /// Raw timestamp (s) where the first bucket starts.
    pub start: f64,
    pub peaks: Vec<f32>,
}

/// Picture measurements of one video stream, frame by frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PictureStats {
    /// Raw timestamp (s) of each frame, ascending.
    pub pts: Vec<f64>,
    /// How much each frame differs from the one before, 0..100: FFmpeg's
    /// scene score (scdet), on luma. The first frame scores 0.
    pub score: Vec<f32>,
    /// Share of each frame's pixels that are black (luma at most 10% of the
    /// way from black to white), 0..1.
    pub black: Vec<f32>,
}

/// Little-endian readers for the cache formats below.
struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.0.split_first_chunk::<N>()?;
        self.0 = rest;
        Some(*head)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }
    fn f64(&mut self) -> Option<f64> {
        self.take().map(f64::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take().map(f32::from_le_bytes)
    }
}

impl AudioLevels {
    const TAG: &'static [u8; 4] = b"vcl1";

    /// A compact form for caching measurements on disk.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Self::TAG.to_vec();
        out.extend_from_slice(&u64::from(self.per_second).to_le_bytes());
        out.extend_from_slice(&self.start.to_le_bytes());
        out.extend_from_slice(&(self.peaks.len() as u64).to_le_bytes());
        for p in &self.peaks {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut r = Bytes(b.strip_prefix(Self::TAG)?);
        let per_second = u32::try_from(r.u64()?).ok()?;
        let start = r.f64()?;
        let n = usize::try_from(r.u64()?).ok()?;
        let peaks = (0..n).map(|_| r.f32()).collect::<Option<Vec<_>>>()?;
        Some(Self {
            per_second,
            start,
            peaks,
        })
    }
}

impl PictureStats {
    const TAG: &'static [u8; 4] = b"vcp1";

    /// A compact form for caching measurements on disk.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Self::TAG.to_vec();
        out.extend_from_slice(&(self.pts.len() as u64).to_le_bytes());
        for i in 0..self.pts.len() {
            out.extend_from_slice(&self.pts[i].to_le_bytes());
            out.extend_from_slice(&self.score[i].to_le_bytes());
            out.extend_from_slice(&self.black[i].to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut r = Bytes(b.strip_prefix(Self::TAG)?);
        let n = usize::try_from(r.u64()?).ok()?;
        let mut s = Self::default();
        for _ in 0..n {
            s.pts.push(r.f64()?);
            s.score.push(r.f32()?);
            s.black.push(r.f32()?);
        }
        Some(s)
    }
}

/// Loudness of a mix (EBU R128, ITU-R BS.1770).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Loudness {
    /// Integrated loudness, LUFS (`-inf` for silence).
    pub integrated: f64,
    /// Loudness range, LU.
    pub range: f64,
    /// Highest true peak, dBTP.
    pub true_peak: f64,
}

fn linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// What a stretch of picture or sound is measured over: frames `[start,
/// end)` on the timeline showing source frames from `source_in`, of media
/// whose source frame 0 is at raw time `origin`. A clip, or a whole media
/// item (start 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: i64,
    pub end: i64,
    pub source_in: i64,
    pub origin: f64,
    pub fps: Rational,
}

impl Span {
    pub fn of_clip(c: &Clip, origin: f64, fps: Rational) -> Self {
        Self {
            start: c.start,
            end: c.end(),
            source_in: c.source_in,
            origin,
            fps,
        }
    }

    /// Raw time where frame `t`'s audio starts.
    fn time(&self, t: i64) -> f64 {
        self.origin + (self.source_in + t - self.start) as f64 / self.fps.as_f64()
    }

    /// Frame position (fractional) of raw time `x`.
    fn frame(&self, x: f64) -> f64 {
        self.start as f64 + (x - self.origin) * self.fps.as_f64() - self.source_in as f64
    }
}

/// Silent stretches of `levels` between raw times `from` and `to`, at least
/// `min` seconds long: where every peak, after `gain_db`, stays below
/// `threshold_db` dBFS. Time without audio (before or after the stream)
/// counts as silent. Raw seconds, clipped to the window.
pub fn silences(
    levels: &AudioLevels,
    from: f64,
    to: f64,
    threshold_db: f64,
    gain_db: f64,
    min: f64,
) -> Vec<(f64, f64)> {
    let rate = f64::from(levels.per_second.max(1));
    let limit = linear(threshold_db - gain_db) as f32;
    let bucket = |x: f64| ((x - levels.start) * rate).floor() as i64;
    let time = |i: i64| levels.start + i as f64 / rate;
    let quiet = |i: i64| {
        usize::try_from(i)
            .ok()
            .and_then(|i| levels.peaks.get(i))
            .is_none_or(|&p| p < limit)
    };
    let (first, last) = (bucket(from), bucket(to - 1e-9));
    let mut out = Vec::new();
    let mut run: Option<i64> = None;
    for i in first..=last + 1 {
        let silent = i <= last && quiet(i);
        match (silent, run) {
            (true, None) => run = Some(i),
            (false, Some(s)) => {
                let (a, b) = (time(s).max(from), time(i).min(to));
                if b - a >= min - 1e-9 {
                    out.push((a, b));
                }
                run = None;
            }
            _ => {}
        }
    }
    out
}

/// Ripple Delete Silence's settings, in frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceSettings {
    /// Audio below this level (dBFS) is silence.
    pub threshold_db: f64,
    /// Silence kept before each audible part.
    pub pre_head: i64,
    /// Silence kept after each audible part.
    pub post_tail: i64,
    /// The shortest silence removed ("minimum to trip").
    pub min: i64,
}

/// The silent parts of `span` to remove, as frame ranges: each silence at
/// least `min` long, less `post_tail` after the sound before it and
/// `pre_head` before the sound after it (none at the span's edges). Only
/// whole frames are removed.
pub fn silent_parts(
    span: &Span,
    levels: &AudioLevels,
    gain_db: f64,
    s: &SilenceSettings,
) -> Vec<(i64, i64)> {
    let f = span.fps.as_f64();
    let quiet = silences(
        levels,
        span.time(span.start),
        span.time(span.end),
        s.threshold_db,
        gain_db,
        s.min as f64 / f,
    );
    let mut parts = Vec::new();
    for (a, b) in quiet {
        let (fa, fb) = (span.frame(a), span.frame(b));
        let x0 = if fa <= span.start as f64 + 1e-6 {
            span.start as f64
        } else {
            fa + s.post_tail as f64
        };
        let x1 = if fb >= span.end as f64 - 1e-6 {
            span.end as f64
        } else {
            fb - s.pre_head as f64
        };
        let (p0, p1) = ((x0 - 1e-6).ceil() as i64, (x1 + 1e-6).floor() as i64);
        let (p0, p1) = (p0.max(span.start), p1.min(span.end));
        if p1 > p0 {
            parts.push((p0, p1));
        }
    }
    parts
}

/// The source frame shown at frame `t` of `span`: the first one at or after
/// the frame's time less half a frame (the sampling rule, export.md §2).
fn shown(span: &Span, stats: &PictureStats, t: i64) -> usize {
    let target = span.time(t) - 0.5 / span.fps.as_f64() - 1e-9;
    stats.pts.partition_point(|&p| p < target)
}

/// Frames of `span` where a new shot starts, with their scores: the first
/// frame showing a source frame that scores at least `threshold` against the
/// one before it (Detect Scene Cuts). Frames skipped between two
/// shown ones count too. The span's own first frame never counts.
pub fn scene_cuts(span: &Span, stats: &PictureStats, threshold: f64) -> Vec<(i64, f32)> {
    let mut cuts = Vec::new();
    let mut prev = shown(span, stats, span.start);
    for t in span.start + 1..span.end {
        let k = shown(span, stats, t).min(stats.score.len());
        if k > prev {
            let score = stats.score[prev + 1..k.min(stats.score.len() - 1) + 1]
                .iter()
                .fold(0f32, |m, &s| m.max(s));
            if f64::from(score) >= threshold {
                cuts.push((t, score));
            }
        }
        prev = k;
    }
    cuts
}

/// Stretches of `span` at least `min` frames long whose frames are black:
/// at least `share` of their pixels.
pub fn black_parts(span: &Span, stats: &PictureStats, share: f64, min: i64) -> Vec<(i64, i64)> {
    let black = |t: i64| {
        let k = shown(span, stats, t).min(stats.black.len().saturating_sub(1));
        stats.black.get(k).is_some_and(|&b| f64::from(b) >= share)
    };
    let mut out = Vec::new();
    let mut run: Option<i64> = None;
    for t in span.start..=span.end {
        let is = t < span.end && black(t);
        match (is, run) {
            (true, None) => run = Some(t),
            (false, Some(s)) => {
                if t - s >= min.max(1) {
                    out.push((s, t));
                }
                run = None;
            }
            _ => {}
        }
    }
    out
}

/// The Boring Detector: clips on video tracks longer than
/// `boring` frames, and clips shorter than `jump` frames (jump cuts and
/// flash frames).
pub fn boring(p: &Project, boring: i64, jump: i64) -> (Vec<&Clip>, Vec<&Clip>) {
    let clips = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .flat_map(|t| &t.clips);
    let (mut long, mut short) = (Vec::new(), Vec::new());
    for c in clips {
        if c.duration > boring {
            long.push(c);
        } else if c.duration < jump {
            short.push(c);
        }
    }
    (long, short)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fps(n: i64) -> Rational {
        Rational::new(n, 1).unwrap()
    }

    /// 100 buckets a second: `loud` seconds marked loud (0.5), the rest silent.
    fn levels(secs: f64, loud: &[(f64, f64)]) -> AudioLevels {
        let n = (secs * 100.0) as usize;
        let peaks = (0..n)
            .map(|i| {
                let t = i as f64 / 100.0;
                if loud.iter().any(|&(a, b)| t >= a && t < b) {
                    0.5
                } else {
                    0.001
                }
            })
            .collect();
        AudioLevels {
            per_second: 100,
            start: 0.0,
            peaks,
        }
    }

    #[test]
    fn silences_are_runs_below_the_threshold() {
        let l = levels(4.0, &[(1.0, 1.5), (3.0, 3.2)]);
        let s = silences(&l, 0.0, 4.0, -40.0, 0.0, 0.9);
        assert_eq!(
            s,
            vec![(0.0, 1.0), (1.5, 3.0)],
            "0.8 s at the end is too short"
        );
        // Gain makes quiet sound loud enough: -60 dBFS + 30 dB is above -40.
        assert!(silences(&l, 0.0, 4.0, -40.0, 30.0, 0.1).is_empty());
        // Past the stream's end counts as silent.
        assert_eq!(silences(&l, 3.5, 5.0, -40.0, 0.0, 1.0), vec![(3.5, 5.0)]);
    }

    #[test]
    fn silent_parts_keep_heads_and_tails() {
        // Speech at 1.0–1.5 s and 3.0–3.2 s of a 4 s clip at 10 fps.
        let l = levels(4.0, &[(1.0, 1.5), (3.0, 3.2)]);
        let span = Span {
            start: 100,
            end: 140,
            source_in: 0,
            origin: 0.0,
            fps: fps(10),
        };
        let s = SilenceSettings {
            threshold_db: -40.0,
            pre_head: 2,
            post_tail: 3,
            min: 5,
        };
        // Leading silence (no tail to keep): 100..108; between: 1.5 s + 3
        // frames .. 3.0 s - 2 frames = 118..128; trailing 0.8 s: 132+3..140.
        assert_eq!(
            silent_parts(&span, &l, 0.0, &s),
            vec![(100, 108), (118, 128), (135, 140)]
        );
    }

    #[test]
    fn black_parts_need_their_length() {
        let n = 40;
        let stats = PictureStats {
            pts: (0..n).map(|k| f64::from(k) / 10.0).collect(),
            score: vec![0.0; n as usize],
            black: (0..n)
                .map(|k| {
                    if (10..20).contains(&k) || k == 30 {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect(),
        };
        let span = Span {
            start: 0,
            end: 40,
            source_in: 0,
            origin: 0.0,
            fps: fps(10),
        };
        assert_eq!(black_parts(&span, &stats, 0.98, 5), vec![(10, 20)]);
        assert_eq!(
            black_parts(&span, &stats, 0.98, 1),
            vec![(10, 20), (30, 31)]
        );
    }
}
