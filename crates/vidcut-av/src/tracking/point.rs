use vidcut_core::analysis::{Found, Pointed};
use vidcut_core::model::{AdaptiveMode, PatternChannel};

use crate::ffmpeg::frame;

pub const NAME: &str = "point1";
const LONGEST: u32 = 1920;
const COARSE: usize = 16;
const NEAR: f64 = 2.0;
const MARGIN: f64 = 4.0;
const FLAT: f64 = 1e-6;
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
const PICKED: [PatternChannel; 5] = [
    PatternChannel::Luminance,
    PatternChannel::Red,
    PatternChannel::Green,
    PatternChannel::Blue,
    PatternChannel::Alpha,
];

pub struct Rgba {
    pub size: (usize, usize),
    pub data: Vec<u8>,
}

impl Rgba {
    pub fn of(frame: &frame::Video) -> Self {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let (stride, data) = (frame.stride(0), frame.data(0));
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            out.extend_from_slice(&data[y * stride..y * stride + w * 4]);
        }
        Self {
            size: (w, h),
            data: out,
        }
    }

    fn value(&self, x: usize, y: usize, c: PatternChannel) -> f32 {
        let i = (y * self.size.0 + x) * 4;
        let f = |k: usize| f32::from(self.data[i + k]) / 255.0;
        match c {
            PatternChannel::Red => f(0),
            PatternChannel::Green => f(1),
            PatternChannel::Blue => f(2),
            PatternChannel::Alpha => f(3),
            PatternChannel::Auto | PatternChannel::Luminance => {
                LUMA[0] * f(0) + LUMA[1] * f(1) + LUMA[2] * f(2)
            }
        }
    }
}

pub fn working((w, h): (u32, u32)) -> (u32, u32) {
    let long = w.max(h).max(1);
    if long <= LONGEST {
        return (w.max(1), h.max(1));
    }
    let scale = f64::from(LONGEST) / f64::from(long);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
    (side(w), side(h))
}

struct Plane {
    w: usize,
    h: usize,
    v: Vec<f32>,
    origin: [f64; 2],
    scale: f64,
}

