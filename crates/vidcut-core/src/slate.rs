use crate::generators::{RGBA_F32, each_row, put_row, timeline_code};
use crate::model::Hdr;

pub const TEXT: &str = "Media Offline";

const BLACK: [u8; 3] = [0, 0, 0];
const GROUND: [u8; 3] = [56, 6, 6];
const INK: [u8; 3] = [159, 35, 34];

const UNITS: f64 = 1080.0;
const SIGN_MIDDLE: f64 = 1.0;
const APEX: f64 = 303.65;
const BASE: f64 = 538.0;
const HALF_BASE: f64 = 137.85;
const CORNER: f64 = 6.6;
const STEM_TOP: f64 = 385.2;
const STEM_TOP_HALF: f64 = 12.25;
const STEM_BOTTOM: f64 = 477.0;
const STEM_BOTTOM_HALF: f64 = 6.1;
const STEM_CORNER: f64 = 3.0;
const DOT: f64 = 504.0;
const DOT_RADIUS: f64 = 12.0;
const TEXT_SIZE: f64 = 120.0;
const BASELINE: f64 = 737.0;

pub struct Coverage {
    pub left: i32,
    pub top: i32,
    pub width: usize,
    pub alpha: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl Band {
    fn of((w, h): (u32, u32)) -> Self {
        let (w, h) = (f64::from(w), f64::from(h));
        let (width, height) = if w * 9.0 > h * 16.0 {
            (h * 16.0 / 9.0, h)
        } else {
            (w, w * 9.0 / 16.0)
        };
        Self {
            left: (w - width) / 2.0,
            top: (h - height) / 2.0,
            width,
            height,
        }
    }

    fn unit(&self) -> f64 {
        self.height / UNITS
    }

    fn point(&self, x: f64, y: f64) -> [f64; 2] {
        let u = self.unit();
        [self.left + self.width / 2.0 + x * u, self.top + y * u]
    }

    fn covers(&self, x: f64, y: f64) -> f64 {
        let span =
            |at: f64, from: f64, to: f64| ((at + 1.0).min(to) - at.max(from)).clamp(0.0, 1.0);
        span(x, self.left, self.left + self.width) * span(y, self.top, self.top + self.height)
    }
}

struct Sign {
    triangle: Vec<[f64; 2]>,
    stem: Vec<[f64; 2]>,
    stem_top: [f64; 2],
    dot: [f64; 2],
    unit: f64,
    edges: [f64; 4],
}

impl Sign {
    fn of(b: &Band) -> Self {
        let u = b.unit();
        let at = |x: f64, y: f64| b.point(SIGN_MIDDLE + x, y);
        let triangle = [at(0.0, APEX), at(HALF_BASE, BASE), at(-HALF_BASE, BASE)];
        let stem = [
            at(-STEM_TOP_HALF, STEM_TOP),
            at(STEM_TOP_HALF, STEM_TOP),
            at(STEM_BOTTOM_HALF, STEM_BOTTOM),
            at(-STEM_BOTTOM_HALF, STEM_BOTTOM),
        ];
        let [left, top] = at(-HALF_BASE, APEX);
        let [right, bottom] = at(HALF_BASE, BASE);
        Self {
            triangle: inset(&triangle, CORNER * u),
            stem: inset(&stem, STEM_CORNER * u),
            stem_top: at(0.0, STEM_TOP),
            dot: at(0.0, DOT),
            unit: u,
            edges: [left - 1.0, top - 1.0, right + 1.0, bottom + 1.0],
        }
    }

    fn ink(&self, p: [f64; 2]) -> f64 {
        let [left, top, right, bottom] = self.edges;
        if p[0] < left || p[0] > right || p[1] < top || p[1] > bottom {
            return 0.0;
        }
        let u = self.unit;
        let triangle = covered(distance(p, &self.triangle) - CORNER * u);
        let stem = covered(distance(p, &self.stem) - STEM_CORNER * u)
            .max(covered(apart(p, self.stem_top) - STEM_TOP_HALF * u));
        let dot = covered(apart(p, self.dot) - DOT_RADIUS * u);
        triangle * (1.0 - stem.max(dot))
    }
}

fn covered(distance: f64) -> f64 {
    (0.5 - distance).clamp(0.0, 1.0)
}

fn apart(p: [f64; 2], q: [f64; 2]) -> f64 {
    (p[0] - q[0]).hypot(p[1] - q[1])
}

fn outward(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    [dy / len, -dx / len]
}

fn inset(corners: &[[f64; 2]], by: f64) -> Vec<[f64; 2]> {
    let n = corners.len();
    (0..n)
        .map(|i| {
            let v = corners[i];
            let before = outward(corners[(i + n - 1) % n], v);
            let after = outward(v, corners[(i + 1) % n]);
            let k = by / (1.0 + before[0] * after[0] + before[1] * after[1]);
            [
                v[0] - k * (before[0] + after[0]),
                v[1] - k * (before[1] + after[1]),
            ]
        })
        .collect()
}

fn distance(p: [f64; 2], corners: &[[f64; 2]]) -> f64 {
    let mut plane = f64::MIN;
    let mut edge = f64::MAX;
    for (i, &a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % corners.len()];
        let n = outward(a, b);
        let (px, py) = (p[0] - a[0], p[1] - a[1]);
        plane = plane.max(px * n[0] + py * n[1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        edge = edge.min((px - t * dx).hypot(py - t * dy));
    }
    if plane <= 0.0 { plane } else { edge }
}

fn mixed(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [0, 1, 2].map(|c| a[c] + (b[c] - a[c]) * t)
}

pub fn draw((w, h): (u32, u32), colors: Option<Hdr>, data: &mut [u8], stride: usize) {
    let band = Band::of((w, h));
    let sign = Sign::of(&band);
    let [black, ground, ink] = [BLACK, GROUND, INK].map(|c| timeline_code(c, colors));
    let width = w as usize;
    each_row(data, stride, h as usize, |i, line| {
        let y = i as f64;
        let codes = (0..width).map(|j| {
            let x = j as f64;
            let face = mixed(ground, ink, sign.ink([x + 0.5, y + 0.5]));
            mixed(black, face, band.covers(x, y))
        });
        put_row(&mut line[..width * RGBA_F32], codes);
    });
}

pub fn text((w, h): (u32, u32)) -> (f32, [f32; 2]) {
    let band = Band::of((w, h));
    let [x, y] = band.point(0.0, BASELINE);
    ((TEXT_SIZE * band.unit()) as f32, [x as f32, y as f32])
}

pub fn put_text(
    text: &Coverage,
    (w, h): (u32, u32),
    colors: Option<Hdr>,
    data: &mut [u8],
    stride: usize,
) {
    let ink = timeline_code(INK, colors);
    for (j, row) in text.alpha.chunks(text.width.max(1)).enumerate() {
        let y = text.top + j as i32;
        if y < 0 || y >= h as i32 {
            continue;
        }
        let line = &mut data[y as usize * stride..];
        for (i, &a) in row.iter().enumerate() {
            let x = text.left + i as i32;
            if a <= 0.0 || x < 0 || x >= w as i32 {
                continue;
            }
            let at = x as usize * RGBA_F32;
            let pixel = &mut line[at..at + RGBA_F32];
            let parts = pixel.as_chunks_mut::<4>().0;
            let now = [0, 1, 2].map(|c| f64::from(f32::from_le_bytes(parts[c])));
            for (bytes, v) in parts.iter_mut().zip(mixed(now, ink, f64::from(a))) {
                *bytes = (v as f32).to_le_bytes();
            }
        }
    }
}
