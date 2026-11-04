use std::ops::RangeInclusive;

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::keyframes::{checked_handle, value_at};
use crate::model::{Chart, EbuLevel, Generator, Gradient, Hdr, Keyframe};

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

const BAR_COLORS: [Code; 7] = [
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 1.0],
    [0.0, 1.0, 0.0],
    [1.0, 0.0, 1.0],
    [1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0],
];

const YCBCR_SLOPES: [f64; 3] = [2.539648, 0.359182, 2.814179];

type Code = [f64; 3];
type Band = (u32, Vec<Code>);

struct Bt2111 {
    range: (f64, f64),
    grey40: f64,
    mid: f64,
    two: f64,
    four: f64,
    left: [Code; 3],
    right: [Code; 3],
    ramp: (usize, usize),
}

const NARROW: (f64, f64) = (64.0, 876.0);
const NARROW_RAMP: (usize, usize) = (858, 876);

const BT2111_HLG_NARROW: Bt2111 = Bt2111 {
    range: NARROW,
    grey40: 414.0,
    mid: 721.0,
    two: 80.0,
    four: 99.0,
    left: [
        [713.0, 719.0, 316.0],
        [538.0, 709.0, 718.0],
        [512.0, 706.0, 296.0],
    ],
    right: [
        [651.0, 286.0, 705.0],
        [639.0, 269.0, 164.0],
        [227.0, 147.0, 702.0],
    ],
    ramp: NARROW_RAMP,
};

const BT2111_PQ_NARROW: Bt2111 = Bt2111 {
    range: NARROW,
    grey40: 414.0,
    mid: 572.0,
    two: 80.0,
    four: 99.0,
    left: [
        [568.0, 571.0, 381.0],
        [484.0, 566.0, 571.0],
        [474.0, 564.0, 368.0],
    ],
    right: [
        [536.0, 361.0, 564.0],
        [530.0, 350.0, 256.0],
        [317.0, 236.0, 562.0],
    ],
    ramp: NARROW_RAMP,
};

const BT2111_PQ_FULL: Bt2111 = Bt2111 {
    range: (0.0, 1023.0),
    grey40: 409.0,
    mid: 593.0,
    two: 20.0,
    four: 41.0,
    left: [
        [589.0, 592.0, 370.0],
        [491.0, 586.0, 592.0],
        [478.0, 584.0, 355.0],
    ],
    right: [
        [551.0, 347.0, 584.0],
        [544.0, 334.0, 225.0],
        [296.0, 201.0, 582.0],
    ],
    ramp: (791, 1022),
};

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
    Chart(Chart),
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

fn grey(v: f64) -> Code {
    [v; 3]
}

fn row_end(k: u16, h: u32) -> u32 {
    (f32::from(k) / 12.0 * h as f32).floor() as u32
}

fn columns(w: u32, code: impl Fn(u32) -> Code) -> Vec<Code> {
    (0..w).map(code).collect()
}

fn paint(row: &mut [Code], from: usize, to: usize, code: Code) {
    if let Some(span) = row.get_mut(from..to.min(row.len())) {
        span.fill(code);
    }
}

fn ten_step(w: u32) -> Vec<Code> {
    let band = w.div_ceil(11);
    columns(w, |x| grey(f64::from((x / band).min(10)) / 10.0))
}

fn hundred_mv_steps(w: u32) -> Vec<Code> {
    let band = w.div_ceil(10);
    columns(w, |x| {
        grey(f64::from((x / band).saturating_sub(1).min(7)) / 7.0)
    })
}

fn ycbcr_ramp(w: u32) -> Vec<Code> {
    columns(w, |x| {
        let t = f64::from(x) / f64::from(w);
        YCBCR_SLOPES.map(|slope| (0.5 + slope * (t - 0.5)).clamp(0.0, 1.0))
    })
}