impl Plane {
    fn around(p: &Rgba, c: PatternChannel, middle: [f64; 2], half: [f64; 2]) -> Self {
        let (pw, ph) = p.size;
        let span = |m: f64, r: f64, most: usize| {
            let lo = (m - r - MARGIN).floor().clamp(0.0, most as f64) as usize;
            let hi = (m + r + MARGIN).ceil().clamp(lo as f64, most as f64) as usize;
            (lo, hi)
        };
        let (x0, x1) = span(middle[0], half[0], pw);
        let (y0, y1) = span(middle[1], half[1], ph);
        let v = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| p.value(x, y, c))
            .collect();
        Self {
            w: x1 - x0,
            h: y1 - y0,
            v,
            origin: [x0 as f64, y0 as f64],
            scale: 1.0,
        }
    }

    fn at(&self, x: isize, y: isize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.v[y * self.w + x]
    }

    fn sample(&self, x: f64, y: f64) -> f32 {
        let (fx, fy) = (x.floor(), y.floor());
        let (tx, ty) = ((x - fx) as f32, (y - fy) as f32);
        let (ix, iy) = (fx as isize, fy as isize);
        let top = self.at(ix, iy) * (1.0 - tx) + self.at(ix + 1, iy) * tx;
        let low = self.at(ix, iy + 1) * (1.0 - tx) + self.at(ix + 1, iy + 1) * tx;
        top * (1.0 - ty) + low * ty
    }

    fn half(&self) -> Self {
        let (w, h) = (self.w / 2, self.h / 2);
        let v = (0..h)
            .flat_map(|j| (0..w).map(move |i| (2 * i as isize, 2 * j as isize)))
            .map(|(x, y)| {
                let sum = self.at(x, y) + self.at(x + 1, y) + self.at(x, y + 1);
                (sum + self.at(x + 1, y + 1)) / 4.0
            })
            .collect();
        Self {
            w,
            h,
            v,
            origin: self.origin,
            scale: self.scale * 2.0,
        }
    }

    fn local(&self, p: [f64; 2]) -> [f64; 2] {
        [0, 1].map(|i| (p[i] - self.origin[i]) / self.scale)
    }

    fn global(&self, u: [f64; 2]) -> [f64; 2] {
        [0, 1].map(|i| self.origin[i] + u[i] * self.scale)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Patch {
    w: usize,
    h: usize,
    v: Vec<f32>,
    norm: f64,
}

impl Patch {
    fn of(plane: &Plane, middle: [f64; 2], (w, h): (usize, usize)) -> Self {
        let x0 = middle[0] - w as f64 / 2.0;
        let y0 = middle[1] - h as f64 / 2.0;
        let mut v: Vec<f32> = (0..h)
            .flat_map(|j| (0..w).map(move |i| (i, j)))
            .map(|(i, j)| plane.sample(x0 + i as f64, y0 + j as f64))
            .collect();
        let mean = v.iter().map(|x| f64::from(*x)).sum::<f64>() / v.len().max(1) as f64;
        for x in &mut v {
            *x -= mean as f32;
        }
        let norm = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
        Self { w, h, v, norm }
    }

    fn flat(&self) -> bool {
        self.norm * self.norm < FLAT * (self.w * self.h) as f64
    }

    fn like(&self, other: &Self) -> f64 {
        if self.flat() || other.flat() || self.v.len() != other.v.len() {
            return 0.0;
        }
        f64::from(dot(&self.v, &other.v)) / (self.norm * other.norm)
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    let (wide_a, rest_a) = a.as_chunks::<8>();
    let (wide_b, rest_b) = b.as_chunks::<8>();
    for (x, y) in wide_a.iter().zip(wide_b) {
        for ((lane, a), b) in lanes.iter_mut().zip(x).zip(y) {
            *lane += a * b;
        }
    }
    let rest: f32 = rest_a.iter().zip(rest_b).map(|(x, y)| x * y).sum();
    lanes.iter().sum::<f32>() + rest
}

fn sums(plane: &Plane) -> (Vec<f64>, Vec<f64>) {
    let (w, h) = (plane.w + 1, plane.h + 1);
    let (mut one, mut two) = (vec![0.0; w * h], vec![0.0; w * h]);
    for y in 0..plane.h {
        let (mut a, mut b) = (0.0, 0.0);
        for x in 0..plane.w {
            let v = f64::from(plane.v[y * plane.w + x]);
            a += v;
            b += v * v;
            one[(y + 1) * w + x + 1] = one[y * w + x + 1] + a;
            two[(y + 1) * w + x + 1] = two[y * w + x + 1] + b;
        }
    }
    (one, two)
}

fn search(plane: &Plane, t: &Patch, around: [f64; 2], reach: [f64; 2]) -> Option<([f64; 2], f64)> {
    if t.flat() || plane.w < t.w || plane.h < t.h {
        return None;
    }
    let span = |a: f64, r: f64, side: usize, most: usize| {
        let top = (most - side) as f64;
        let lo = (a - side as f64 / 2.0 - r).ceil().clamp(0.0, top) as usize;
        let hi = (a - side as f64 / 2.0 + r).floor().clamp(lo as f64, top) as usize;
        (lo, hi)
    };
    let (x0, x1) = span(around[0], reach[0], t.w, plane.w);
    let (y0, y1) = span(around[1], reach[1], t.h, plane.h);
    let (one, two) = sums(plane);
    let stride = plane.w + 1;
    let rect = |s: &[f64], x: usize, y: usize| {
        let (r, b) = (x + t.w, y + t.h);
        s[b * stride + r] - s[y * stride + r] - s[b * stride + x] + s[y * stride + x]
    };
    let n = (t.w * t.h) as f64;
    let cols = x1 - x0 + 1;
    let mut scores = Vec::with_capacity(cols * (y1 - y0 + 1));
    for oy in y0..=y1 {
        for ox in x0..=x1 {
            let mut sum = 0.0f32;
            for j in 0..t.h {
                let row = (oy + j) * plane.w + ox;
                sum += dot(&t.v[j * t.w..(j + 1) * t.w], &plane.v[row..row + t.w]);
            }
            let (s, q) = (rect(&one, ox, oy), rect(&two, ox, oy));
            let spread = q - s * s / n;
            let score = match spread < FLAT * n {
                true => 0.0,
                false => f64::from(sum) / (t.norm * spread.sqrt()),
            };
            scores.push(score);
        }
    }
    let (best, score) = scores
        .iter()
        .copied()
        .enumerate()
        .fold((0, f64::MIN), |m, (i, v)| if v > m.1 { (i, v) } else { m });
    if score <= 0.0 {
        return None;
    }
    let (bx, by) = (best % cols, best / cols);
    let at = |x: usize, y: usize| scores[y * cols + x];
    let peak = |a: f64, b: f64, c: f64| {
        let curve = a - 2.0 * b + c;
        match curve < -FLAT {
            true => (0.5 * (a - c) / curve).clamp(-0.5, 0.5),
            false => 0.0,
        }
    };
    let dx = match bx > 0 && bx + 1 < cols {
        true => peak(at(bx - 1, by), score, at(bx + 1, by)),
        false => 0.0,
    };
    let rows = y1 - y0 + 1;
    let dy = match by > 0 && by + 1 < rows {
        true => peak(at(bx, by - 1), score, at(bx, by + 1)),
        false => 0.0,
    };
    let x = (x0 + bx) as f64 + dx + t.w as f64 / 2.0;
    let y = (y0 + by) as f64 + dy + t.h as f64 / 2.0;
    Some(([x, y], score))
}

fn picked(p: &Rgba, middle: [f64; 2], (w, h): (usize, usize)) -> PatternChannel {
    let (pw, ph) = p.size;
    let span = |m: f64, side: usize, most: usize| {
        let lo = (m - side as f64 / 2.0).round().clamp(0.0, most as f64) as usize;
        (lo, (lo + side).min(most))
    };
    let (x0, x1) = span(middle[0], w, pw);
    let (y0, y1) = span(middle[1], h, ph);
    let spread = |c: PatternChannel| {
        let values: Vec<f64> = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| f64::from(p.value(x, y, c)))
            .collect();
        let n = values.len().max(1) as f64;
        let mean = values.iter().sum::<f64>() / n;
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n
    };
    let mut best = (PICKED[0], spread(PICKED[0]));
    for c in &PICKED[1..] {
        let s = spread(*c);
        if s > best.1 + FLAT {
            best = (*c, s);
        }
    }
    best.0
}

pub struct Point {
    channel: PatternChannel,
    adaptive: AdaptiveMode,
    tolerance: f64,
    side: (usize, usize),
    reach: [f64; 2],
    size: [f32; 2],
    picture: (usize, usize),
    first: Patch,
    now: Patch,
    coarse: Option<Patch>,
    last: [f64; 2],
}

impl Point {
    pub fn start(p: &Rgba, [cx, cy, w, h]: [f64; 4], look: &Pointed) -> Self {
        let (pw, ph) = (p.size.0 as f64, p.size.1 as f64);
        let side = (w.round().max(3.0) as usize, h.round().max(3.0) as usize);
        let channel = match look.channel {
            PatternChannel::Auto => picked(p, [cx, cy], side),
            c => c,
        };
        let reach = [
            ((f64::from(look.search[0]) * pw - side.0 as f64) / 2.0).max(0.0),
            ((f64::from(look.search[1]) * ph - side.1 as f64) / 2.0).max(0.0),
        ];
        let half = [side.0 as f64 / 2.0, side.1 as f64 / 2.0];
        let plane = Plane::around(p, channel, [cx, cy], half);
        let first = Patch::of(&plane, plane.local([cx, cy]), side);
        let mut point = Self {
            channel,
            adaptive: look.adaptive,
            tolerance: f64::from(look.tolerance),
            side,
            reach,
            size: [(w / pw) as f32, (h / ph) as f32],
            picture: p.size,
            now: first.clone(),
            first,
            coarse: None,
            last: [cx, cy],
        };
        point.coarse = point.halved(&plane, [cx, cy]);
        point
    }

    fn halved(&self, plane: &Plane, at: [f64; 2]) -> Option<Patch> {
        let fine = self.side.0.min(self.side.1) >= COARSE;
        let far = self.reach[0].max(self.reach[1]) > NEAR;
        let half = plane.half();
        let side = (self.side.0 / 2, self.side.1 / 2);
        (fine && far).then(|| Patch::of(&half, half.local(at), side))
    }

    pub fn step(&mut self, p: &Rgba) -> Found {
        let half = [0, 1].map(|i| self.side_of(i) / 2.0 + self.reach[i]);
        let plane = Plane::around(p, self.channel, self.last, half);
        let guess = self.coarse.as_ref().and_then(|t| {
            let small = plane.half();
            let reach = self.reach.map(|r| r / 2.0);
            let (c, _) = search(&small, t, small.local(self.last), reach)?;
            Some(small.global(c))
        });
        let (around, reach) = match guess {
            Some(g) => (g, self.reach.map(|r| r.min(NEAR))),
            None => (self.last, self.reach),
        };
        let mut score = 0.0;
        if let Some((c, s)) = search(&plane, &self.now, plane.local(around), reach) {
            let at = plane.global(c);
            self.last = at;
            score = s;
            self.adapt(&plane, at);
        }
        let (pw, ph) = (self.picture.0 as f64, self.picture.1 as f64);
        Found {
            center: [(self.last[0] / pw) as f32, (self.last[1] / ph) as f32],
            size: self.size,
            rotation: 0.0,
            score: score.clamp(0.0, 1.0) as f32,
        }
    }

    fn side_of(&self, i: usize) -> f64 {
        [self.side.0, self.side.1][i] as f64
    }

    fn adapt(&mut self, plane: &Plane, at: [f64; 2]) {
        if self.adaptive == AdaptiveMode::None {
            return;
        }
        let new = Patch::of(plane, plane.local(at), self.side);
        let change = (1.0 - self.first.like(&new)).clamp(0.0, 1.0);
        if self.adaptive == AdaptiveMode::BestMatch && change > self.tolerance {
            return;
        }
        self.now = new;
        self.coarse = self.halved(plane, at);
    }
}
