use std::sync::Arc;

use crate::analysis::Shake;
use crate::model::{Stabilized, Stabilizing};

const SMOOTH_SECS: f64 = 1.5;
const SHAKE_RANGE: f64 = 1.2;
const NEAR_SECS: f64 = 0.5;
const SEARCH_STEPS: usize = 24;
const REACH: f64 = 1e-9;
const SPOT: f64 = 0.35;

type Matrix = [f64; 9];

const SAME: Matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub first: i64,
    pub fps: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Motion {
    pub motion: f64,
    pub shake: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Steadied {
    pub made: Stabilized,
    pub frames: usize,
    pub zoom: f64,
    pub before: Motion,
    pub after: Motion,
}

pub fn steadied(shake: &Shake, values: &Stabilizing, shot: Shot) -> Steadied {
    let (w, h) = shake.size;
    let aspect = f64::from(w.max(1)) / f64::from(h.max(1));
    let steps: Vec<Matrix> = shake.steps.iter().flatten().map(widened).collect();
    let all = shake.steps.len() + 1;
    let rate = shot.fps * (steps.len() + 1) as f64 / all as f64;
    let lock = values.camera_lock;
    let fixes = match lock {
        true => locked(&steps),
        false => {
            let smooth = values.smooth();
            let fixes = smoothed(
                &steps,
                smooth * SMOOTH_SECS * rate,
                smooth * SHAKE_RANGE,
                aspect,
            );
            let fixes: Vec<Matrix> = fixes.iter().map(|f| blend(f, values.strength())).collect();
            limited(&fixes, values.cropping_ratio(), aspect, rate)
        }
    };
    let least = match lock {
        true => Stabilizing::CROPPING_RATIO.0,
        false => values.cropping_ratio(),
    };
    let zoom = if values.zoom {
        zoom_for(&fixes, aspect, least)
    } else {
        1.0
    };
    let scale = [zoom, 0.0, 0.0, 0.0, zoom, 0.0, 0.0, 0.0, 1.0];
    let forward: Vec<Matrix> = fixes.iter().map(|f| times(&scale, f)).collect();
    let after: Vec<Matrix> = steps
        .iter()
        .enumerate()
        .map(|(t, m)| times(&times(&forward[t + 1], m), &inverse(&forward[t])))
        .collect();
    let shown: Vec<[f32; 8]> = forward.iter().map(|f| to_uv(&inverse(f), aspect)).collect();
    let picture = shake.steps.iter().scan(0, |at, s| {
        *at += usize::from(s.is_some());
        Some(*at)
    });
    let frames = std::iter::once(0)
        .chain(picture)
        .map(|k| shown[k])
        .collect();
    Steadied {
        made: Stabilized {
            with: values.normal(),
            first: shot.first,
            frames: Arc::new(frames),
        },
        frames: all,
        zoom,
        before: moved(&steps, aspect, shot.height),
        after: moved(&after, aspect, shot.height),
    }
}

fn widened(m: &[f32; 8]) -> Matrix {
    let [a, b, c, d, e, f, g, h] = m.map(f64::from);
    [a, b, c, d, e, f, g, h, 1.0]
}

fn normalized(m: Matrix) -> Matrix {
    match m[8].abs() > REACH {
        true => m.map(|v| v / m[8]),
        false => m,
    }
}

fn times(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
        }
    }
    normalized(out)
}

fn inverse(m: &Matrix) -> Matrix {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < REACH {
        return SAME;
    }
    normalized([
        (e * i - f * h) / det,
        (c * h - b * i) / det,
        (b * f - c * e) / det,
        (f * g - d * i) / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        (d * h - e * g) / det,
        (b * g - a * h) / det,
        (a * e - b * d) / det,
    ])
}

fn apply(m: &Matrix, [x, y]: [f64; 2]) -> Option<[f64; 2]> {
    let w = m[6] * x + m[7] * y + m[8];
    (w > REACH).then(|| {
        [
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        ]
    })
}

fn blend(m: &Matrix, k: f64) -> Matrix {
    from_parts(parts(m).map(|v| k * v))
}

