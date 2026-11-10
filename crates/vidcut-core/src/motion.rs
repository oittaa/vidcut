use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use crate::analysis::{Detections, Found, Look};
use crate::model::{MotionPath, PathSample};

pub const PATHS_KEPT: usize = 256;
pub const SAMPLES_KEPT: usize = 1_000_000;
pub const REACH: f64 = 10.0;
pub const TURN: f64 = 100_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub center: [f64; 2],
    pub size: [f64; 2],
    pub rotation: f64,
}

impl Area {
    pub const WHOLE: Self = Self {
        center: [0.5, 0.5],
        size: [1.0, 1.0],
        rotation: 0.0,
    };
}

impl PathSample {
    pub fn area(&self) -> Area {
        Area {
            center: self.center,
            size: self.size,
            rotation: self.rotation,
        }
    }

    pub fn problem(&self) -> Option<String> {
        let within =
            |v: &[f64; 2], lo: f64| v.iter().all(|x| x.is_finite() && (lo..=REACH).contains(x));
        if !within(&self.center, -REACH) {
            return Some(format!(
                "the center on source frame {} is two fractions from -{REACH} to {REACH}",
                self.frame
            ));
        }
        if !within(&self.size, 0.0) {
            return Some(format!(
                "the size on source frame {} is two fractions from 0 to {REACH}",
                self.frame
            ));
        }
        if !self.rotation.is_finite() || self.rotation.abs() > TURN {
            return Some(format!(
                "the rotation on source frame {} is from -{TURN} to {TURN} degrees",
                self.frame
            ));
        }
        None
    }
}

impl MotionPath {
    pub fn area_at(&self, frame: f64) -> Option<Area> {
        let s = &self.samples;
        let next = s.partition_point(|x| x.frame as f64 <= frame);
        if next == 0 || next == s.len() {
            return s.get(next.saturating_sub(1)).map(PathSample::area);
        }
        let (a, b) = (&s[next - 1], &s[next]);
        let t = (frame - a.frame as f64) / (b.frame - a.frame) as f64;
        let mix = |x: f64, y: f64| x + (y - x) * t;
        Some(Area {
            center: [mix(a.center[0], b.center[0]), mix(a.center[1], b.center[1])],
            size: [mix(a.size[0], b.size[0]), mix(a.size[1], b.size[1])],
            rotation: mix(a.rotation, b.rotation),
        })
    }

    pub fn span(&self) -> Option<(i64, i64)> {
        Some((self.samples.first()?.frame, self.samples.last()?.frame))
    }
}

pub fn samples_problem(samples: &[PathSample]) -> Option<String> {
    if samples.is_empty() {
        return Some("a motion path has at least one sample".into());
    }
    if samples.len() > SAMPLES_KEPT {
        return Some(format!(
            "a motion path has {SAMPLES_KEPT} samples at most, not {}",
            samples.len()
        ));
    }
    if let Some(w) = samples.windows(2).find(|w| w[0].frame >= w[1].frame) {
        return Some(format!(
            "the samples of a motion path go up in source frames, one for a frame: {} then {}",
            w[0].frame, w[1].frame
        ));
    }
    samples.iter().find_map(PathSample::problem)
}

pub fn problem(paths: &[MotionPath]) -> Option<String> {
    if paths.len() > PATHS_KEPT {
        return Some(format!(
            "a clip has {PATHS_KEPT} motion paths at most, not {}",
            paths.len()
        ));
    }
    let mut ids = HashSet::new();
    for path in paths {
        if number(&path.id).is_none() {
            return Some(format!("motion path id {:?} is not p<N>", path.id));
        }
        if !ids.insert(path.id.as_str()) {
            return Some(format!("two motion paths have the id {}", path.id));
        }
        if let Some(why) = samples_problem(&path.samples) {
            return Some(format!("{}: {why}", path.id));
        }
    }
    None
}

fn number(id: &str) -> Option<u64> {
    id.strip_prefix('p')?.parse().ok()
}

pub fn next_id(paths: &[MotionPath]) -> String {
    let last = paths.iter().filter_map(|p| number(&p.id)).max();
    format!("p{}", last.map_or(1, |n| n + 1))
}

