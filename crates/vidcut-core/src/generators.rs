use std::ops::RangeInclusive;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::keyframes::{checked_handle, value_at};
use crate::model::{Generator, Gradient, Hdr, Keyframe};

const BT709_TO_BT2020: [[f64; 3]; 3] = [
    [0.62740389, 0.32928304, 0.04331307],
    [0.06909729, 0.91954040, 0.01136232],
    [0.01639144, 0.08801331, 0.89559525],
];

const BT2020_LUMA: [f64; 3] = [0.2627, 0.6780, 0.0593];

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 32.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 128.0;
const PQ_C3: f64 = 2392.0 / 128.0;
const PQ_WHITE: f64 = 0.01;

const HLG_A: f64 = 0.17883277;
const HLG_B: f64 = 0.28466892;
const HLG_C: f64 = 0.55991073;
const HLG_GAMMA: f64 = 1.2;
const HLG_WHITE: f64 = 0.1;

pub const CENTER: RangeInclusive<f64> = -1.0..=1.0;

const RGB48: usize = 6;

fn pq_encode(light: f64) -> f64 {
    let p = light.max(0.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * p) / (1.0 + PQ_C3 * p)).powf(PQ_M2)
}

fn pq_decode(code: f64) -> f64 {
    let p = code.max(0.0).powf(1.0 / PQ_M2);
    ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1)
}

fn hlg_encode(light: f64) -> f64 {
    match light.max(0.0) {
        l if l <= 1.0 / 12.0 => (3.0 * l).sqrt(),
        l => HLG_A * (12.0 * l - HLG_B).ln() + HLG_C,
    }
}

pub fn pq_code(nits: f64) -> f64 {
    pq_encode(nits / 10000.0)
}

pub fn hlg_code(nits: f64) -> f64 {
    hlg_encode((nits / 1000.0).max(0.0).powf(1.0 / HLG_GAMMA))
}

fn hlg_decode(code: f64) -> f64 {
    match code.max(0.0) {
        c if c <= 0.5 => c * c / 3.0,
        c => (((c - HLG_C) / HLG_A).exp() + HLG_B) / 12.0,
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn to_bt2020(c: [f64; 3]) -> [f64; 3] {
    BT709_TO_BT2020.map(|row| dot(row, c))
}

fn hlg_display(scene: [f64; 3]) -> [f64; 3] {
    let gain = dot(BT2020_LUMA, scene).max(0.0).powf(HLG_GAMMA - 1.0);
    scene.map(|v| v * gain)
}

fn hlg_scene(display: [f64; 3]) -> [f64; 3] {
    let luma = dot(BT2020_LUMA, display);
    if luma <= 0.0 {
        return [0.0; 3];
    }
    let gain = luma.powf((1.0 - HLG_GAMMA) / HLG_GAMMA);
    display.map(|v| v * gain)
}

fn fractions(color: [u8; 3]) -> [f64; 3] {
    color.map(|v| f64::from(v) / 255.0)
}

fn in_timeline(c: [f64; 3], colors: Option<Hdr>) -> [f64; 3] {
    match colors {
        None => c,
        Some(Hdr::Pq) => {
            let white = pq_encode(PQ_WHITE);
            to_bt2020(c.map(pq_decode)).map(|v| white * pq_encode(v))
        }
        Some(Hdr::Hlg) => {
            let white = hlg_encode(HLG_WHITE.powf(1.0 / HLG_GAMMA));
            hlg_scene(to_bt2020(hlg_display(c.map(hlg_decode)))).map(|v| white * hlg_encode(v))
        }
    }
}

pub fn timeline_code(color: [u8; 3], colors: Option<Hdr>) -> [f64; 3] {
    in_timeline(fractions(color), colors)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

impl Default for Gradient {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_x_keys: Vec::new(),
            center_y: 0.0,
            center_y_keys: Vec::new(),
            upper_left: [0xff, 0x00, 0x00],
            upper_right: [0xff, 0xff, 0x00],
            lower_left: [0x00, 0x00, 0xff],
            lower_right: [0x00, 0xff, 0xff],
        }
    }
}

pub fn checked_center(value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value.clamp(*CENTER.start(), *CENTER.end()))
    } else {
        Err(Error::invalid(format!("center {value} is not a number")))
    }
}

impl Gradient {
    pub fn is_keyed(&self) -> bool {
        !self.center_x_keys.is_empty() || !self.center_y_keys.is_empty()
    }

    pub fn keys(&self, axis: Axis) -> &[Keyframe] {
        match axis {
            Axis::X => &self.center_x_keys,
            Axis::Y => &self.center_y_keys,
        }
    }

    pub fn keys_mut(&mut self, axis: Axis) -> &mut Vec<Keyframe> {
        match axis {
            Axis::X => &mut self.center_x_keys,
            Axis::Y => &mut self.center_y_keys,
        }
    }

    pub fn plain(&self, axis: Axis) -> f64 {
        match axis {
            Axis::X => self.center_x,
            Axis::Y => self.center_y,
        }
    }

    pub fn plain_mut(&mut self, axis: Axis) -> &mut f64 {
        match axis {
            Axis::X => &mut self.center_x,
            Axis::Y => &mut self.center_y,
        }
    }

    pub fn center_at(&self, axis: Axis, frame: f64) -> f64 {
        value_at(self.keys(axis), frame).map_or(self.plain(axis), |v| {
            v.clamp(*CENTER.start(), *CENTER.end())
        })
    }