fn ebu_bars(level: EbuLevel, w: u32) -> Vec<Code> {
    let at = match level {
        EbuLevel::Percent75 => 0.75,
        EbuLevel::Percent100 => 1.0,
    };
    let band = w.div_ceil(8);
    columns(w, |x| match (x / band).min(7) as usize {
        0 => grey(1.0),
        bar @ 1..=6 => BAR_COLORS[bar].map(|v| v * at),
        _ => grey(0.0),
    })
}

fn smpte_bars(w: u32, h: u32) -> Vec<Band> {
    let base = if w < 1920 {
        1920.0
    } else {
        f64::from(2 * (8 * h / 9))
            .min(f64::from(w) * 4.0 / 3.0)
            .max(1920.0)
    };
    let c = (base * 3.0 / 28.0).ceil() as usize;
    let u = 2 * c.div_ceil(6);
    let full = (w as usize).max(1920);
    let d = full.saturating_sub(7 * c) / 2;
    let end = d + 7 * c;

    let mut bars = vec![grey(0.4); full];
    for (n, color) in BAR_COLORS.iter().enumerate() {
        paint(
            &mut bars,
            d + n * c,
            d + (n + 1) * c,
            color.map(|v| v * 0.75),
        );
    }

    let mut second = vec![grey(0.75); full];
    paint(&mut second, 0, d, [0.0, 1.0, 1.0]);
    paint(&mut second, d, d + c, grey(1.0));
    paint(&mut second, end, full, [0.0, 0.0, 1.0]);

    let ramp_from = (d + c) as f64;
    let ramp_span = end.min(full) as f64 - ramp_from;
    let mut third: Vec<Code> = (0..full)
        .map(|x| grey(((x as f64 - ramp_from) / ramp_span).clamp(0.0, 1.0)))
        .collect();
    paint(&mut third, 0, d, [1.0, 1.0, 0.0]);
    paint(&mut third, end, full, [1.0, 0.0, 0.0]);

    let mut last = vec![grey(0.15); full];
    paint(&mut last, d, end, grey(0.0));
    let white = d + 9 * u / 2;
    paint(&mut last, white, white + 6 * u, grey(1.0));
    let near_black = white + 6 * u + 9 * u / 2;
    paint(&mut last, near_black, near_black + u, grey(0.02));
    paint(
        &mut last,
        near_black + 2 * u,
        near_black + 3 * u,
        grey(0.04),
    );

    vec![
        (row_end(7, h), bars),
        (row_end(8, h), second),
        (row_end(9, h), third),
        (h, last),
    ]
}

fn bt2111_bars(codes: &Bt2111, w: u32, h: u32) -> Vec<Band> {
    let full = w as usize;
    let of = |code: f64| (code - codes.range.0) / codes.range.1;
    let mid = of(codes.mid);
    let d = full / 8;
    let c = full * 206 / 1920;
    let bars = [
        d,
        d + c,
        d + 2 * c,
        d + 3 * c,
        full - d - 3 * c,
        full - d - 2 * c,
        full - d - c,
        full - d,
    ];
    let half = |b: usize| bars[b] + (bars[b + 1] - bars[b]) / 2;

    let mut top = vec![grey(of(codes.grey40)); full];
    let mut second = top.clone();
    for (b, color) in BAR_COLORS.iter().enumerate() {
        paint(&mut top, bars[b], bars[b + 1], *color);
        paint(&mut second, bars[b], bars[b + 1], color.map(|v| v * mid));
    }

    let mut stairs = vec![grey(mid); full];
    paint(&mut stairs, d, half(1), grey(0.0));
    let edges: Vec<usize> = (1..=5).flat_map(|b| [half(b), bars[b + 1]]).collect();
    for (step, pair) in edges.windows(2).enumerate() {
        paint(
            &mut stairs,
            pair[0],
            pair[1],
            grey((step + 1) as f64 / 10.0),
        );
    }
    paint(&mut stairs, bars[6], bars[7], grey(1.0));

    let ramp_from = (full * codes.ramp.0 / 1920) as f64;
    let ramp_span = full as f64 * codes.ramp.1 as f64 / 1920.0;
    let ramp: Vec<Code> = (0..full)
        .map(|x| grey(((x as f64 - ramp_from) / ramp_span).clamp(0.0, 1.0)))
        .collect();

    let mut last = vec![grey(0.0); full];
    for (n, (left, right)) in codes.left.iter().zip(&codes.right).enumerate() {
        let (from, to) = (d * n / 3, d * (n + 1) / 3);
        paint(&mut last, from, to, left.map(of));
        paint(&mut last, full - d + from, full - d + to, right.map(of));
    }
    let gap = ((c as f64 - full as f64 * 70.0 / 1920.0) / 2.0).floor() as usize;
    let patch = c - 2 * gap;
    let two = bars[1] + gap;
    paint(&mut last, two, two + patch, grey(of(codes.two)));
    paint(&mut last, bars[2], bars[2] + patch, grey(of(codes.four)));
    let middle = full / 2;
    paint(&mut last, middle, middle + full * 438 / 1920, grey(mid));

    vec![
        (row_end(1, h), top),
        (row_end(7, h), second),
        (row_end(8, h), stairs),
        (row_end(9, h), ramp),
        (h, last),
    ]
}

