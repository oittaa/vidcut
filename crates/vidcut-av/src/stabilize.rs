use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use vidcut_core::analysis::{Shake, Shaky};
use vidcut_core::model::StabilizeMode;
use vidcut_core::render::RenderPlan;

use crate::error::{Error, Result};
use crate::ffmpeg::frame;
use crate::tracking::{Reader, Shown, going};

mod gpu;

const SIDE: f64 = 640.0;
const LEVELS: usize = 4;
const GRID: (usize, usize) = (24, 14);
const EDGE: usize = 8;
const HALF: i32 = 6;
const ROUNDS: usize = 12;
const SETTLED: f32 = 0.01;
const RETURN: f32 = 0.75;
const QUALITY: f32 = 0.02;
const FLAT: f32 = 0.01;
const INLIER: f64 = 1.5;
const TRIES: usize = 400;
const LEAST: usize = 8;
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;
const REPEAT: f32 = 0.25;

pub fn name() -> &'static str {
    "steady2"
}

type Pairs = Vec<([f32; 2], [f32; 2])>;

enum Seen {
    First,
    Same,
    Moved(Pairs),
}

enum Measuring {
    Cpu(Option<Vec<Gray>>),
    Gpu(Box<gpu::Measuring>),
}

impl Measuring {
    fn seen(&mut self, frame: &frame::Video) -> Result<Seen> {
        match self {
            Self::Gpu(g) => g.seen(frame),
            Self::Cpu(last) => {
                let levels = pyramid(Gray::of(frame));
                let seen = match last.as_ref() {
                    None => Seen::First,
                    Some(before) if repeats(&before[0], &levels[0]) => Seen::Same,
                    Some(before) => Seen::Moved(pairs(before, &levels)),
                };
                *last = Some(levels);
                Ok(seen)
            }
        }
    }
}

pub fn measure(
    plan: &RenderPlan,
    look: &Shaky,
    gpu: Option<&crate::hw::GpuDecoder>,
    source: &Path,
    on_gpu: bool,
) -> Result<Shake> {
    crate::cancel::stop()?;
    let _turn = crate::turns::cpu_analysis()?;
    let size = working(plan.width, plan.height);
    let mut measuring = match on_gpu {
        true => Measuring::Gpu(Box::new(gpu::Measuring::start(size)?)),
        false => Measuring::Cpu(None),
    };
    let reader = Reader { plan, size, gpu };
    let (lo, hi) = (look.frames.0.max(0), look.frames.1.min(plan.frames));
    if hi <= lo {
        return Err(Error::Invalid("the clip has no frame to stabilize".into()));
    }
    let shown = Shown {
        path: source,
        started: Instant::now(),
        all: (hi - lo) as f64,
    };
    let Some(mut run) = going(reader.run(lo, hi))? else {
        return Err(stopped());
    };
    let mut steps = Vec::new();
    for done in 0..hi - lo {
        let frame = match going(run.next_frame())? {
            None => return Err(stopped()),
            Some(None) => {
                return Err(Error::Invalid(format!(
                    "the picture of the clip ends before frame {}",
                    lo + done
                )));
            }
            Some(Some(f)) => f,
        };
        crate::cancel::stop()?;
        match measuring.seen(&frame)? {
            Seen::First => {}
            Seen::Same => steps.push(None),
            Seen::Moved(pairs) => steps.push(Some(moved(&pairs, size, look.mode))),
        }
        shown.report(done as usize + 1);
    }
    Ok(Shake { size, steps })
}

fn repeats(before: &Gray, after: &Gray) -> bool {
    let n = before.px.len().max(1) as f32;
    let apart: f32 = before
        .px
        .iter()
        .zip(&after.px)
        .map(|(a, b)| (a - b).abs())
        .sum();
    apart / n < REPEAT
}

fn stopped() -> Error {
    Error::Invalid(crate::export::CANCELLED.into())
}

fn working(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let scale = (SIDE / w.max(h)).min(1.0);
    let side = |v: f64| ((v * scale).round() as u32).max(2);
    (side(w), side(h))
}

