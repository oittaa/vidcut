use crate::model::{BlendMode, Corrector, Grade, GradeLink, GradeNode, GradeTool, KeyOutput};
use crate::tone_tables::{HIGHLIGHTS, SHADOWS, TONE_AMOUNTS};
use crate::window_key::Keying;

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

const BRADFORD_BACK: [[f64; 3]; 3] = [
    [0.9870, -0.1471, 0.1600],
    [0.4323, 0.5184, 0.0493],
    [-0.0085, 0.0400, 0.9685],
];

const ISOTEMPERATURE: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.30470],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.18740, 0.28668, -0.40955],
    [90.0, 0.18880, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.24010, 0.34308, -2.0637],
    [325.0, 0.24792, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.26400, 0.35200, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.31320, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.770],
    [600.0, 0.33724, 0.36051, -116.45],
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

const WHEEL_LEAST_GAIN: f64 = 0.001;

const WHEEL_MOST_LIFT: f64 = 0.999;

const WHEEL_STEEPEST: f64 = 200.0;

const NEWTON_STEPS: usize = 100;

const NEWTON_CLOSE: f64 = 1e-12;

pub const TONE_SIZE: usize = 1024;

pub const THUMBNAIL_GRAY: f64 = 0.5;

pub const KEY_BLUR_STEP: f64 = 0.00844;

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
    fn of(g: &Corrector) -> Option<Self> {
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

fn white_xyz(kelvin: f64, duv: f64) -> [f64; 3] {
    let mired = 1e6 / kelvin;
    let last = ISOTEMPERATURE.len() - 2;
    let i = ISOTEMPERATURE
        .iter()
        .rposition(|row| row[0] <= mired)
        .unwrap_or(0)
        .min(last);
    let (a, b) = (ISOTEMPERATURE[i], ISOTEMPERATURE[i + 1]);
    let f = (b[0] - mired) / (b[0] - a[0]);
    let between = |p: f64, q: f64| p * f + q * (1.0 - f);
    let unit = |slope: f64| {
        let length = 1.0f64.hypot(slope);
        [1.0 / length, slope / length]
    };
    let (da, db) = (unit(a[3]), unit(b[3]));
    let (nu, nv) = (between(da[0], db[0]), between(da[1], db[1]));
    let length = nu.hypot(nv);
    let u = between(a[1], b[1]) - duv * nu / length;
    let v = between(a[2], b[2]) - duv * nv / length;
    let d = 2.0 * u - 8.0 * v + 4.0;
    let (x, y) = (3.0 * u / d, 2.0 * v / d);
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn temp_matrix(temp: f64, tint: f64) -> [[f64; 3]; 3] {
    if temp == 0.0 && tint == 0.0 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let from = apply(&BRADFORD, white_xyz(WHITE_KELVIN, 0.0));
    let to = apply(&BRADFORD, white_xyz(WHITE_KELVIN - temp, -tint / TINT_DUV));
    let scale: [[f64; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| if i == j { to[i] / from[i] } else { 0.0 })
    });
    let into = product(&BRADFORD, &RGB_TO_XYZ);
    let back = product(&inverse(&RGB_TO_XYZ), &BRADFORD_BACK);
    product(&back, &product(&scale, &into))
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
    let gain = gain.max(WHEEL_LEAST_GAIN);
    let steep = WHEEL_STEEPEST * (gain - 1.0) + gain;
    let least = match steep > 0.0 {
        true => (gain - WHEEL_STEEPEST) / steep,
        false => f64::NEG_INFINITY,
    };
    let lift = (2.0 * lift).max(least).min(WHEEL_MOST_LIFT);
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
        } else if p < 1.0 {
            p + (1.0 - p) * hermite(&self.knots, (x - p) / (1.0 - p))
        } else {
            1.0
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

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Flat(f64),
    Map(usize),
}

pub fn key_join(a: f64, b: f64) -> f64 {
    let h = (0.5 - (a - b).abs()).max(0.0);
    (8.0 * (a.max(b) + 0.5 * h * h) - 1.0) / 7.0
}

pub fn key_blur_spread((blur, ratio): (f64, f64), height: f64) -> [f64; 2] {
    let s = KEY_BLUR_STEP * blur.max(0.0) * height;
    [s * (1.0 + ratio).min(1.0), s * (1.0 - ratio).min(1.0)]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyFold {
    pub gain: f64,
    pub offset: f64,
    pub invert: bool,
    pub matte: bool,
    pub finish: Option<(bool, KeyOutput)>,
}

impl KeyFold {
    pub fn of(&self, acc: f64, key: f64) -> f64 {
        let v = self.gain * key + self.offset;
        let x = if self.invert { 1.0 - v } else { v };
        let v = match self.matte {
            true => 1.0 - key_join(1.0 - acc, 1.0 - x),
            false => key_join(acc, x),
        };
        match self.finish {
            Some((true, out)) => out.of(v.clamp(0.0, 1.0)),
            Some((false, out)) => out.of(v),
            None => v,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum KeySource {
    Windows,
    Link(GradeLink),
}

fn picture_of(l: GradeLink, count: usize) -> usize {
    l.node as usize + if l.key { count } else { 0 }
}

fn key_of(l: GradeLink, count: usize) -> usize {
    l.node as usize + if l.key { 0 } else { count }
}

fn key_steps(node: &GradeNode, off: &[bool]) -> Vec<(KeySource, KeyFold)> {
    let fold = |(gain, offset, invert), matte| KeyFold {
        gain,
        offset,
        invert,
        matte,
        finish: None,
    };
    let (mut steps, first) = match &node.tool {
        GradeTool::Corrector(_) => {
            let windows = node.windows.iter().any(|w| w.on);
            let mut steps = Vec::new();
            if windows {
                steps.push((KeySource::Windows, fold((1.0, 0.0, false), false)));
            }
            if let Some(from) = node.key_from {
                let i = node.key_input;
                let how = fold((i.gain, i.offset, i.invert), windows && !i.mask);
                steps.push((KeySource::Link(from), how));
            }
            (steps, true)
        }
        GradeTool::KeyMixer { mix } => {
            let linked = (node.inputs.iter().zip(mix))
                .filter_map(|(v, m)| v.filter(|v| !is_off(off, *v)).map(|v| (v, *m)));
            let (mattes, masks): (Vec<_>, Vec<_>) = linked.partition(|(_, m)| m.matte);
            let steps = (masks.into_iter().chain(mattes))
                .map(|(v, m)| {
                    (
                        KeySource::Link(v),
                        fold((m.gain, m.offset, m.invert), m.matte),
                    )
                })
                .collect();
            (steps, false)
        }
        _ => (Vec::new(), false),
    };
    if let Some(last) = steps.last_mut() {
        last.1.finish = Some((first, node.key));
    }
    steps
}

fn switched_off(g: &Grade) -> Vec<bool> {
    let off = |d: &GradeNode| !d.on && matches!(d.tool, GradeTool::Corrector(_));
    std::iter::once(false)
        .chain(g.nodes.iter().map(off))
        .collect()
}

fn is_off(off: &[bool], l: GradeLink) -> bool {
    l.key && off.get(l.node as usize).copied().unwrap_or(false)
}

fn cut_off(node: &GradeNode, off: &[bool]) -> bool {
    let corrector = matches!(node.tool, GradeTool::Corrector(_));
    corrector && node.key_from.is_some_and(|v| is_off(off, v))
}

fn key_sources<'a>(node: &'a GradeNode, off: &'a [bool]) -> impl Iterator<Item = GradeLink> + 'a {
    let mixed = node.inputs.iter().flatten().filter(|_| node.takes_keys());
    let mixed = mixed.copied().filter(|v| !is_off(off, *v));
    mixed.chain(node.key_from)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    Flat(f64),
    Map,
}

fn shape_of(node: &GradeNode, shapes: &[Shape], off: &[bool], count: usize) -> Shape {
    let steps = key_steps(node, off);
    if steps.is_empty() {
        return Shape::Flat(node.key.of(1.0));
    }
    let mut acc = 0.0;
    for (source, fold) in steps {
        match source {
            KeySource::Link(l) => match shapes[key_of(l, count)] {
                Shape::Flat(x) => acc = fold.of(acc, x),
                Shape::Map => return Shape::Map,
            },
            KeySource::Windows => return Shape::Map,
        }
    }
    Shape::Flat(acc)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pass {
    Copy {
        from: usize,
        to: usize,
    },
    Key {
        to: usize,
        keying: Box<Keying>,
    },
    KeyFold {
        to: usize,
        acc: Key,
        input: Key,
        fold: KeyFold,
    },
    KeyBlur {
        on: usize,
        blur: (f64, f64),
    },
    KeySharpen {
        on: usize,
        from: usize,
    },
    Luma {
        from: usize,
        to: usize,
    },
    Gray {
        key: Key,
        alpha: usize,
        to: usize,
    },
    Correct {
        on: usize,
        grading: Box<Grading>,
        key: Key,
    },
    Parallel {
        on: usize,
        change: usize,
        base: usize,
    },
    Layer {
        on: usize,
        top: usize,
        mode: BlendMode,
        key: Key,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tap {
    pub node: u32,
    pub after: usize,
    pub from: Option<usize>,
    pub key: Key,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Graded {
    pub passes: Vec<Pass>,
    pub registers: usize,
    pub taps: Vec<Tap>,
}

pub fn order(g: &Grade) -> Option<Vec<u32>> {
    fn visit(g: &Grade, k: u32, state: &mut [u8], out: &mut Vec<u32>) -> Option<()> {
        match state[k as usize] {
            2 => return Some(()),
            1 => return None,
            _ => {}
        }
        state[k as usize] = 1;
        for v in g.node(k)?.links().map(|l| l.node) {
            if v > g.last() {
                return None;
            }
            if v > 0 {
                visit(g, v, state, out)?;
            }
        }
        state[k as usize] = 2;
        out.push(k);
        Some(())
    }
    let mut state = vec![0u8; g.nodes.len() + 1];
    let mut out = Vec::new();
    for k in 1..=g.last() {
        visit(g, k, &mut state, &mut out)?;
    }
    Some(out)
}

pub fn complete(g: &Grade) -> bool {
    let used = |k: u32| {
        let from = |l: GradeLink| l.node == k;
        g.output.is_some_and(from) || (g.nodes.iter()).any(|n| n.links().any(from))
    };
    g.output.is_some()
        && g.nodes.iter().zip(1..).all(|(n, k)| {
            let fed = match n.tool {
                GradeTool::Corrector(_) => n.inputs.iter().all(Option::is_some),
                GradeTool::KeyMixer { .. } => true,
                _ => n.inputs.iter().any(Option::is_some),
            };
            fed && used(k)
        })
}

fn shared(lists: impl IntoIterator<Item = Vec<usize>>) -> Option<Vec<usize>> {
    lists
        .into_iter()
        .reduce(|a, b| a.into_iter().filter(|v| b.contains(v)).collect())
}

fn paths_in(through: &[Vec<usize>], g: &Grade, k: u32) -> Option<Vec<usize>> {
    let count = g.nodes.len() + 1;
    let ins = g.node(k)?.inputs.iter().flatten();
    shared(ins.map(|l| through[picture_of(*l, count)].clone()))
}

fn passed(g: &Grade) -> Option<Vec<Vec<usize>>> {
    let count = g.nodes.len() + 1;
    let mut through: Vec<Vec<usize>> = (0..2 * count).map(|i| vec![0, i]).collect();
    through[0] = vec![0];
    for k in order(g)? {
        if g.node(k)?.takes_keys() {
            continue;
        }
        let mut path = paths_in(&through, g, k)?;
        path.push(k as usize);
        through[k as usize] = path;
    }
    Some(through)
}

fn base(through: &[Vec<usize>], g: &Grade, k: u32) -> Option<usize> {
    paths_in(through, g, k)?.pop()
}

struct Registers {
    holder: Vec<usize>,
    left: Vec<usize>,
    free: Vec<usize>,
}

impl Registers {
    fn done(&mut self, v: usize) {
        let r = self.holder[v];
        self.left[r] -= 1;
        if self.left[r] == 0 {
            self.free.push(r);
        }
    }

    fn take(&mut self, v: usize, passes: &mut Vec<Pass>) -> usize {
        let r = self.holder[v];
        if self.left[r] == 1 {
            self.left[r] = 0;
            return r;
        }
        self.left[r] -= 1;
        let to = self.free.pop().unwrap_or_else(|| {
            self.left.push(0);
            self.left.len() - 1
        });
        passes.push(Pass::Copy { from: r, to });
        to
    }

    fn hold(&mut self, k: usize, r: usize, uses: usize) {
        self.holder[k] = r;
        self.left[r] = uses;
    }

    fn alias(&mut self, k: usize, v: usize, uses: usize) {
        let r = self.holder[v];
        self.holder[k] = r;
        self.left[r] += uses;
        self.done(v);
    }

    fn kept(&mut self, uses: usize) -> usize {
        let r = self.free.pop().unwrap_or_else(|| {
            self.left.push(0);
            self.left.len() - 1
        });
        self.left[r] = uses;
        r
    }

    fn used(&mut self, key: &Key) {
        if let Key::Map(r) = *key {
            self.left[r] -= 1;
            if self.left[r] == 0 {
                self.free.push(r);
            }
        }
    }
}

impl Grade {
    pub fn is_active(&self) -> bool {
        self.is_keyed() || Graded::of(self).is_some()
    }
}

struct Uses {
    count: usize,
    all: Vec<usize>,
    over: Vec<usize>,
    whole: Vec<usize>,
    keys: Vec<Key>,
    shapes: Vec<Shape>,
    counts: Vec<usize>,
    off: Vec<bool>,
    tapped: bool,
}

impl Uses {
    fn kept(&self, k: usize) -> usize {
        self.all[k] - self.over[k]
    }

    fn partial(&self, k: usize) -> bool {
        match self.shapes[k] {
            Shape::Map => true,
            Shape::Flat(v) => v > 0.0 && v < 1.0,
        }
    }

    fn twice(&self, node: &GradeNode, c: &Corrector, k: usize) -> bool {
        let seen = self.kept(k) > 0 || self.tapped;
        node.on && !c.is_usual() && self.over[k] > 0 && seen && self.partial(k)
    }

    fn own(&self, node: &GradeNode, k: usize) -> usize {
        match &node.tool {
            GradeTool::Corrector(c) if node.on => {
                let own = !c.is_usual() && (self.twice(node, c, k) || self.over[k] == 0);
                usize::from(own) + self.over[k] + usize::from(self.tapped)
            }
            _ => 0,
        }
    }

    fn source(
        &self,
        (node, l): (&GradeNode, GradeLink),
        r: &mut Registers,
        passes: &mut Vec<Pass>,
    ) -> Key {
        let key = self.keys[key_of(l, self.count)].clone();
        let blur = (node.key_input.blur, node.key_input.blur_ratio);
        match key {
            Key::Map(from) if !node.takes_keys() && blur.0 != 0.0 => {
                let to = r.kept(1);
                passes.push(Pass::Copy { from, to });
                let spread = (blur.0.abs(), blur.1);
                passes.push(Pass::KeyBlur {
                    on: to,
                    blur: spread,
                });
                if blur.0 < 0.0 {
                    passes.push(Pass::KeySharpen { on: to, from });
                }
                r.used(&key);
                Key::Map(to)
            }
            _ => key,
        }
    }

    fn folded(
        &self,
        (node, count): (&GradeNode, usize),
        r: &mut Registers,
        passes: &mut Vec<Pass>,
    ) -> Option<Key> {
        let steps = key_steps(node, &self.off);
        if let [(KeySource::Windows, _)] = steps[..] {
            let to = r.kept(count);
            let keying = Box::new(Keying::of(node)?);
            passes.push(Pass::Key { to, keying });
            return Some(Key::Map(to));
        }
        let mut acc = Key::Flat(0.0);
        for (i, (source, fold)) in steps.iter().enumerate() {
            let input = match *source {
                KeySource::Windows => {
                    let to = r.kept(1);
                    let keying = Keying {
                        output: KeyOutput::default(),
                        ..Keying::of(node)?
                    };
                    let keying = Box::new(keying);
                    passes.push(Pass::Key { to, keying });
                    Key::Map(to)
                }
                KeySource::Link(l) => self.source((node, l), r, passes),
            };
            acc = match (&acc, &input) {
                (Key::Flat(a), Key::Flat(x)) => Key::Flat(fold.of(*a, *x)),
                _ => {
                    let uses = if i + 1 == steps.len() { count } else { 1 };
                    let to = r.kept(uses);
                    passes.push(Pass::KeyFold {
                        to,
                        acc: acc.clone(),
                        input: input.clone(),
                        fold: *fold,
                    });
                    r.used(&acc);
                    r.used(&input);
                    Key::Map(to)
                }
            };
        }
        Some(acc)
    }

    fn keyed(
        &self,
        (node, k): (&GradeNode, usize),
        r: &mut Registers,
        passes: &mut Vec<Pass>,
    ) -> Option<Key> {
        let count = self.counts[k];
        match self.shapes[k] {
            Shape::Flat(v) => Some(Key::Flat(v)),
            Shape::Map if count == 0 => Some(Key::Flat(1.0)),
            Shape::Map => self.folded((node, count), r, passes),
        }
    }
}

fn corrected(
    (node, c, k, x): (&GradeNode, &Corrector, usize, usize),
    uses: &mut Uses,
    r: &mut Registers,
    passes: &mut Vec<Pass>,
) -> Option<usize> {
    let grading = || Box::new(Grading::of(c));
    if !node.on {
        r.alias(k, x, uses.kept(k));
        return Some(r.holder[k]);
    }
    let key = uses.keys[k].clone();
    let none = key == Key::Flat(0.0);
    if c.is_usual() || none {
        r.alias(k, x, uses.all[k]);
        (!none).then_some(r.holder[k])
    } else if uses.twice(node, c, k) {
        let mix = r.take(x, passes);
        let on = r.take(x, passes);
        passes.push(Pass::Correct {
            on,
            grading: grading(),
            key: Key::Flat(1.0),
        });
        passes.push(Pass::Layer {
            on: mix,
            top: on,
            mode: BlendMode::Normal,
            key: key.clone(),
        });
        r.used(&key);
        r.hold(k, mix, uses.kept(k));
        if uses.kept(k) == 0 {
            r.free.push(mix);
        }
        let whole = uses.all.len() + k;
        r.hold(whole, on, uses.over[k]);
        uses.whole[k] = whole;
        Some(mix)
    } else if uses.all[k] == 0 && !uses.tapped {
        r.done(x);
        r.used(&key);
        None
    } else {
        let on = r.take(x, passes);
        let key = match uses.over[k] {
            0 => key,
            _ => Key::Flat(1.0),
        };
        passes.push(Pass::Correct {
            on,
            grading: grading(),
            key: key.clone(),
        });
        r.used(&key);
        r.hold(k, on, uses.all[k]);
        Some(on)
    }
}

fn layered(
    (g, ins, mode): (&Grade, &[usize], BlendMode),
    uses: &Uses,
    r: &mut Registers,
    passes: &mut Vec<Pass>,
) -> usize {
    let on = r.take(ins[0], passes);
    for &v in &ins[1..] {
        let (key, off) = match g.node(v as u32) {
            Some(d) if !d.tool.is_mixer() => (uses.keys[v].clone(), !d.on),
            _ => (Key::Flat(1.0), false),
        };
        if off {
            continue;
        }
        let slot = uses.whole[v];
        if key != Key::Flat(0.0) {
            let top = r.holder[slot];
            let layer = Pass::Layer {
                on,
                top,
                mode,
                key: key.clone(),
            };
            passes.push(layer);
            r.used(&key);
        }
        r.done(slot);
    }
    on
}

fn converted(k: usize, uses: &mut Uses, r: &mut Registers, passes: &mut Vec<Pass>) {
    let at = uses.count + k;
    if uses.counts[at] > 0 {
        let to = r.kept(uses.counts[at]);
        passes.push(Pass::Luma {
            from: r.holder[k],
            to,
        });
        r.done(k);
        uses.keys[at] = Key::Map(to);
    }
    if uses.all[at] > 0 {
        let key = uses.keys[k].clone();
        let to = r.kept(uses.all[at]);
        passes.push(Pass::Gray {
            key: key.clone(),
            alpha: r.holder[0],
            to,
        });
        r.used(&key);
        r.done(0);
        r.holder[at] = to;
    }
}

fn keys_only(p: &Pass) -> bool {
    matches!(
        p,
        Pass::Copy { .. }
            | Pass::Key { .. }
            | Pass::KeyFold { .. }
            | Pass::KeyBlur { .. }
            | Pass::KeySharpen { .. }
            | Pass::Luma { .. }
    )
}

impl Graded {
    pub fn of(g: &Grade) -> Option<Self> {
        let made = Self::made(g, false)?;
        (!made.passes.iter().all(keys_only)).then_some(made)
    }

    pub fn tapped(g: &Grade) -> Option<Self> {
        Self::made(g, true)
    }

    fn uses(g: &Grade, (order, through): (&[u32], &[Vec<usize>]), tapped: bool) -> Option<Uses> {
        let count = g.nodes.len() + 1;
        let off = switched_off(g);
        let mut shapes = vec![Shape::Flat(1.0); count];
        shapes.resize(2 * count, Shape::Map);
        for &k in order {
            let node = g.node(k)?;
            shapes[k as usize] = match cut_off(node, &off) {
                true => Shape::Flat(0.0),
                false => shape_of(node, &shapes, &off, count),
            };
        }
        let mut uses = Uses {
            count,
            all: vec![0; 2 * count],
            over: vec![0; 2 * count],
            whole: (0..2 * count).collect(),
            keys: vec![Key::Flat(1.0); 2 * count],
            shapes,
            counts: vec![0; 2 * count],
            off,
            tapped,
        };
        for &k in order {
            let node = g.node(k)?;
            match &node.tool {
                GradeTool::ParallelMixer => {
                    let x = base(through, g, k)?;
                    uses.all[x] += 1;
                }
                GradeTool::LayerMixer { .. } => {
                    for l in node.inputs.iter().flatten().skip(1) {
                        uses.over[picture_of(*l, count)] += 1;
                    }
                }
                GradeTool::Corrector(_) | GradeTool::KeyMixer { .. } => {}
            }
        }
        let pictures = g.nodes.iter().filter(|n| !n.takes_keys());
        for l in pictures.flat_map(|n| n.inputs.iter().flatten()) {
            uses.all[picture_of(*l, count)] += 1;
        }
        uses.all[picture_of(g.output?, count)] += 1;
        for &k in order.iter().rev() {
            let node = g.node(k)?;
            let k = k as usize;
            if uses.counts[count + k] > 0 {
                uses.all[k] += 1;
            }
            if uses.all[count + k] > 0 {
                uses.counts[k] += 1;
                uses.all[0] += 1;
            }
            if let (GradeTool::Corrector(c), Some(Some(x))) = (&node.tool, node.inputs.first())
                && uses.twice(node, c, k)
            {
                uses.all[picture_of(*x, count)] += 1;
            }
            uses.counts[k] += uses.own(node, k);
            if uses.counts[k] > 0 && !cut_off(node, &uses.off) {
                for l in key_sources(node, &uses.off) {
                    uses.counts[key_of(l, count)] += 1;
                }
            }
        }
        if uses.counts[count] > 0 {
            uses.all[0] += 1;
        }
        Some(uses)
    }

    fn made(g: &Grade, tapped: bool) -> Option<Self> {
        if !complete(g) {
            return None;
        }
        let order = order(g)?;
        let count = g.nodes.len() + 1;
        let out = picture_of(g.output?, count);
        let through = passed(g)?;
        let mut uses = Self::uses(g, (&order, &through), tapped)?;
        let mut r = Registers {
            holder: vec![0; 2 * uses.all.len()],
            left: vec![uses.all[0]],
            free: Vec::new(),
        };
        let mut passes = Vec::new();
        let mut taps = Vec::new();
        converted(0, &mut uses, &mut r, &mut passes);
        for &k in &order {
            let node = g.node(k)?;
            let ins: Vec<usize> = (node.inputs.iter().flatten())
                .map(|l| picture_of(*l, count))
                .collect();
            let number = k;
            let k = k as usize;
            uses.keys[k] = uses.keyed((node, k), &mut r, &mut passes)?;
            match &node.tool {
                GradeTool::Corrector(c) => {
                    let from = corrected((node, c, k, ins[0]), &mut uses, &mut r, &mut passes);
                    if tapped {
                        let key = match node.on {
                            true => uses.keys[k].clone(),
                            false => Key::Flat(1.0),
                        };
                        r.used(&key);
                        taps.push(Tap {
                            node: number,
                            after: passes.len(),
                            from,
                            key,
                        });
                    }
                }
                GradeTool::ParallelMixer => {
                    let x = base(&through, g, number)?;
                    let on = r.take(ins[0], &mut passes);
                    for &v in &ins[1..] {
                        let (change, base) = (r.holder[v], r.holder[x]);
                        if change != base {
                            passes.push(Pass::Parallel { on, change, base });
                        }
                        r.done(v);
                    }
                    r.done(x);
                    r.hold(k, on, uses.all[k]);
                }
                GradeTool::LayerMixer { mode } => {
                    let mode = if node.on { *mode } else { BlendMode::Normal };
                    let on = layered((g, &ins, mode), &uses, &mut r, &mut passes);
                    r.hold(k, on, uses.all[k]);
                }
                GradeTool::KeyMixer { .. } => {}
            }
            converted(k, &mut uses, &mut r, &mut passes);
        }
        if r.holder[out] != 0 {
            passes.push(Pass::Copy {
                from: r.holder[out],
                to: 0,
            });
        }
        Some(Self {
            passes,
            registers: r.left.len(),
            taps,
        })
    }
}

impl Grading {
    pub fn of(g: &Corrector) -> Self {
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