pub fn put(samples: &[PathSample], new: &[PathSample]) -> Vec<PathSample> {
    let mut out = Vec::with_capacity(samples.len() + new.len());
    let (mut i, mut j) = (0, 0);
    while i < samples.len() && j < new.len() {
        match samples[i].frame.cmp(&new[j].frame) {
            Ordering::Less => {
                out.push(samples[i]);
                i += 1;
            }
            Ordering::Greater => {
                out.push(new[j]);
                j += 1;
            }
            Ordering::Equal => {
                out.push(new[j]);
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&samples[i..]);
    out.extend_from_slice(&new[j..]);
    out
}

pub fn clear(samples: &mut Vec<PathSample>, (first, last): (i64, i64)) {
    samples.retain(|s| s.frame < first || s.frame > last);
}

pub fn rescale(paths: &mut [MotionPath], conv: impl Fn(i64) -> i64) {
    for path in paths {
        let samples = Arc::make_mut(&mut path.samples);
        for s in samples.iter_mut() {
            s.frame = conv(s.frame);
        }
        samples.dedup_by_key(|s| s.frame);
    }
}

pub const SLACK: f64 = 0.05;
pub const LEAST_SLACK: f64 = 0.0005;

#[derive(Debug, Clone, PartialEq)]
pub struct FoundPath {
    pub samples: Vec<PathSample>,
    pub gaps: Vec<[i64; 2]>,
    pub seen: usize,
}

pub const START: f32 = 0.7;
pub const KEEP: f32 = 0.4;
pub const OVERLAP: f64 = 0.2;
pub const WAIT_SECS: f64 = 1.0;
pub const SHORTEST_SECS: f64 = 0.5;

struct Open {
    last: Found,
    seen: i64,
    sure: usize,
    samples: Vec<PathSample>,
}

fn overlap(a: &Found, b: &Found) -> f64 {
    let side = |i: usize| {
        let (a0, a1) = (a.center[i] - a.size[i] / 2.0, a.center[i] + a.size[i] / 2.0);
        let (b0, b1) = (b.center[i] - b.size[i] / 2.0, b.center[i] + b.size[i] / 2.0);
        f64::from((a1.min(b1) - a0.max(b0)).max(0.0))
    };
    let both = side(0) * side(1);
    let (sa, sb) = (
        f64::from(a.size[0] * a.size[1]),
        f64::from(b.size[0] * b.size[1]),
    );
    let all = sa + sb - both;
    if all > 0.0 { both / all } else { 0.0 }
}

fn round5(v: f32) -> f64 {
    (f64::from(v) * 1e5).round() / 1e5
}

fn sample(frame: i64, f: &Found) -> PathSample {
    PathSample {
        frame,
        center: f.center.map(round5),
        size: f.size.map(round5),
        rotation: 0.0,
    }
}

pub fn shown_time(origin: f64, fps: f64, frame: i64) -> f64 {
    origin + (frame as f64 - 0.5) / fps - 1e-9
}

pub fn look(origin: f64, fps: f64, (first, end): (i64, i64)) -> Look {
    Look {
        from: shown_time(origin, fps, first),
        to: shown_time(origin, fps, end - 1),
        every: 1.0 / fps,
    }
}

pub fn joined(
    found: &Detections,
    origin: f64,
    fps: f64,
    (first, end): (i64, i64),
) -> Vec<FoundPath> {
    let wait = (WAIT_SECS * fps).round() as i64;
    let shortest = (SHORTEST_SECS * fps).round().max(1.0) as usize;
    let mut open: Vec<Open> = Vec::new();
    let mut done: Vec<Open> = Vec::new();
    for frame in first..end {
        let boxes = found.shown(shown_time(origin, fps, frame));
        let boxes: Vec<Found> = boxes.iter().copied().filter(|f| f.score >= KEEP).collect();
        let (live, gone): (Vec<Open>, Vec<Open>) =
            open.into_iter().partition(|o| frame - o.seen <= wait);
        open = live;
        done.extend(gone);
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for (t, o) in open.iter().enumerate() {
            for (b, f) in boxes.iter().enumerate() {
                let near = overlap(&o.last, f);
                if near >= OVERLAP {
                    pairs.push((near, t, b));
                }
            }
        }
        pairs.sort_by(|x, y| y.0.total_cmp(&x.0));
        let mut joined = vec![false; boxes.len()];
        let mut moved = vec![false; open.len()];
        for (_, t, b) in pairs {
            if !moved[t] && !joined[b] {
                (moved[t], joined[b]) = (true, true);
                let o = &mut open[t];
                (o.last, o.seen) = (boxes[b], frame);
                o.sure += usize::from(boxes[b].score >= START);
                o.samples.push(sample(frame, &boxes[b]));
            }
        }
        let fresh = boxes
            .iter()
            .zip(&joined)
            .filter(|(f, j)| !**j && f.score >= START);
        for (f, _) in fresh {
            open.push(Open {
                last: *f,
                seen: frame,
                sure: 1,
                samples: vec![sample(frame, f)],
            });
        }
    }
    done.extend(open);
    done.retain(|o| o.sure >= shortest);
    let mut done: Vec<Vec<PathSample>> = done.into_iter().map(|o| o.samples).collect();
    done.sort_by(|a, b| {
        (a[0].frame, a[0].center[0])
            .partial_cmp(&(b[0].frame, b[0].center[0]))
            .unwrap_or(Ordering::Equal)
    });
    let made = |seen: Vec<PathSample>| FoundPath {
        gaps: gaps(&seen),
        samples: simplified(&seen),
        seen: seen.len(),
    };
    done.into_iter().map(made).collect()
}

fn gaps(samples: &[PathSample]) -> Vec<[i64; 2]> {
    samples
        .windows(2)
        .filter(|w| w[1].frame - w[0].frame > 1)
        .map(|w| [w[0].frame + 1, w[1].frame - 1])
        .collect()
}

fn off(a: &PathSample, b: &PathSample, s: &PathSample) -> f64 {
    let t = (s.frame - a.frame) as f64 / (b.frame - a.frame) as f64;
    let mix = |x: f64, y: f64| x + (y - x) * t;
    let side = |i: usize| {
        let center = (s.center[i] - mix(a.center[i], b.center[i])).abs();
        let size = (s.size[i] - mix(a.size[i], b.size[i])).abs();
        center.max(size) / (SLACK * s.size[i]).max(LEAST_SLACK)
    };
    side(0).max(side(1))
}

pub fn simplified(samples: &[PathSample]) -> Vec<PathSample> {
    if samples.len() < 3 {
        return samples.to_vec();
    }
    let mut keep = vec![false; samples.len()];
    (keep[0], keep[samples.len() - 1]) = (true, true);
    let mut parts = vec![(0, samples.len() - 1)];
    while let Some((a, b)) = parts.pop() {
        let worst = (a + 1..b)
            .map(|i| (off(&samples[a], &samples[b], &samples[i]), i))
            .max_by(|x, y| x.0.total_cmp(&y.0));
        if let Some((far, i)) = worst
            && far > 1.0
        {
            keep[i] = true;
            parts.push((a, i));
            parts.push((i, b));
        }
    }
    let kept = samples.iter().zip(&keep).filter(|(_, k)| **k);
    kept.map(|(s, _)| *s).collect()
}