struct Gray {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Gray {
    fn of(frame: &frame::Video) -> Self {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let (stride, data) = (frame.stride(0), frame.data(0));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let row = &data[y * stride..y * stride + w * 3];
            px.extend(row.as_chunks::<3>().0.iter().map(|&[r, g, b]| {
                0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)
            }));
        }
        Self { w, h, px }
    }

    fn get(&self, x: usize, y: usize) -> f32 {
        self.px[y.min(self.h - 1) * self.w + x.min(self.w - 1)]
    }

    fn at(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let top = self.get(x0, y0) + (self.get(x0 + 1, y0) - self.get(x0, y0)) * fx;
        let low = self.get(x0, y0 + 1) + (self.get(x0 + 1, y0 + 1) - self.get(x0, y0 + 1)) * fx;
        top + (low - top) * fy
    }

    fn blurred(&self) -> Self {
        let (w, h) = (self.w, self.h);
        let mut across = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (l, r) = (x.saturating_sub(1), (x + 1).min(w - 1));
                across[y * w + x] = (self.get(l, y) + 2.0 * self.get(x, y) + self.get(r, y)) / 4.0;
            }
        }
        let mut px = vec![0.0; w * h];
        for y in 0..h {
            let (u, d) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                px[y * w + x] =
                    (across[u * w + x] + 2.0 * across[y * w + x] + across[d * w + x]) / 4.0;
            }
        }
        Self { w, h, px }
    }

    fn half(&self) -> Self {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (2 * x, 2 * y);
                let sum = self.get(sx, sy)
                    + self.get(sx + 1, sy)
                    + self.get(sx, sy + 1)
                    + self.get(sx + 1, sy + 1);
                px.push(sum / 4.0);
            }
        }
        Self { w, h, px }
    }
}

fn pyramid(gray: Gray) -> Vec<Gray> {
    let mut levels = vec![gray.blurred()];
    while levels.len() < LEVELS {
        let next = levels[levels.len() - 1].blurred().half();
        levels.push(next);
    }
    levels
}

fn corners(g: &Gray) -> Vec<[f32; 2]> {
    let (w, h) = (g.w, g.h);
    if w <= 2 * EDGE || h <= 2 * EDGE {
        return Vec::new();
    }
    let (cols, rows) = GRID;
    let (inner_w, inner_h) = (w - 2 * EDGE, h - 2 * EDGE);
    let cells: Vec<Mutex<(f32, usize, usize)>> =
        (0..cols * rows).map(|_| Mutex::new((0.0, 0, 0))).collect();
    crate::pool::run(cells.len(), &|i| {
        let (r, c) = (i / cols, i % cols);
        let xs = (EDGE + c * inner_w / cols, EDGE + (c + 1) * inner_w / cols);
        let ys = (EDGE + r * inner_h / rows, EDGE + (r + 1) * inner_h / rows);
        *crate::pool::lock(&cells[i]) = best(g, xs, ys);
    });
    let cells: Vec<(f32, usize, usize)> = cells
        .into_iter()
        .map(|c| c.into_inner().unwrap_or_else(|e| e.into_inner()))
        .collect();
    let top = cells.iter().fold(0.0f32, |top, c| top.max(c.0));
    cells
        .into_iter()
        .filter(|c| c.0 > QUALITY * top && c.0 > 0.0)
        .map(|(_, x, y)| [x as f32, y as f32])
        .collect()
}

fn best(g: &Gray, (x0, x1): (usize, usize), (y0, y1): (usize, usize)) -> (f32, usize, usize) {
    let r = 2;
    let (left, top) = (x0 - r, y0 - r);
    let (pw, ph) = (x1 - x0 + 2 * r, y1 - y0 + 2 * r);
    let n = pw * ph;
    let (mut xx, mut xy, mut yy) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
    for j in 0..ph {
        for i in 0..pw {
            let at = (top + j) * g.w + left + i;
            let gx = (g.px[at + 1] - g.px[at - 1]) * 0.5;
            let gy = (g.px[at + g.w] - g.px[at - g.w]) * 0.5;
            (xx[j * pw + i], xy[j * pw + i], yy[j * pw + i]) = (gx * gx, gx * gy, gy * gy);
        }
    }
    let across = |v: &[f32]| -> Vec<f32> {
        let mut out = vec![0.0f32; n];
        for j in 0..ph {
            for i in r..pw - r {
                out[j * pw + i] = (i - r..=i + r).map(|k| v[j * pw + k]).sum();
            }
        }
        out
    };
    let (xx, xy, yy) = (across(&xx), across(&xy), across(&yy));
    let down =
        |v: &[f32], i: usize, j: usize| -> f32 { (j - r..=j + r).map(|k| v[k * pw + i]).sum() };
    let mut found = (0.0f32, 0, 0);
    for j in r..ph - r {
        for i in r..pw - r {
            let (a, b, c) = (down(&xx, i, j), down(&xy, i, j), down(&yy, i, j));
            let least = (a + c) / 2.0 - (((a - c) / 2.0).powi(2) + b * b).sqrt();
            if least > found.0 {
                found = (least, left + i, top + j);
            }
        }
    }
    found
}

fn on_level(v: f32, level: usize) -> f32 {
    let k = (1 << level) as f32;
    (v + 0.5) / k - 0.5
}