fn weights(sigma: f64) -> Vec<f64> {
    let reach = (3.0 * sigma).ceil().max(0.0) as usize;
    (0..=2 * reach)
        .map(|i| {
            let x = i as f64 - reach as f64;
            (-x * x / (2.0 * sigma * sigma)).exp()
        })
        .collect()
}

fn locked(steps: &[Matrix]) -> Vec<Matrix> {
    let mut path = vec![SAME];
    for m in steps {
        let last = path[path.len() - 1];
        path.push(times(m, &last));
    }
    path.iter().map(inverse).collect()
}

type Parts = [f64; 8];

fn parts(m: &Matrix) -> Parts {
    let [a, b, x, c, d, y, g, h, _] = *m;
    let (even, odd) = ((a + d) / 2.0, (c - b) / 2.0);
    let size = even.hypot(odd).max(REACH);
    [
        x,
        y,
        odd.atan2(even),
        size.ln(),
        (a - d) / 2.0 / size,
        (b + c) / 2.0 / size,
        g,
        h,
    ]
}

fn from_parts([x, y, turn, grow, stretch, shear, g, h]: Parts) -> Matrix {
    let size = grow.exp();
    let (cos, sin) = (size * turn.cos(), size * turn.sin());
    let (stretch, shear) = (size * stretch, size * shear);
    [
        cos + stretch,
        shear - sin,
        x,
        sin + shear,
        cos - stretch,
        y,
        g,
        h,
        1.0,
    ]
}

fn smoothed(steps: &[Matrix], sigma: f64, range: f64, aspect: f64) -> Vec<Matrix> {
    let mut path: Vec<Parts> = vec![[0.0; 8]];
    for m in steps {
        let (last, step) = (path[path.len() - 1], parts(m));
        path.push(std::array::from_fn(|i| last[i] + step[i]));
    }
    if sigma < 0.5 {
        return vec![SAME; path.len()];
    }
    let (x, y) = (aspect / 2.0, 0.5);
    let corners = [[-x, -y], [x, -y], [x, y], [-x, y]];
    let lines = fitted(&path, sigma, |_, _, _| 1.0);
    let goals = fitted(&path, sigma, |t, at, d| {
        let off = from_parts(std::array::from_fn(|i| {
            path[at][i] - path[t][i] - lines[t].1[i] * d
        }));
        (-spread(&off, &corners) / (2.0 * range * range)).exp()
    });
    (0..path.len())
        .map(|t| from_parts(std::array::from_fn(|i| goals[t].0[i] - path[t][i])))
        .collect()
}

fn spread(m: &Matrix, corners: &[[f64; 2]; 4]) -> f64 {
    let moves = corners.iter().map(|c| {
        apply(m, *c).map_or(f64::INFINITY, |[u, v]| {
            (u - c[0]).powi(2) + (v - c[1]).powi(2)
        })
    });
    moves.sum::<f64>() / corners.len() as f64
}

fn fitted(
    path: &[Parts],
    sigma: f64,
    weight: impl Fn(usize, usize, f64) -> f64,
) -> Vec<(Parts, Parts)> {
    let w = weights(sigma);
    let reach = (w.len() / 2) as i64;
    let n = path.len() as i64;
    (0..n)
        .map(|t| {
            let (mut level, mut slope) = ([0.0; 8], [0.0; 8]);
            let (mut s0, mut s1, mut s2) = (0.0, 0.0, 0.0);
            for (k, wk) in w.iter().enumerate() {
                let d = k as i64 - reach;
                let at = t + d;
                if (0..n).contains(&at) {
                    let d = d as f64;
                    let wk = wk * weight(t as usize, at as usize, d);
                    for (i, v) in path[at as usize].iter().enumerate() {
                        level[i] += wk * v;
                        slope[i] += wk * d * v;
                    }
                    (s0, s1, s2) = (s0 + wk, s1 + wk * d, s2 + wk * d * d);
                }
            }
            let det = s0 * s2 - s1 * s1;
            let goal: Parts = std::array::from_fn(|i| match det > REACH {
                true => (s2 * level[i] - s1 * slope[i]) / det,
                false => level[i] / s0,
            });
            let speed: Parts = std::array::from_fn(|i| match det > REACH {
                true => (s0 * slope[i] - s1 * level[i]) / det,
                false => 0.0,
            });
            (goal, speed)
        })
        .collect()
}

