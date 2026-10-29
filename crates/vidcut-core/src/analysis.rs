use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::keyframes::Gain;
use crate::model::{Clip, Project, TrackKind};
use crate::render::Pace;
use crate::time::Rational;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioLevels {
    pub per_second: u32,
    pub start: f64,
    pub peaks: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VoiceActivity {
    pub rate: u32,
    pub frame: u32,
    pub start: f64,
    pub speech: Vec<f32>,
}

pub const SCENE_SCORE: f64 = 10.0;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PictureStats {
    pub pts: Vec<f64>,
    pub score: Vec<f32>,
    pub black: Vec<f32>,
}

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

impl VoiceActivity {
    const TAG: &'static [u8; 4] = b"vcv1";

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Self::TAG.to_vec();
        out.extend_from_slice(&u64::from(self.rate).to_le_bytes());
        out.extend_from_slice(&u64::from(self.frame).to_le_bytes());
        out.extend_from_slice(&self.start.to_le_bytes());
        out.extend_from_slice(&(self.speech.len() as u64).to_le_bytes());
        for p in &self.speech {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut r = Bytes(b.strip_prefix(Self::TAG)?);
        let rate = u32::try_from(r.u64()?).ok()?;
        let frame = u32::try_from(r.u64()?).ok()?;
        let start = r.f64()?;
        let n = usize::try_from(r.u64()?).ok()?;
        let speech = (0..n).map(|_| r.f32()).collect::<Option<Vec<_>>>()?;
        Some(Self {
            rate,
            frame,
            start,
            speech,
        })
    }
}

impl PictureStats {
    const TAG: &'static [u8; 4] = b"vcp1";

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
    /// Highest sample, dBFS.
    pub sample_peak: f64,
}

fn linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: i64,
    pub end: i64,
    pub source_in: i64,
    pub origin: f64,
    pub fps: Rational,
    pub pace: Pace,
}

impl Span {
    pub fn of_clip(c: &Clip, origin: f64, fps: Rational) -> Vec<Self> {
        c.runs(c.start, c.end())
            .into_iter()
            .map(|(a, b)| Self {
                start: a,
                end: b,
                source_in: c.source_at(a),
                origin,
                fps,
                pace: c.pace().after(a - c.start),
            })
            .collect()
    }

    fn at(&self, pos: f64) -> f64 {
        self.origin + pos / self.fps.as_f64()
    }

    fn time(&self, t: f64) -> f64 {
        self.at(self.source_in as f64 + self.pace.sound(t - self.start as f64))
    }

    fn frame(&self, x: f64) -> f64 {
        let pos = (x - self.origin) * self.fps.as_f64() - self.source_in as f64;
        self.start as f64 + (pos - self.pace.sound(0.0)) / self.pace.speed()
    }

    fn shown_time(&self, t: i64) -> f64 {
        self.at((self.source_in + self.pace.offset(t - self.start)) as f64)
    }
}

pub struct Heard<'a> {
    pub span: &'a Span,
    pub levels: &'a AudioLevels,
    pub gain: Gain<'a>,
}

const QUIET_AROUND: f64 = 0.02;

pub fn quietest(heard: &[Heard], at: i64, around: std::ops::RangeInclusive<i64>) -> i64 {
    around
        .map(|f| (level_at(heard, f), (f - at).abs(), f))
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map_or(at, |(_, _, f)| f)
}

fn level_at(heard: &[Heard], frame: i64) -> f64 {
    heard
        .iter()
        .filter(|h| h.span.pace.speed() != 0.0 && h.span.start <= frame && frame <= h.span.end)
        .map(|h| {
            let rate = f64::from(h.levels.per_second.max(1));
            let at = h.span.time(frame as f64);
            let i = ((at - h.levels.start) * rate).floor() as i64;
            let reach = (QUIET_AROUND * rate).round().max(1.0) as i64;
            let peak = (i - reach..=i + reach)
                .filter_map(|i| usize::try_from(i).ok().and_then(|i| h.levels.peaks.get(i)))
                .fold(0.0_f32, |m, &p| m.max(p));
            f64::from(peak) * linear(h.gain.at(at))
        })
        .sum()
}

