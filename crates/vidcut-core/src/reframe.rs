use std::ops::Range;
use std::sync::Arc;

use crate::analysis::Tracked;
use crate::model::{Keyframe, MotionPath, Picture};
use crate::motion::FoundPath;
use crate::request::ReframeMode;

pub const SMOOTH: (f64, f64) = (0.0, 10.0);
pub const USUAL_SMOOTH: f64 = 0.5;
pub const TOLERANCE: (f64, f64) = (0.01, 100.0);
pub const USUAL_TOLERANCE: f64 = 1.0;
const ROOM: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    pub center: [f64; 2],
    pub size: [f64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub key: i64,
    pub seen: Option<Seen>,
    pub cut: bool,
    pub zoom: [f64; 2],
    pub anchor: [f64; 2],
    pub now: [f64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub size: [f64; 2],
    pub flip: [bool; 2],
    pub pixels: [f64; 2],
    pub fps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    pub mode: ReframeMode,
    pub smooth: f64,
    pub tolerance: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Moved {
    pub keys: Vec<Keyframe>,
    pub y: Vec<f64>,
    pub axes: [bool; 2],
    pub seen: usize,
}

impl Shape {
    fn scale(&self, f: &Frame, i: usize) -> f64 {
        (f.zoom[i] * self.size[i]).max(ROOM)
    }

    fn half(&self, f: &Frame, i: usize) -> f64 {
        0.5 / self.scale(f, i)
    }

    fn shown(&self, i: usize, v: f64) -> f64 {
        if self.flip[i] { 1.0 - v } else { v }
    }

    fn view(&self, f: &Frame, i: usize) -> f64 {
        let (a, s) = (f.anchor[i], self.size[i]);
        a + (0.5 - f.now[i] - (a - 0.5) * s) / self.scale(f, i)
    }

    fn place(&self, f: &Frame, i: usize, v: f64) -> f64 {
        let (a, s) = (f.anchor[i], self.size[i]);
        let at = 0.5 - (a - 0.5) * s - self.scale(f, i) * (v - a);
        at.clamp(0.5 - Picture::REACH, 0.5 + Picture::REACH)
    }

    fn goal(&self, f: &Frame, i: usize) -> Option<f64> {
        let b = f.seen?;
        let (half, c, w) = (self.half(f, i), self.shown(i, b.center[i]), b.size[i]);
        Some(match i == 1 && w > 2.0 * half {
            true => c - w / 2.0 + half,
            false => c,
        })
    }

    fn held(&self, f: &Frame, i: usize, v: f64) -> f64 {
        let half = self.half(f, i);
        if half >= 0.5 {
            0.5
        } else {
            v.clamp(half, 1.0 - half)
        }
    }

    pub fn picked(&self, f: &Frame, area: Seen) -> Seen {
        let at = |i: usize| {
            let (a, s) = (f.anchor[i], self.size[i]);
            let v = a + (area.center[i] - f.now[i] - (a - 0.5) * s) / self.scale(f, i);
            self.shown(i, v)
        };
        Seen {
            center: [at(0), at(1)],
            size: [0, 1].map(|i| area.size[i] / self.scale(f, i)),
        }
    }
}

pub fn axes(mode: ReframeMode, frames: &[Frame], shape: &Shape) -> [bool; 2] {
    let room = |i: usize| frames.iter().any(|f| shape.half(f, i) < 0.5 - ROOM);
    match mode {
        ReframeMode::Auto => [room(0), room(1)],
        ReframeMode::Pan => [true, false],
        ReframeMode::Tilt => [false, true],
    }
}

fn shots(cuts: impl Iterator<Item = bool>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (t, cut) in cuts.enumerate() {
        match out.last_mut() {
            Some(last) if !cut => last.end = t + 1,
            _ => out.push(t..t + 1),
        }
    }
    out
}

fn nearest(goals: &[Option<[f64; 2]>], t: usize) -> Option<[f64; 2]> {
    let before = goals[..=t].iter().rposition(Option::is_some);
    let after = goals[t..].iter().position(Option::is_some).map(|d| t + d);
    let near = match (before, after) {
        (Some(b), Some(a)) if a - t < t - b => a,
        (Some(b), _) => b,
        (None, a) => a?,
    };
    goals[near]
}

fn views(frames: &[Frame], shape: &Shape, sigma: f64) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(frames.len());
    for shot in shots(frames.iter().map(|f| f.cut)) {
        let part = &frames[shot];
        let goals: Vec<Option<[f64; 2]>> = part
            .iter()
            .map(|f| Some([shape.goal(f, 0)?, shape.goal(f, 1)?]))
            .collect();
        let any = goals.iter().any(Option::is_some);
        let path: Vec<[f64; 2]> = (0..part.len())
            .map(|t| match any {
                true => nearest(&goals, t).unwrap_or([0.5; 2]),
                false => [0, 1].map(|i| shape.view(&part[t], i)),
            })
            .collect();
        let smooth = match sigma < 0.5 {
            true => path,
            false => crate::stabilize::fitted(&path, sigma, |_, _, _| 1.0)
                .into_iter()
                .map(|(goal, _)| goal)
                .collect(),
        };
        let held = part.iter().zip(smooth);
        out.extend(held.map(|(f, v)| [0, 1].map(|i| shape.held(f, i, v[i]))));
    }
    out
}

fn kept(points: &[(i64, [f64; 2])], tolerance: f64) -> Vec<bool> {
    let mut keep = vec![false; points.len()];
    if points.is_empty() {
        return keep;
    }
    let last = points.len() - 1;
    (keep[0], keep[last]) = (true, true);
    let mut parts = vec![(0, last)];
    while let Some((a, b)) = parts.pop() {
        let (pa, pb) = (points[a], points[b]);
        let off = |i: usize| {
            let (k, p) = points[i];
            let t = (k - pa.0) as f64 / (pb.0 - pa.0).max(1) as f64;
            let line = [0, 1].map(|j| pa.1[j] + (pb.1[j] - pa.1[j]) * t);
            (p[0] - line[0]).abs().max((p[1] - line[1]).abs())
        };
        let worst = (a + 1..b)
            .map(|i| (off(i), i))
            .max_by(|x, y| x.0.total_cmp(&y.0));
        if let Some((far, i)) = worst
            && far > tolerance
        {
            keep[i] = true;
            parts.push((a, i));
            parts.push((i, b));
        }
    }
    keep
}

fn rounded(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

pub fn moved(frames: &[Frame], shape: &Shape, rules: &Rules) -> Moved {
    let axes = axes(rules.mode, frames, shape);
    let views = views(frames, shape, rules.smooth * shape.fps);
    let places: Vec<[f64; 2]> = frames
        .iter()
        .zip(&views)
        .map(|(f, v)| {
            [0, 1].map(|i| match axes[i] {
                true => shape.place(f, i, v[i]),
                false => f.now[i],
            })
        })
        .collect();
    let mut out = Moved {
        axes,
        seen: frames.iter().filter(|f| f.seen.is_some()).count(),
        ..Moved::default()
    };
    for shot in shots(frames.iter().map(|f| f.cut)) {
        let points: Vec<(i64, [f64; 2])> = shot
            .clone()
            .map(|t| {
                let pixels = [0, 1].map(|i| places[t][i] * shape.pixels[i]);
                (frames[t].key, pixels)
            })
            .collect();
        let keep = kept(&points, rules.tolerance);
        for (t, _) in shot.zip(keep).filter(|(_, k)| *k) {
            out.keys.push(Keyframe {
                frame: frames[t].key,
                value: rounded(places[t][0]),
                in_handle: None,
                out_handle: None,
            });
            out.y.push(rounded(places[t][1]));
        }
    }
    out
}

pub fn found(paths: &[FoundPath], files: &[i64], cuts: &[bool]) -> Vec<Option<Seen>> {
    let paths: Vec<MotionPath> = paths
        .iter()
        .map(|p| MotionPath {
            subject: None,
            tracked: false,
            samples: Arc::new(p.samples.clone()),
        })
        .collect();
    let at = |p: &MotionPath, file: i64| {
        p.area_at(file as f64).map(|a| Seen {
            center: a.center,
            size: a.size,
        })
    };
    let mut out = vec![None; files.len()];
    for shot in shots(cuts.iter().copied()) {
        let weight = |p: &MotionPath| -> f64 {
            let areas = files[shot.clone()].iter().filter_map(|&f| at(p, f));
            areas.map(|s| s.size[0] * s.size[1]).sum()
        };
        let mut order: Vec<(f64, &MotionPath)> = paths
            .iter()
            .map(|p| (weight(p), p))
            .filter(|(w, _)| *w > 0.0)
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        for t in shot {
            out[t] = order.iter().find_map(|(_, p)| at(p, files[t]));
        }
    }
    out
}

pub fn tracked(found: &Tracked, base: i64, files: &[i64]) -> Vec<Option<Seen>> {
    files
        .iter()
        .map(|&f| {
            let i = usize::try_from(f - base - found.first).ok()?;
            let b = found.found.get(i)?;
            Some(Seen {
                center: b.center.map(f64::from),
                size: b.size.map(f64::from),
            })
        })
        .collect()
}