fn fits(back: &Matrix, share: f64, aspect: f64) -> bool {
    let (x, y) = (share * aspect / 2.0, share / 2.0);
    let (ex, ey) = (aspect / 2.0 + 1e-6, 0.5 + 1e-6);
    [[-x, -y], [x, -y], [x, y], [-x, y]]
        .iter()
        .all(|c| apply(back, *c).is_some_and(|[u, v]| u.abs() <= ex && v.abs() <= ey))
}

fn largest(lo: f64, hi: f64, ok: impl Fn(f64) -> bool) -> f64 {
    if ok(hi) {
        return hi;
    }
    let (mut lo, mut hi) = (lo, hi);
    for _ in 0..SEARCH_STEPS {
        let mid = (lo + hi) / 2.0;
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn limited(fixes: &[Matrix], share: f64, aspect: f64, fps: f64) -> Vec<Matrix> {
    let room: Vec<f64> = fixes
        .iter()
        .map(|f| largest(0.0, 1.0, |k| fits(&inverse(&blend(f, k)), share, aspect)))
        .collect();
    let near = (NEAR_SECS * fps).round().max(1.0) as i64;
    let n = room.len() as i64;
    let least: Vec<f64> = (0..n)
        .map(|t| {
            let span = (t - near).max(0)..=(t + near).min(n - 1);
            span.map(|k| room[k as usize]).fold(1.0, f64::min)
        })
        .collect();
    let w = weights(near as f64 / 3.0);
    let reach = (w.len() / 2) as i64;
    (0..n)
        .map(|t| {
            let (mut sum, mut total) = (0.0, 0.0);
            for (k, wk) in w.iter().enumerate() {
                let at = t + k as i64 - reach;
                if (0..n).contains(&at) {
                    sum += wk * least[at as usize];
                    total += wk;
                }
            }
            blend(&fixes[t as usize], sum / total)
        })
        .collect()
}

fn zoom_for(fixes: &[Matrix], aspect: f64, least: f64) -> f64 {
    let backs: Vec<Matrix> = fixes.iter().map(inverse).collect();
    let share = largest(least, 1.0, |q| backs.iter().all(|b| fits(b, q, aspect)));
    1.0 / share
}

fn to_uv(back: &Matrix, aspect: f64) -> [f32; 8] {
    let into = [1.0 / aspect, 0.0, 0.5, 0.0, 1.0, 0.5, 0.0, 0.0, 1.0];
    let out = [aspect, 0.0, -aspect / 2.0, 0.0, 1.0, -0.5, 0.0, 0.0, 1.0];
    let m = times(&times(&into, back), &out);
    std::array::from_fn(|i| m[i] as f32)
}

fn moved(maps: &[Matrix], aspect: f64, height: f64) -> Motion {
    let (x, y) = (SPOT * aspect, SPOT);
    let spots = [[0.0, 0.0], [-x, -y], [x, -y], [x, y], [-x, y]];
    let moves: Vec<Vec<[f64; 2]>> = maps
        .iter()
        .map(|m| {
            spots
                .iter()
                .map(|p| {
                    let [u, v] = apply(m, *p).unwrap_or(*p);
                    [(u - p[0]) * height, (v - p[1]) * height]
                })
                .collect()
        })
        .collect();
    let mean = |values: Vec<f64>| match values.len() {
        0 => 0.0,
        n => values.iter().sum::<f64>() / n as f64,
    };
    let size = |[a, b]: [f64; 2]| a.hypot(b);
    let motion = mean(moves.iter().flatten().map(|v| size(*v)).collect());
    let shake = mean(
        moves
            .windows(2)
            .flat_map(|w| w[0].iter().zip(&w[1]))
            .map(|(a, b)| size([b[0] - a[0], b[1] - a[1]]))
            .collect(),
    );
    Motion { motion, shake }
}