pub fn words_in(span: &Span, words: &[crate::transcript::Word]) -> Vec<(usize, i64, i64)> {
    words_between(span, (span.start, span.end), words)
}

pub fn words_between(
    span: &Span,
    (from, to): (i64, i64),
    words: &[crate::transcript::Word],
) -> Vec<(usize, i64, i64)> {
    if span.pace.speed() == 0.0 {
        return Vec::new();
    }
    words
        .iter()
        .enumerate()
        .filter_map(|(i, w)| {
            let a = span.frame(span.origin + w.start);
            let b = span.frame(span.origin + w.end);
            let (a, b) = (a.min(b), a.max(b));
            let middle = (a + b) / 2.0;
            if middle < from as f64 || middle >= to as f64 {
                return None;
            }
            let start = (a.round() as i64).max(from);
            let end = (b.round() as i64).max(start + 1).min(to);
            (end > start).then_some((i, start, end))
        })
        .collect()
}

const CLICK: f64 = 0.05;
const CLICK_GAP: f64 = 0.1;

pub fn silences(
    levels: &AudioLevels,
    from: f64,
    to: f64,
    threshold_db: f64,
    gain: &Gain,
    min: f64,
) -> Vec<(f64, f64)> {
    let rate = f64::from(levels.per_second.max(1));
    let bucket = |x: f64| ((x - levels.start) * rate).floor() as i64;
    let time = |i: i64| levels.start + i as f64 / rate;
    let limit = limits(gain, threshold_db, rate, &time);
    let (first, last) = (bucket(from), bucket(to - 1e-9));
    let gap = (CLICK_GAP * rate).round().max(1.0) as usize;
    let lo = first - gap as i64;
    let mut quiet: Vec<bool> = (lo..=last + gap as i64)
        .map(|i| {
            usize::try_from(i)
                .ok()
                .and_then(|i| levels.peaks.get(i))
                .is_none_or(|&p| p < limit(i))
        })
        .collect();
    for (a, b) in clicks(&quiet, (CLICK * rate).round() as usize, gap) {
        quiet[a..b].fill(true);
    }
    let mut out = Vec::new();
    let mut run: Option<i64> = None;
    for i in first..=last + 1 {
        let silent = i <= last && quiet[(i - lo) as usize];
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

fn limits<'a>(
    gain: &'a Gain,
    threshold_db: f64,
    rate: f64,
    time: &'a dyn Fn(i64) -> f64,
) -> impl Fn(i64) -> f32 + 'a {
    let flat = linear(threshold_db - gain.db) as f32;
    move |i| {
        if gain.is_flat() {
            flat
        } else {
            linear(threshold_db - gain.at(time(i) + 0.5 / rate)) as f32
        }
    }
}

fn clicks(quiet: &[bool], len: usize, gap: usize) -> Vec<(usize, usize)> {
    let quiet_at = |r: Option<std::ops::Range<usize>>| {
        r.and_then(|r| quiet.get(r))
            .is_some_and(|s| s.iter().all(|&q| q))
    };
    let mut out = Vec::new();
    let mut a = 0;
    for run in quiet.chunk_by(|x, y| x == y) {
        let b = a + run.len();
        if !run[0]
            && run.len() < len
            && quiet_at(a.checked_sub(gap).map(|s| s..a))
            && quiet_at(Some(b..b + gap))
        {
            out.push((a, b));
        }
        a = b;
    }
    out
}

fn speech_runs(
    speech: &[f32],
    frame: i64,
    threshold: f64,
    min_speech: f64,
    min_silence: f64,
) -> Vec<(i64, i64)> {
    let below = (threshold - 0.15).max(0.01).min(threshold);
    let mut out = Vec::new();
    let (mut start, mut quiet_from): (Option<i64>, Option<i64>) = (None, None);
    for (i, &p) in speech.iter().enumerate() {
        let (p, at) = (f64::from(p), frame * i as i64);
        if p >= threshold {
            quiet_from = None;
            start.get_or_insert(at);
        } else if p < below
            && let Some(s) = start
        {
            let q = *quiet_from.get_or_insert(at);
            if (at - q) as f64 >= min_silence {
                if (q - s) as f64 > min_speech {
                    out.push((s, q));
                }
                (start, quiet_from) = (None, None);
            }
        }
    }
    let end = frame * speech.len() as i64;
    if let Some(s) = start
        && (end - s) as f64 > min_speech
    {
        out.push((s, end));
    }
    out
}

