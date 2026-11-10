use std::cmp::Ordering;
use std::collections::HashSet;

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
        for s in &mut path.samples {
            s.frame = conv(s.frame);
        }
        path.samples.dedup_by_key(|s| s.frame);
    }
}
