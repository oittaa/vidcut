use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use crate::analysis::{Detections, Found, Look};
use crate::model::{MotionPath, PathSample, Subject};

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

pub fn followed(
    found: &Detections,
    origin: f64,
    fps: f64,
    (first, end): (i64, i64),
    own: &Area,
    bounds: [f64; 4],
) -> (Vec<PathSample>, f64) {
    let round = |v: f64| (v * 1e5).round() / 1e5;
    let mut samples = Vec::new();
    let mut scores = 0.0;
    for frame in first..end {
        let [b] = found.shown(shown_time(origin, fps, frame)) else {
            continue;
        };
        let was = (bounds[2] * bounds[3]).max(1e-12);
        let grown = (f64::from(b.size[0] * b.size[1]) / was).sqrt();
        let center = [0, 1].map(|i| f64::from(b.center[i]) + (own.center[i] - bounds[i]) * grown);
        scores += f64::from(b.score);
        samples.push(PathSample {
            frame,
            center: center.map(round),
            size: own.size.map(|s| round(s * grown)),
            rotation: (own.rotation * 100.0).round() / 100.0,
        });
    }
    let score = scores / samples.len().max(1) as f64;
    (samples, (score * 100.0).round() / 100.0)
}

pub fn stopped(found: &Detections, origin: f64, fps: f64, frame: i64) -> &'static str {
    let scene = found.scene_shown(shown_time(origin, fps, frame));
    if f64::from(scene) >= crate::analysis::SCENE_SCORE {
        "cut"
    } else {
        "lost"
    }
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
    pub fn follows(&self, named: &str) -> bool {
        self.id == named || self.subject.is_some_and(|s| s.name() == named)
    }

    pub fn area_at(&self, frame: f64) -> Option<Area> {
        let s = &self.samples;
        let (first, last) = self.span()?;
        if frame < first as f64 || frame > last as f64 {
            return None;
        }
        let next = s.partition_point(|x| x.frame as f64 <= frame);
        if next == s.len() {
            return s.last().map(PathSample::area);
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

    pub fn held_at(&self, frame: f64) -> Option<Area> {
        let (first, last) = self.span()?;
        self.area_at(frame.clamp(first as f64, last as f64))
    }

    pub fn base_at(&self, frame: f64) -> Option<Area> {
        if self.tracked {
            self.held_at(frame)
        } else {
            self.area_at(frame)
        }
    }

    pub fn span(&self) -> Option<(i64, i64)> {
        Some((self.samples.first()?.frame, self.samples.last()?.frame))
    }
}

pub fn areas_of(paths: &[MotionPath], named: &str, frame: f64) -> Vec<Area> {
    let followed = paths.iter().filter(|m| m.follows(named));
    followed.filter_map(|m| m.base_at(frame)).collect()
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
pub const TURN_SLACK: f64 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub struct FoundPath {
    pub samples: Vec<PathSample>,
    pub gaps: Vec<[i64; 2]>,
    pub seen: usize,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Join {
    pub start: f32,
    pub keep: f32,
    pub overlap: f64,
    pub wait_secs: f64,
    pub shortest_secs: f64,
    pub steady: f64,
}

impl Join {
    pub fn of(subject: Subject) -> Self {
        match subject {
            Subject::Face => Self {
                start: 0.7,
                keep: 0.4,
                overlap: 0.2,
                wait_secs: 1.0,
                shortest_secs: 0.5,
                steady: 0.5,
            },
            Subject::Text => Self {
                start: 0.7,
                keep: 0.6,
                overlap: 0.2,
                wait_secs: 1.0,
                shortest_secs: 1.0,
                steady: 0.8,
            },
            Subject::Person | Subject::Vehicle => Self {
                start: 0.5,
                keep: 0.35,
                overlap: 0.2,
                wait_secs: 1.0,
                shortest_secs: 1.0,
                steady: 0.5,
            },
        }
    }

    pub fn problem(&self) -> Option<&'static str> {
        let share = |v: f64| (0.0..=1.0).contains(&v);
        let secs = |v: f64| (0.0..=Self::MOST_SECS).contains(&v);
        if !share(f64::from(self.start)) || !share(f64::from(self.keep)) {
            return Some("start and keep are scores from 0 to 1");
        }
        if self.keep > self.start {
            return Some("keep is at most start");
        }
        if !share(self.overlap) || !share(self.steady) {
            return Some("overlap and steady are shares from 0 to 1");
        }
        if !secs(self.wait_secs) || !secs(self.shortest_secs) {
            return Some("wait and shortest are from 0 to 60 seconds");
        }
        None
    }

    pub const MOST_SECS: f64 = 60.0;
}

struct Open {
    last: Found,
    seen: i64,
    total: f64,
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
        rotation: (f64::from(f.rotation) * 100.0).round() / 100.0,
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
        side: None,
    }
}

pub fn joined(
    found: &Detections,
    origin: f64,
    fps: f64,
    (first, end): (i64, i64),
    rules: &Join,
) -> Vec<FoundPath> {
    let wait = (rules.wait_secs * fps).round() as i64;
    let shortest = (rules.shortest_secs * fps).round().max(1.0) as usize;
    let mut open: Vec<Open> = Vec::new();
    let mut done: Vec<Open> = Vec::new();
    let mut shots = vec![first];
    for frame in first..end {
        let shown = |f: i64| shown_time(origin, fps, f);
        if frame > first && found.cut(shown(frame - 1), shown(frame)) {
            done.append(&mut open);
            shots.push(frame);
        }
        let boxes = found.shown(shown(frame));
        let kept = boxes.iter().copied().filter(|f| f.score >= rules.keep);
        let boxes: Vec<Found> = kept.collect();
        let (live, gone): (Vec<Open>, Vec<Open>) =
            open.into_iter().partition(|o| frame - o.seen <= wait);
        open = live;
        done.extend(gone);
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for (t, o) in open.iter().enumerate() {
            for (b, f) in boxes.iter().enumerate() {
                let near = overlap(&o.last, f);
                if near >= rules.overlap {
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
                o.total += f64::from(boxes[b].score);
                o.samples.push(sample(frame, &boxes[b]));
            }
        }
        let fresh = boxes
            .iter()
            .zip(&joined)
            .filter(|(f, j)| !**j && f.score >= rules.start);
        for (f, _) in fresh {
            open.push(Open {
                last: *f,
                seen: frame,
                total: f64::from(f.score),
                samples: vec![sample(frame, f)],
            });
        }
    }
    done.extend(open);
    let lasts = |o: &Open| (o.seen - o.samples[0].frame + 1) as usize;
    let steady = |o: &Open| o.samples.len() as f64 >= rules.steady * lasts(o) as f64;
    let left = |o: &Open| {
        let from = o.samples[0].frame;
        let next = shots.partition_point(|&s| s <= from);
        (shots.get(next).copied().unwrap_or(end) - from) as usize
    };
    done.retain(|o| lasts(o) >= shortest.min(left(o)) && steady(o));
    done.sort_by(|a, b| {
        let (a, b) = (&a.samples[0], &b.samples[0]);
        (a.frame, a.center[0])
            .partial_cmp(&(b.frame, b.center[0]))
            .unwrap_or(Ordering::Equal)
    });
    let made = |o: Open| FoundPath {
        gaps: gaps(&o.samples),
        samples: simplified(&o.samples),
        seen: o.samples.len(),
        score: (o.total / o.samples.len() as f64 * 100.0).round() / 100.0,
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
    let turn = (s.rotation - mix(a.rotation, b.rotation)).abs() / TURN_SLACK;
    side(0).max(side(1)).max(turn)
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