pub fn non_speech(
    va: &VoiceActivity,
    from: f64,
    to: f64,
    threshold: f64,
    min_speech: f64,
    min: f64,
) -> Vec<(f64, f64)> {
    let rate = f64::from(va.rate.max(1));
    let frame = i64::from(va.frame.max(1));
    let index = |t: f64| (t - va.start) * rate / frame as f64;
    let n = va.speech.len();
    let first = (index(from).floor().max(0.0) as usize).min(n);
    let last = (index(to).ceil().max(0.0) as usize).clamp(first, n);
    let time = |s: i64| va.start + (first as i64 * frame + s) as f64 / rate;
    let runs = speech_runs(
        &va.speech[first..last],
        frame,
        threshold,
        min_speech * rate,
        min * rate,
    );
    let mut out = Vec::new();
    let mut at = from;
    for (a, b) in runs
        .into_iter()
        .map(|(a, b)| (time(a).max(from), time(b).min(to)))
        .chain([(to, to)])
    {
        if a > at && a - at >= min - 1e-9 {
            out.push((at, a));
        }
        at = at.max(b);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceSettings {
    pub threshold_db: f64,
    pub pre_head: i64,
    pub post_tail: i64,
    pub min: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeechSettings {
    pub threshold: f64,
    pub min_speech: i64,
}

pub fn joined(spans: &[Span], f: impl Fn(&Span) -> Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (a, b) in spans.iter().flat_map(f) {
        match out.last_mut() {
            Some(last) if last.1 == a => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

pub fn silent_parts(
    span: &Span,
    levels: &AudioLevels,
    gain: &Gain,
    s: &SilenceSettings,
) -> Vec<(i64, i64)> {
    parts_of(span, s, |from, to, min| {
        silences(levels, from, to, s.threshold_db, gain, min)
    })
}

pub fn non_speech_parts(
    span: &Span,
    va: &VoiceActivity,
    s: &SilenceSettings,
    speech: &SpeechSettings,
) -> Vec<(i64, i64)> {
    let min_speech = speech.min_speech as f64 * span.pace.speed().abs() / span.fps.as_f64();
    parts_of(span, s, |from, to, min| {
        non_speech(va, from, to, speech.threshold, min_speech, min)
    })
}

pub fn speech_frames(span: &Span, frames: (i64, i64), va: &VoiceActivity) -> Vec<(f64, f64)> {
    sounding_frames(span, frames, |from, to| {
        let mut out = Vec::new();
        let mut at = from;
        let quiet = non_speech(
            va,
            from,
            to,
            crate::retime::SPEECH,
            crate::retime::SHORTEST_SPEECH,
            crate::retime::SHORTEST_PAUSE,
        );
        for (a, b) in quiet.into_iter().chain([(to, to)]) {
            if a > at {
                out.push((at, a));
            }
            at = at.max(b);
        }
        out
    })
}

pub fn loud_frames(
    span: &Span,
    frames: (i64, i64),
    levels: &AudioLevels,
    threshold_db: f64,
    gain: &Gain,
) -> Vec<(f64, f64)> {
    sounding_frames(span, frames, |from, to| {
        let rate = f64::from(levels.per_second.max(1));
        let bucket = |x: f64| ((x - levels.start) * rate).floor() as i64;
        let time = |i: i64| levels.start + i as f64 / rate;
        let limit = limits(gain, threshold_db, rate, &time);
        let (first, last) = (bucket(from), bucket(to - 1e-9));
        let mut out = Vec::new();
        let mut run: Option<i64> = None;
        for i in first..=last + 1 {
            let loud = i <= last
                && usize::try_from(i)
                    .ok()
                    .and_then(|i| levels.peaks.get(i))
                    .is_some_and(|&p| p >= limit(i));
            match (loud, run) {
                (true, None) => run = Some(i),
                (false, Some(s)) => {
                    out.push((time(s).max(from), time(i).min(to)));
                    run = None;
                }
                _ => {}
            }
        }
        out
    })
}

fn sounding_frames(
    span: &Span,
    (from, to): (i64, i64),
    runs: impl FnOnce(f64, f64) -> Vec<(f64, f64)>,
) -> Vec<(f64, f64)> {
    if span.pace.speed() == 0.0 || to <= from {
        return Vec::new();
    }
    let (x, y) = (span.time(from as f64), span.time(to as f64));
    runs(x.min(y), x.max(y))
        .into_iter()
        .map(|(a, b)| {
            let (fa, fb) = (span.frame(a), span.frame(b));
            (fa.min(fb).max(from as f64), fa.max(fb).min(to as f64))
        })
        .filter(|(a, b)| b > a)
        .collect()
}

fn parts_of(
    span: &Span,
    s: &SilenceSettings,
    quiet: impl FnOnce(f64, f64, f64) -> Vec<(f64, f64)>,
) -> Vec<(i64, i64)> {
    if span.pace.speed() == 0.0 {
        return Vec::new();
    }
    let f = span.fps.as_f64();
    let (from, to) = (span.time(span.start as f64), span.time(span.end as f64));
    let quiet = quiet(
        from.min(to),
        from.max(to),
        s.min as f64 * span.pace.speed().abs() / f,
    );
    let mut parts = Vec::new();
    for (a, b) in quiet {
        let (fa, fb) = (span.frame(a), span.frame(b));
        let (fa, fb) = (fa.min(fb), fa.max(fb));
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
    parts.sort_unstable();
    parts
}

fn shown(span: &Span, stats: &PictureStats, t: i64) -> usize {
    let target = span.shown_time(t) - 0.5 / span.fps.as_f64() - 1e-9;
    stats.pts.partition_point(|&p| p < target)
}

pub fn scene_cuts(span: &Span, stats: &PictureStats, threshold: f64) -> Vec<(i64, f32)> {
    let mut cuts = Vec::new();
    let mut prev = shown(span, stats, span.start);
    for t in span.start + 1..span.end {
        let k = shown(span, stats, t).min(stats.score.len());
        let (lo, hi) = (
            prev.min(k),
            prev.max(k).min(stats.score.len().saturating_sub(1)),
        );
        if hi > lo {
            let score = stats.score[lo + 1..hi + 1]
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
        let s = silences(&l, 0.0, 4.0, -40.0, &Gain::flat(0.0), 0.9);
        assert_eq!(
            s,
            vec![(0.0, 1.0), (1.5, 3.0)],
            "0.8 s at the end is too short"
        );
        assert!(silences(&l, 0.0, 4.0, -40.0, &Gain::flat(30.0), 0.1).is_empty());
        assert_eq!(
            silences(&l, 3.5, 5.0, -40.0, &Gain::flat(0.0), 1.0),
            vec![(3.5, 5.0)]
        );
    }

    #[test]
    fn a_click_in_silence_is_silent() {
        let l = levels(2.0, &[(1.0, 1.02)]);
        assert_eq!(
            silences(&l, 0.0, 2.0, -40.0, &Gain::flat(0.0), 0.5),
            vec![(0.0, 2.0)]
        );
    }

    #[test]
    fn a_click_just_after_sound_is_sound() {
        let l = levels(3.0, &[(1.0, 1.5), (1.55, 1.57)]);
        assert_eq!(
            silences(&l, 0.0, 3.0, -40.0, &Gain::flat(0.0), 0.5),
            vec![(0.0, 1.0), (1.57, 3.0)]
        );
    }

    #[test]
    fn silent_parts_keep_heads_and_tails() {
        let l = levels(4.0, &[(1.0, 1.5), (3.0, 3.2)]);
        let span = Span {
            pace: Default::default(),
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
        assert_eq!(
            silent_parts(&span, &l, &Gain::flat(0.0), &s),
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
            pace: Default::default(),
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