    pub fn corners(&self) -> [[u8; 3]; 4] {
        [
            self.upper_left,
            self.upper_right,
            self.lower_left,
            self.lower_right,
        ]
    }

    pub fn problem(&self) -> Option<String> {
        [Axis::X, Axis::Y].into_iter().find_map(|axis| {
            let keys = self.keys(axis);
            if keys.windows(2).any(|w| w[0].frame >= w[1].frame) {
                return Some("center keyframes out of order or on one frame".to_string());
            }
            let plain = self.plain(axis);
            let value = |v: f64| CENTER.contains(&v);
            let shaped = |k: &Keyframe| {
                [(k.in_handle, true), (k.out_handle, false)]
                    .into_iter()
                    .all(|(h, before)| h.is_none_or(|h| checked_handle(h, before).is_ok()))
            };
            (!value(plain) || !keys.iter().all(|k| value(k.value) && shaped(k)))
                .then(|| "a center outside -1..1".to_string())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pattern {
    SolidColor([u8; 3]),
    FourColorGradient {
        center: [u64; 2],
        corners: [[u8; 3]; 4],
    },
}

impl Pattern {
    pub fn of_gradient(g: &Gradient, frame: f64) -> Self {
        Self::FourColorGradient {
            center: [Axis::X, Axis::Y].map(|axis| g.center_at(axis, frame).to_bits()),
            corners: g.corners(),
        }
    }
}

fn bend(t: f64, p: f64) -> f64 {
    if p <= 0.0 {
        0.5 + 0.5 * t
    } else if p >= 1.0 {
        0.5 * t
    } else if t <= p {
        0.5 * t / p
    } else {
        0.5 + 0.5 * (t - p) / (1.0 - p)
    }
}

fn bent_along(length: u32, center: f64) -> Vec<f64> {
    let last = f64::from(length.saturating_sub(1).max(1));
    let p = f64::midpoint(center, 1.0);
    (0..length).map(|i| bend(f64::from(i) / last, p)).collect()
}

fn mixed(corners: [[f64; 3]; 4], u: f64, v: f64) -> [f64; 3] {
    let [ul, ur, ll, lr] = corners;
    [0, 1, 2]
        .map(|c| (1.0 - v) * ((1.0 - u) * ul[c] + u * ur[c]) + v * ((1.0 - u) * ll[c] + u * lr[c]))
}

fn put_row(row: &mut [u8], codes: impl Iterator<Item = [f64; 3]>) {
    for (pixel, code) in row.as_chunks_mut::<RGB48>().0.iter_mut().zip(codes) {
        for (bytes, v) in pixel.as_chunks_mut::<2>().0.iter_mut().zip(code) {
            *bytes = ((v * 65535.0).round() as u16).to_le_bytes();
        }
    }
}

fn same_rows(data: &mut [u8], stride: usize, width: usize, codes: &[[f64; 3]]) {
    let bytes = width * RGB48;
    let (head, rest) = data.split_at_mut(stride.min(data.len()));
    put_row(&mut head[..bytes], codes.iter().copied().cycle());
    for line in rest.chunks_mut(stride) {
        line[..bytes].copy_from_slice(&head[..bytes]);
    }
}

fn each_row(data: &mut [u8], stride: usize, height: usize, work: impl Fn(usize, &mut [u8]) + Sync) {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let block = height.div_ceil(threads).max(1);
    let work = &work;
    std::thread::scope(|s| {
        for (b, lines) in data.chunks_mut(block * stride).enumerate() {
            s.spawn(move || {
                for (i, line) in lines.chunks_mut(stride).enumerate() {
                    work(b * block + i, line);
                }
            });
        }
    });
}

pub fn draw(
    pattern: Pattern,
    (w, h): (u32, u32),
    colors: Option<Hdr>,
    data: &mut [u8],
    stride: usize,
) {
    let width = w as usize;
    match pattern {
        Pattern::SolidColor(color) => {
            same_rows(data, stride, width, &[timeline_code(color, colors)]);
        }
        Pattern::FourColorGradient { center, corners } => {
            let [x, y] = center.map(f64::from_bits);
            let across = bent_along(w, x);
            let down = bent_along(h, y);
            let corners = corners.map(fractions);
            each_row(data, stride, h as usize, |i, line| {
                let v = down[i];
                let codes = across
                    .iter()
                    .map(|&u| in_timeline(mixed(corners, u, v), colors));
                put_row(&mut line[..width * RGB48], codes);
            });
        }
    }
}

pub fn name(g: &Generator) -> &str {
    match g {
        Generator::SolidColor { .. } => Generator::SOLID_COLOR,
        Generator::FourColorGradient(_) => Generator::FOUR_COLOR_GRADIENT,
        Generator::Carried { .. } if g.is_offline() => "Media Offline",
        Generator::Carried { reference } => [&reference["generator_kind"], &reference["name"]]
            .into_iter()
            .filter_map(Value::as_str)
            .find(|n| !n.is_empty())
            .unwrap_or("generator"),
    }
}

pub fn shown(g: &Generator) -> Value {
    if !g.draws() {
        return json!({ "kind": "carried", "name": name(g) });
    }
    let mut v = json!(g);
    v["name"] = json!(name(g));
    v
}
