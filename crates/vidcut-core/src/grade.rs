use crate::model::Grade;
use crate::tone_tables::{HIGHLIGHTS, SHADOWS, TONE_AMOUNTS};

pub const OFFSET_STEP: f64 = 187.5 / 1023.0;

pub const HALF_KNOTS: usize = 4;

pub const LOG_KNOTS: usize = 24;

const SATURATION_LUMA: [f64; 3] = [0.21, 0.70, 0.09];

const RGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.4123908, 0.3575843, 0.1804808],
    [0.2126390, 0.7151687, 0.0721923],
    [0.0193308, 0.1191948, 0.9505322],
];

const BRADFORD: [[f64; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

const PLANCK_U: [f64; 9] = [
    0.17974010196483373,
    0.07215506820121004,
    -0.008043884176963433,
    5.55473618568263,
    -27.032152908369465,
    68.71567204412175,
    -103.81110787405582,
    87.91043671155502,
    -32.18225368793109,
];

const PLANCK_V: [f64; 9] = [
    0.26480436774103844,
    0.16960053906710726,
    2.06154621742016,
    -11.486467199932205,
    26.337090785049178,
    -25.963728337451204,
    -6.661635292181216,
    37.24195997977927,
    -23.47137754153343,
];

const WHITE_KELVIN: f64 = 6500.0;

const TINT_DUV: f64 = 10000.0;

const HUE_SHEAR: f64 = 0.006131;

const HUE_SCALE: f64 = 1.022535;

const BOOST_UP: f64 = 0.0040204;

const BOOST_DOWN: f64 = 0.0057435;

const LOG_STEPS: f64 = 1023.0;

const LOG_GRID: i64 = 50;

const LOG_EDGE: f64 = 0.045;

const CONTRAST_ENDS: [f64; 2] = [126.5 / 1023.0, 894.5 / 1023.0];

const CONTRAST_REACH: f64 = 0.125;

const NEWTON_STEPS: usize = 100;

const NEWTON_CLOSE: f64 = 1e-12;

pub const TONE_SIZE: usize = 1024;

pub const TONE_EPS: f64 = 0.01;

pub const TONE_TOP: f64 = 2.0;

const TONE_LOW: f64 = 0.25;

const TONE_HIGH: f64 = 1.75;

const TONE_SCALE_WIDTH: u32 = 320;

const TONE_MOST_SCALE: u32 = 4;

const TONE_RADIUS_STEP: u32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct Tone {
    pub table: Vec<f64>,
    pub below: f64,
    pub above: f64,
    pub detail: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToneFilter {
    pub copy: (u32, u32),
    pub radius: u32,
}

pub fn tone_filter((w, h): (u32, u32)) -> ToneFilter {
    let scale = (w / TONE_SCALE_WIDTH).clamp(1, TONE_MOST_SCALE);
    ToneFilter {
        copy: ((w / scale).max(1), (h / scale).max(1)),
        radius: w.max(h).div_ceil(TONE_RADIUS_STEP) / scale,
    }
}

fn tone_change(table: &[[f64; TONE_SIZE]; TONE_AMOUNTS.len()], amount: f64) -> Vec<f64> {
    let zero = [0.0; TONE_SIZE];
    let mut points: Vec<(f64, &[f64; TONE_SIZE])> =
        TONE_AMOUNTS.iter().copied().zip(table).collect();
    points.push((0.0, &zero));
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    let i = points
        .windows(2)
        .position(|w| amount <= w[1].0)
        .unwrap_or(points.len() - 2);
    let first = i.saturating_sub(1).min(points.len() - 4);
    let near = &points[first..first + 4];
    let mut out = vec![0.0; TONE_SIZE];
    for (j, (a, row)) in near.iter().enumerate() {
        let weight: f64 = near
            .iter()
            .enumerate()
            .filter(|(k, _)| *k != j)
            .map(|(_, (b, _))| (amount - b) / (a - b))
            .product();
        for (o, v) in out.iter_mut().zip(row.iter()) {
            *o += weight * v;
        }
    }
    out
}

impl Tone {
    fn of(g: &Grade) -> Option<Self> {
        if g.shadows == 0.0 && g.highlights == 0.0 && g.midtone_detail == 0.0 {
            return None;
        }
        let shadows = tone_change(&SHADOWS, g.shadows);
        let highlights = tone_change(&HIGHLIGHTS, g.highlights);
        let step = TONE_TOP / (TONE_SIZE - 1) as f64;
        let table: Vec<f64> = (0..TONE_SIZE)
            .map(|i| i as f64 * step + shadows[i] + highlights[i])
            .collect();
        let at = |x: f64| {
            let u = x / step;
            let i = (u.floor() as usize).min(TONE_SIZE - 2);
            table[i] + (u - i as f64) * (table[i + 1] - table[i])
        };
        Some(Self {
            below: at(TONE_LOW) / TONE_LOW,
            above: (table[TONE_SIZE - 1] - at(TONE_HIGH)) / (TONE_TOP - TONE_HIGH),
            detail: g.midtone_detail / 100.0,
            table,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContrastCurve {
    pub pivot: f64,
    pub slope: f64,
    pub knots: Vec<[f64; 3]>,
    pub ends: [f64; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogCurve {
    pub low: f64,
    pub high: f64,
    pub shadow: f64,
    pub highlight: f64,
    pub knots: Vec<[f64; 3]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Grading {
    pub tone: Option<Tone>,
    pub boost: f64,
    pub temp: [[f64; 3]; 3],
    pub lift: [f64; 4],
    pub weight: [f64; 4],
    pub power: [f64; 4],
    pub offset: [f64; 3],
    pub lum_mix: f64,
    pub contrast: ContrastCurve,
    pub logs: [Option<LogCurve>; 3],
    pub color: [[f64; 3]; 3],
}

fn product(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| (0..3).map(|k| m[i][k] * v[k]).sum())
}

fn inverse(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let c = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let det: f64 = (0..3).map(|j| m[0][j] * c(0, j)).sum();
    std::array::from_fn(|i| std::array::from_fn(|j| c(j, i) / det))
}

fn polynomial(c: &[f64], x: f64) -> f64 {
    c.iter().rev().fold(0.0, |acc, k| acc * x + k)
}

fn planck_uv(kelvin: f64) -> [f64; 2] {
    let m = 1000.0 / kelvin;
    [polynomial(&PLANCK_U, m), polynomial(&PLANCK_V, m)]
}

fn white_xyz(kelvin: f64, duv: f64) -> [f64; 3] {
    let [u, v] = planck_uv(kelvin);
    let step = kelvin * 1e-4;
    let [u0, v0] = planck_uv(kelvin - step);
    let [u1, v1] = planck_uv(kelvin + step);
    let (du, dv) = (u1 - u0, v1 - v0);
    let length = du.hypot(dv);
    let (mut nu, mut nv) = (-dv / length, du / length);
    if nv < 0.0 {
        nu = -nu;
        nv = -nv;
    }
    let (u, v) = (u + duv * nu, v + duv * nv);
    let d = 2.0 * u - 8.0 * v + 4.0;
    let (x, y) = (3.0 * u / d, 2.0 * v / d);
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn temp_matrix(temp: f64, tint: f64) -> [[f64; 3]; 3] {
    let cones = product(&BRADFORD, &RGB_TO_XYZ);
    let from = apply(&BRADFORD, white_xyz(WHITE_KELVIN, 0.0));
    let to = apply(&BRADFORD, white_xyz(WHITE_KELVIN - temp, -tint / TINT_DUV));
    let scale: [[f64; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| if i == j { to[i] / from[i] } else { 0.0 })
    });
    product(&inverse(&cones), &product(&scale, &cones))
}

fn color_matrix(saturation: f64, hue: f64) -> [[f64; 3]; 3] {
    let [wr, wg, wb] = SATURATION_LUMA;
    let turned = |v: [f64; 3]| {
        let s = wr * v[0] + wg * v[1] + wb * v[2];
        let (a, b) = (v[0] - s, v[2] - s);
        let (u, w) = (a + HUE_SHEAR * b, HUE_SCALE * b);
        let (sin, cos) = (hue * std::f64::consts::PI).sin_cos();
        let (u, w) = (u * cos - w * sin, u * sin + w * cos);
        let b = w / HUE_SCALE;
        let a = u - HUE_SHEAR * b;
        let (r, bl) = (s + a, s + b);
        let g = (s - wr * r - wb * bl) / wg;
        let s2 = wr * r + wg * g + wb * bl;
        [
            s2 + saturation * (r - s2),
            s2 + saturation * (g - s2),
            s2 + saturation * (bl - s2),
        ]
    };
    let columns = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(turned);
    std::array::from_fn(|i| std::array::from_fn(|j| columns[j][i]))
}

fn wheel(lift: f64, gamma: f64, gain: f64) -> (f64, f64, f64) {
    let lift = 2.0 * lift;
    let gamma = 4.0 * gamma;
    let weight = gain / (1.0 + lift * (gain - 1.0));
    let power = if gamma >= 0.0 {
        1.0 / (1.0 + gamma)
    } else {
        1.0 - gamma
    };
    (lift, weight, power)
}

fn hermite(knots: &[[f64; 3]], x: f64) -> f64 {
    let i = knots
        .windows(2)
        .position(|w| x <= w[1][0])
        .unwrap_or(knots.len() - 2);
    let ([x0, y0, m0], [x1, y1, m1]) = (knots[i], knots[i + 1]);
    let h = x1 - x0;
    let t = (x - x0) / h;
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * y0
        + (t3 - 2.0 * t2 + t) * h * m0
        + (-2.0 * t3 + 3.0 * t2) * y1
        + (t3 - t2) * h * m1
}

fn half_knots(c: f64) -> Vec<[f64; 3]> {
    if c >= 2.0 {
        return vec![[0.0, 0.75, 4.0], [0.25, 1.0, 0.25], [1.0, 1.0, 0.0]];
    }
    let g1 = 3.0 * c / 8.0;
    let t1 = g1 * (2.0 - c);
    let points = [[0.0, 0.0], [t1, g1], [t1 + 0.25, g1 + 0.25], [1.0, 1.0]];
    let slope = |a: [f64; 2], b: [f64; 2]| (b[1] - a[1]) / (b[0] - a[0]);
    let m = [
        1.0 / (2.0 - c),
        slope(points[0], points[2]),
        slope(points[1], points[3]),
        slope(points[2], points[3]),
    ];
    (0..HALF_KNOTS)
        .map(|i| [points[i][0], points[i][1], m[i]])
        .collect()
}

impl ContrastCurve {
    fn of(c: f64, pivot: f64) -> Self {
        let mut out = Self {
            pivot,
            slope: c,
            knots: Vec::new(),
            ends: [c, c],
        };
        if c > 1.0 {
            out.knots = half_knots(c);
            let [a, b] = CONTRAST_ENDS;
            out.ends = [
                (out.inside(a) - out.inside(0.0)) / CONTRAST_REACH,
                (out.inside(1.0) - out.inside(b)) / CONTRAST_REACH,
            ];
        }
        out
    }

    fn inside(&self, x: f64) -> f64 {
        let p = self.pivot;
        let s = if x < p {
            p - p * hermite(&self.knots, (p - x) / p)
        } else {
            p + (1.0 - p) * hermite(&self.knots, (x - p) / (1.0 - p))
        };
        s.clamp(0.0, 1.0)
    }
}

fn bezier(u: f64, m: f64) -> f64 {
    let (x1, y1, x2, y2) = if m >= 0.0 {
        (0.0, m, 1.0 - m, 1.0)
    } else {
        (-m, 0.0, 1.0, 1.0 + m)
    };
    let along = |t: f64, a: f64, b: f64| {
        3.0 * a * t * (1.0 - t) * (1.0 - t) + 3.0 * b * t * t * (1.0 - t) + t * t * t
    };
    let mut t = u;
    for _ in 0..NEWTON_STEPS {
        let x = along(t, x1, x2);
        if (x - u).abs() < NEWTON_CLOSE {
            break;
        }
        let dx =
            3.0 * x1 * (1.0 - t) * (1.0 - 3.0 * t) + 3.0 * x2 * t * (2.0 - 3.0 * t) + 3.0 * t * t;
        t = (t - (x - u) / dx).clamp(0.0, 1.0);
    }
    along(t, y1, y2)
}

fn log_base(z: f64, [s, m, h]: [f64; 3], low: f64, high: f64) -> f64 {
    if z < low {
        return z + if s >= 0.0 {
            s * (low - z)
        } else {
            s / low * (low - z)
        };
    }
    if z > high {
        return z + if h >= 0.0 {
            h * (z - high) / (1.0 - high)
        } else {
            h * (z - high)
        };
    }
    low + (high - low) * bezier((z - low) / (high - low), m)
}

fn log_points(low: f64, high: f64) -> Vec<f64> {
    let index = |v: f64| (v * LOG_STEPS - LOG_EDGE).ceil() as i64;
    let (li, hi) = (index(low), index(high));
    let far = |k: i64, from: i64| (k - from).abs() >= LOG_GRID;
    let mut ks: Vec<i64> = (0..=1000)
        .step_by(LOG_GRID as usize)
        .filter(|&k| far(k, li) && far(k, hi))
        .collect();
    ks.push(hi + LOG_GRID);
    ks.push(LOG_STEPS as i64);
    if (li + LOG_GRID - hi).abs() > LOG_GRID {
        ks.push(li + LOG_GRID);
    }
    ks.sort_unstable();
    ks.dedup();
    ks.into_iter()
        .filter(|&k| (0..=LOG_STEPS as i64).contains(&k))
        .map(|k| k as f64 / LOG_STEPS)
        .collect()
}

fn tangents(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let d: Vec<f64> = xs
        .windows(2)
        .zip(ys.windows(2))
        .map(|(x, y)| (y[1] - y[0]) / (x[1] - x[0]))
        .collect();
    let mut m = vec![0.0; xs.len()];
    for i in 1..xs.len() - 1 {
        let (a, b) = (d[i - 1], d[i]);
        if a * b > 0.0 {
            let (lo, hi) = (a.min(b), a.max(b));
            m[i] = 3.0 * lo * hi / (2.0 * lo + hi);
        }
    }
    m[0] = d[0];
    m[xs.len() - 1] = d[d.len() - 1];
    m
}

impl LogCurve {
    fn of(values: [f64; 3], low: f64, high: f64) -> Option<Self> {
        if values == [0.0; 3] {
            return None;
        }
        let xs = log_points(low, high);
        let ys: Vec<f64> = xs.iter().map(|&z| log_base(z, values, low, high)).collect();
        let ms = tangents(&xs, &ys);
        Some(Self {
            low,
            high,
            shadow: values[0],
            highlight: values[2],
            knots: (0..xs.len()).map(|i| [xs[i], ys[i], ms[i]]).collect(),
        })
    }
}

impl Grading {
    pub fn of(g: &Grade) -> Self {
        let mut lift = [0.0; 4];
        let mut weight = [0.0; 4];
        let mut power = [0.0; 4];
        for i in 0..4 {
            (lift[i], weight[i], power[i]) = wheel(g.lift[i], g.gamma[i], g.gain[i]);
        }
        let boost = if g.color_boost >= 0.0 {
            BOOST_UP * g.color_boost
        } else {
            BOOST_DOWN * g.color_boost
        };
        let channel = |i: usize| [g.log_shadow[i], g.log_midtone[i], g.log_highlight[i]];
        Self {
            tone: Tone::of(g),
            boost,
            temp: temp_matrix(g.temp, g.tint),
            lift,
            weight,
            power,
            offset: g.offset.map(|o| (o - 25.0) / 25.0 * OFFSET_STEP),
            lum_mix: g.lum_mix / 100.0,
            contrast: ContrastCurve::of(g.contrast, g.pivot),
            logs: std::array::from_fn(|i| LogCurve::of(channel(i), g.low_range, g.high_range)),
            color: color_matrix(g.saturation / 50.0, (g.hue - 50.0) / 50.0),
        }
    }
}
