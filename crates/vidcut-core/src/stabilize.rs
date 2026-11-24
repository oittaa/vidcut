use std::sync::Arc;

use crate::analysis::Shake;
use crate::model::{Stabilized, Stabilizing};

const SMOOTH_SECS: f64 = 2.0;
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
    let steps: Vec<Matrix> = shake.steps.iter().map(widened).collect();
    let mut path = vec![SAME];
    for m in &steps {
        let last = path[path.len() - 1];
        path.push(times(m, &last));
    }
    let lock = values.camera_lock;
    let goal = match lock {
        true => vec![SAME; path.len()],
        false => smoothed(&path, values.smooth() * SMOOTH_SECS * shot.fps),
    };
    let fixes: Vec<Matrix> = goal
        .iter()
        .zip(&path)
        .map(|(g, c)| times(g, &inverse(c)))
        .collect();
    let fixes = match lock {
        true => fixes,
        false => limited(&fixes, values.cropping_ratio(), aspect, shot.fps),
    };
    let strength = if lock { 1.0 } else { values.strength() };
    let fixes: Vec<Matrix> = fixes.iter().map(|f| blend(f, strength)).collect();
    let zoom = if values.zoom {
        zoom_for(&fixes, aspect)
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
    let frames = forward.iter().map(|f| to_uv(&inverse(f), aspect)).collect();
    Steadied {
        made: Stabilized {
            with: values.normal(),
            first: shot.first,
            frames: Arc::new(frames),
        },
        frames: forward.len(),
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
    normalized(std::array::from_fn(|i| SAME[i] + k * (m[i] - SAME[i])))
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

fn smoothed(path: &[Matrix], sigma: f64) -> Vec<Matrix> {
    if sigma < 0.5 {
        return path.to_vec();
    }
    let w = weights(sigma);
    let reach = (w.len() / 2) as i64;
    let n = path.len() as i64;
    (0..n)
        .map(|t| {
            let mut sum = [0.0; 9];
            let mut total = 0.0;
            for (k, wk) in w.iter().enumerate() {
                let at = t + k as i64 - reach;
                if (0..n).contains(&at) {
                    let m = &path[at as usize];
                    for (s, v) in sum.iter_mut().zip(m) {
                        *s += wk * v;
                    }
                    total += wk;
                }
            }
            normalized(sum.map(|s| s / total))
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

fn zoom_for(fixes: &[Matrix], aspect: f64) -> f64 {
    let least = Stabilizing::CROPPING_RATIO.0;
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