fn window(w: u32, h: u32) -> Vec<Band> {
    let black = vec![grey(0.0)];
    let box_row = columns(w, |x| {
        grey(if x > w / 4 && x < 3 * w / 4 { 1.0 } else { 0.0 })
    });
    vec![(h / 4 + 1, black.clone()), (3 * h / 4, box_row), (h, black)]
}

fn bands(chart: Chart, w: u32, h: u32) -> Vec<Band> {
    let whole = |codes| vec![(h, codes)];
    match chart {
        Chart::TenStep => whole(ten_step(w)),
        Chart::HundredMvSteps => whole(hundred_mv_steps(w)),
        Chart::GreyScale => whole(columns(w, |x| grey(f64::from(x) / f64::from(w)))),
        Chart::YcbcrRamp => whole(ycbcr_ramp(w)),
        Chart::EbuColorBar(level) => whole(ebu_bars(level, w)),
        Chart::SmpteColorBar => smpte_bars(w, h),
        Chart::Bt2111HlgNarrow => bt2111_bars(&BT2111_HLG_NARROW, w, h),
        Chart::Bt2111PqFull => bt2111_bars(&BT2111_PQ_FULL, w, h),
        Chart::Bt2111PqNarrow => bt2111_bars(&BT2111_PQ_NARROW, w, h),
        Chart::Window => window(w, h),
    }
}

fn draw_bands(all: &[Band], data: &mut [u8], stride: usize, width: usize) {
    let mut rest = data;
    let mut done = 0;
    for (end, codes) in all {
        let bytes = ((end - done) as usize * stride).min(rest.len());
        done = *end;
        let (here, after) = std::mem::take(&mut rest).split_at_mut(bytes);
        rest = after;
        if !here.is_empty() {
            same_rows(here, stride, width, codes);
        }
    }
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
        Pattern::Chart(chart) => draw_bands(&bands(chart, w, h), data, stride, width),
    }
}

pub fn name(g: &Generator) -> &str {
    match g {
        Generator::SolidColor { .. } => Generator::SOLID_COLOR,
        Generator::FourColorGradient(_) => Generator::FOUR_COLOR_GRADIENT,
        Generator::TenStep => Generator::TEN_STEP,
        Generator::HundredMvSteps => Generator::HUNDRED_MV_STEPS,
        Generator::GreyScale => Generator::GREY_SCALE,
        Generator::YcbcrRamp => Generator::YCBCR_RAMP,
        Generator::EbuColorBar { .. } => Generator::EBU_COLOR_BAR,
        Generator::SmpteColorBar => Generator::SMPTE_COLOR_BAR,
        Generator::Bt2111HlgNarrow => Generator::BT2111_HLG_NARROW,
        Generator::Bt2111PqFull => Generator::BT2111_PQ_FULL,
        Generator::Bt2111PqNarrow => Generator::BT2111_PQ_NARROW,
        Generator::Window => Generator::WINDOW,
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