fn follow(from: &[Gray], to: &[Gray], p: [f32; 2]) -> Option<[f32; 2]> {
    let mut g = [0.0f32; 2];
    let side = (2 * HALF + 1) as usize;
    let n = side * side;
    let (mut t, mut ix, mut iy) = (vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    for level in (0..from.len()).rev() {
        let (a, b) = (&from[level], &to[level]);
        let (px, py) = (on_level(p[0], level), on_level(p[1], level));
        let (mut gxx, mut gxy, mut gyy) = (0.0f32, 0.0f32, 0.0f32);
        let mut k = 0;
        for dy in -HALF..=HALF {
            for dx in -HALF..=HALF {
                let (x, y) = (px + dx as f32, py + dy as f32);
                t[k] = a.at(x, y);
                ix[k] = (a.at(x + 1.0, y) - a.at(x - 1.0, y)) * 0.5;
                iy[k] = (a.at(x, y + 1.0) - a.at(x, y - 1.0)) * 0.5;
                gxx += ix[k] * ix[k];
                gxy += ix[k] * iy[k];
                gyy += iy[k] * iy[k];
                k += 1;
            }
        }
        let least = (gxx + gyy) / 2.0 - (((gxx - gyy) / 2.0).powi(2) + gxy * gxy).sqrt();
        let det = gxx * gyy - gxy * gxy;
        if least < FLAT * n as f32 || det <= 0.0 {
            return None;
        }
        let mut v = [0.0f32; 2];
        for _ in 0..ROUNDS {
            let (mut bx, mut by) = (0.0f32, 0.0f32);
            let mut k = 0;
            for dy in -HALF..=HALF {
                for dx in -HALF..=HALF {
                    let x = px + dx as f32 + g[0] + v[0];
                    let y = py + dy as f32 + g[1] + v[1];
                    let diff = t[k] - b.at(x, y);
                    bx += diff * ix[k];
                    by += diff * iy[k];
                    k += 1;
                }
            }
            let dvx = (gyy * bx - gxy * by) / det;
            let dvy = (gxx * by - gxy * bx) / det;
            v = [v[0] + dvx, v[1] + dvy];
            if dvx.abs() < SETTLED && dvy.abs() < SETTLED {
                break;
            }
        }
        g = match level {
            0 => [g[0] + v[0], g[1] + v[1]],
            _ => [2.0 * (g[0] + v[0]), 2.0 * (g[1] + v[1])],
        };
    }
    let q = [p[0] + g[0], p[1] + g[1]];
    let inside = |v: f32, side: usize| v.is_finite() && v >= 0.0 && v <= (side - 1) as f32;
    (inside(q[0], to[0].w) && inside(q[1], to[0].h)).then_some(q)
}

fn pairs(before: &[Gray], after: &[Gray]) -> Pairs {
    let starts = corners(&before[0]);
    let found: Vec<Mutex<Option<[f32; 2]>>> = starts.iter().map(|_| Mutex::new(None)).collect();
    crate::pool::run(starts.len(), &|i| {
        *crate::pool::lock(&found[i]) = followed(before, after, starts[i]);
    });
    starts
        .into_iter()
        .zip(found)
        .filter_map(|(p, q)| Some((p, q.into_inner().unwrap_or_else(|e| e.into_inner())?)))
        .collect()
}

fn followed(before: &[Gray], after: &[Gray], p: [f32; 2]) -> Option<[f32; 2]> {
    let q = follow(before, after, p)?;
    let back = follow(after, before, q)?;
    match (back[0] - p[0]).hypot(back[1] - p[1]) > RETURN {
        true => None,
        false => Some(q),
    }
}

fn moved(pairs: &[([f32; 2], [f32; 2])], (w, h): (u32, u32), mode: StabilizeMode) -> [f32; 8] {
    let (w, h) = (f64::from(w), f64::from(h));
    let centered = |[x, y]: [f32; 2]| {
        [
            (f64::from(x) + 0.5 - w / 2.0) / h,
            (f64::from(y) + 0.5 - h / 2.0) / h,
        ]
    };
    let from: Vec<[f64; 2]> = pairs.iter().map(|(p, _)| centered(*p)).collect();
    let to: Vec<[f64; 2]> = pairs.iter().map(|(_, q)| centered(*q)).collect();
    let m = fitted(mode, &from, &to, INLIER / h).unwrap_or(SAME);
    std::array::from_fn(|i| (m[i] / m[8]) as f32)
}

type Matrix = [f64; 9];

const SAME: Matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

fn least_points(mode: StabilizeMode) -> usize {
    match mode {
        StabilizeMode::Translation => 1,
        StabilizeMode::Similarity => 2,
        StabilizeMode::Perspective => 4,
    }
}

fn solve(
    mode: StabilizeMode,
    from: &[[f64; 2]],
    to: &[[f64; 2]],
    pick: &[usize],
) -> Option<Matrix> {
    match mode {
        StabilizeMode::Translation => translation(from, to, pick),
        StabilizeMode::Similarity => similarity(from, to, pick),
        StabilizeMode::Perspective => perspective(from, to, pick),
    }
}

fn translation(from: &[[f64; 2]], to: &[[f64; 2]], pick: &[usize]) -> Option<Matrix> {
    let n = pick.len() as f64;
    let (mut x, mut y) = (0.0, 0.0);
    for &i in pick {
        x += to[i][0] - from[i][0];
        y += to[i][1] - from[i][1];
    }
    (n > 0.0).then(|| [1.0, 0.0, x / n, 0.0, 1.0, y / n, 0.0, 0.0, 1.0])
}

fn similarity(from: &[[f64; 2]], to: &[[f64; 2]], pick: &[usize]) -> Option<Matrix> {
    let n = pick.len() as f64;
    let mean = |v: &[[f64; 2]]| {
        let s = pick
            .iter()
            .fold([0.0, 0.0], |s, &i| [s[0] + v[i][0], s[1] + v[i][1]]);
        [s[0] / n, s[1] / n]
    };
    let (p, q) = (mean(from), mean(to));
    let (mut a, mut b, mut spread) = (0.0, 0.0, 0.0);
    for &i in pick {
        let (px, py) = (from[i][0] - p[0], from[i][1] - p[1]);
        let (qx, qy) = (to[i][0] - q[0], to[i][1] - q[1]);
        a += px * qx + py * qy;
        b += px * qy - py * qx;
        spread += px * px + py * py;
    }
    if spread < 1e-12 {
        return None;
    }
    let (a, b) = (a / spread, b / spread);
    let tx = q[0] - (a * p[0] - b * p[1]);
    let ty = q[1] - (b * p[0] + a * p[1]);
    Some([a, -b, tx, b, a, ty, 0.0, 0.0, 1.0])
}

fn perspective(from: &[[f64; 2]], to: &[[f64; 2]], pick: &[usize]) -> Option<Matrix> {
    let mut m = [[0.0f64; 9]; 8];
    for &i in pick {
        let ([x, y], [u, v]) = (from[i], to[i]);
        for (row, out) in [
            ([x, y, 1.0, 0.0, 0.0, 0.0, -x * u, -y * u], u),
            ([0.0, 0.0, 0.0, x, y, 1.0, -x * v, -y * v], v),
        ] {
            for r in 0..8 {
                for c in 0..8 {
                    m[r][c] += row[r] * row[c];
                }
                m[r][8] += row[r] * out;
            }
        }
    }
    for c in 0..8 {
        let pivot = (c..8).max_by(|a, b| m[*a][c].abs().total_cmp(&m[*b][c].abs()))?;
        if m[pivot][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, pivot);
        for r in 0..8 {
            if r != c {
                let k = m[r][c] / m[c][c];
                for j in c..9 {
                    m[r][j] -= k * m[c][j];
                }
            }
        }
    }
    let h: [f64; 8] = std::array::from_fn(|r| m[r][8] / m[r][r]);
    let out = [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0];
    out.iter().all(|v| v.is_finite()).then_some(out)
}

fn miss(m: &Matrix, [x, y]: [f64; 2], [u, v]: [f64; 2]) -> f64 {
    let w = m[6] * x + m[7] * y + m[8];
    if w <= 1e-9 {
        return f64::INFINITY;
    }
    let (px, py) = (
        (m[0] * x + m[1] * y + m[2]) / w,
        (m[3] * x + m[4] * y + m[5]) / w,
    );
    (px - u).hypot(py - v)
}

fn fitted(mode: StabilizeMode, from: &[[f64; 2]], to: &[[f64; 2]], near: f64) -> Option<Matrix> {
    let k = least_points(mode);
    let n = from.len();
    if n < LEAST.max(k) {
        return None;
    }
    let inliers =
        |m: &Matrix| -> Vec<usize> { (0..n).filter(|&i| miss(m, from[i], to[i]) < near).collect() };
    let mut seed = SEED ^ n as u64;
    let mut draw = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..TRIES {
        let mut pick = Vec::with_capacity(k);
        while pick.len() < k {
            let i = draw();
            if !pick.contains(&i) {
                pick.push(i);
            }
        }
        let Some(m) = solve(mode, from, to, &pick) else {
            continue;
        };
        let found = inliers(&m);
        if found.len() > best.len() {
            best = found;
        }
    }
    for _ in 0..2 {
        if best.len() < LEAST.max(k) {
            return None;
        }
        let m = solve(mode, from, to, &best)?;
        best = inliers(&m);
    }
    if best.len() < LEAST.max(k) {
        return None;
    }
    solve(mode, from, to, &best)
}
