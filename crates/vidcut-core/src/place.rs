use crate::model::{Picture, VideoInfo};
use crate::render::Placed;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub back: [f64; 6],
    pub shown: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placing {
    pub levels: Vec<Level>,
}

fn level(p: &Picture, (w, h): (f64, f64), [fx, fy, fw, fh]: [f64; 4]) -> Level {
    let [px, py] = p.place();
    let [zx, zy] = p.scale();
    let (sin, cos) = p.turn().to_radians().sin_cos();
    let sx = if p.flip_horizontal { -1.0 } else { 1.0 };
    let sy = if p.flip_vertical { -1.0 } else { 1.0 };
    let (ox, oy) = (
        fx + fw / 2.0 + (px - 0.5) * w,
        fy + fh / 2.0 + (py - 0.5) * h,
    );
    let (a, b) = (sx * cos / (zx * fw), -sx * sin / (zx * fw));
    let (c, d) = (sy * sin / (zy * fh), sy * cos / (zy * fh));
    let c0 = p.crop;
    Level {
        back: [a, b, 0.5 - a * ox - b * oy, c, d, 0.5 - c * ox - d * oy],
        shown: [c0.left, c0.top, 1.0 - c0.right, 1.0 - c0.bottom],
    }
}

fn then(outer: &[f64; 6], inner: &[f64; 6], (w, h): (f64, f64)) -> [f64; 6] {
    let [a1, b1, e1, c1, d1, f1] = *outer;
    let [a2, b2, e2, c2, d2, f2] = *inner;
    let (a2, b2, c2, d2) = (a2 * w, b2 * h, c2 * w, d2 * h);
    [
        a2 * a1 + b2 * c1,
        a2 * b1 + b2 * d1,
        a2 * e1 + b2 * f1 + e2,
        c2 * a1 + d2 * c1,
        c2 * b1 + d2 * d1,
        c2 * e1 + d2 * f1 + f2,
    ]
}

fn ahead(m: &[f64; 6], [u, v]: [f64; 2]) -> [f64; 2] {
    let [a, b, e, c, d, f] = *m;
    let det = a * d - b * c;
    let (u, v) = (u - e, v - f);
    [(d * u - b * v) / det, (a * v - c * u) / det]
}

impl Level {
    pub fn forward(&self, uv: [f64; 2]) -> [f64; 2] {
        ahead(&self.back, uv)
    }
}

impl Placing {
    pub fn new(placed: &[Placed], frame: (f64, f64), fitted: [f64; 4]) -> Self {
        let whole = [0.0, 0.0, frame.0, frame.1];
        let mut levels: Vec<Level> = Vec::with_capacity(placed.len());
        for (i, p) in placed.iter().enumerate().rev() {
            let own = level(&p.picture, frame, if i == 0 { fitted } else { whole });
            let back = match levels.last() {
                Some(outer) => then(&outer.back, &own.back, frame),
                None => own.back,
            };
            levels.push(Level {
                back,
                shown: own.shown,
            });
        }
        Self { levels }
    }

    pub fn at(&self, x: f64, y: f64) -> Option<([f64; 2], f64)> {
        let mut cover = 1.0;
        let mut uv = [0.0, 0.0];
        for l in &self.levels {
            let [a, b, e, c, d, f] = l.back;
            let (u, v) = (a * x + b * y + e, c * x + d * y + f);
            let (du, dv) = (a.hypot(b), c.hypot(d));
            let [u0, v0, u1, v1] = l.shown;
            for edge in [(u - u0) / du, (u1 - u) / du, (v - v0) / dv, (v1 - v) / dv] {
                cover *= (edge + 0.5).clamp(0.0, 1.0);
            }
            if cover <= 0.0 {
                return None;
            }
            uv = [u, v];
        }
        Some((uv, cover))
    }

    pub fn step(&self) -> [f64; 4] {
        self.levels.last().map_or([0.0; 4], |l| {
            let [a, b, _, c, d, _] = l.back;
            [a, b, c, d]
        })
    }

    pub fn share(&self, (w, h): (f64, f64)) -> f64 {
        let Some(l) = self.levels.last() else {
            return 0.0;
        };
        let [a, b, _, c, d, _] = l.back;
        let [u0, v0, u1, v1] = l.shown;
        let quad = (u1 - u0) * (v1 - v0) / (a * d - b * c).abs();
        let boxed = self.bounds((w, h)).map_or(0.0, |[x0, y0, x1, y1]| {
            f64::from(x1 - x0) * f64::from(y1 - y0)
        });
        (quad.min(boxed) / (w * h)).clamp(0.0, 1.0)
    }

    pub fn bounds(&self, (w, h): (f64, f64)) -> Option<[u32; 4]> {
        let mut out = [0.0, 0.0, w, h];
        for l in &self.levels {
            let [u0, v0, u1, v1] = l.shown;
            let corners = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|c| ahead(&l.back, c));
            let xs = corners.map(|c| c[0]);
            let ys = corners.map(|c| c[1]);
            let lo = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min) - 1.0;
            let hi = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max) + 1.0;
            out = [
                out[0].max(lo(xs)),
                out[1].max(lo(ys)),
                out[2].min(hi(xs)),
                out[3].min(hi(ys)),
            ];
        }
        let [x0, y0, x1, y1] = out.map(|v| v.clamp(0.0, w.max(h)));
        let (x0, y0) = (x0.floor() as u32, y0.floor() as u32);
        let (x1, y1) = (x1.ceil().min(w) as u32, y1.ceil().min(h) as u32);
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
    }
}

pub fn fit(w: u32, h: u32, sar: (i32, i32), out_w: u32, out_h: u32) -> (u32, u32) {
    let sar = if sar.0 > 0 && sar.1 > 0 {
        f64::from(sar.0) / f64::from(sar.1)
    } else {
        1.0
    };
    let dar = f64::from(w) * sar / f64::from(h);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if dar > f64::from(out_w) / f64::from(out_h) {
        (out_w, even(f64::from(out_w) / dar).min(out_h))
    } else {
        (even(f64::from(out_h) * dar).min(out_w), out_h)
    }
}

pub fn upright_fit(
    (w, h): (u32, u32),
    sar: (i32, i32),
    rotation: u16,
    (out_w, out_h): (u32, u32),
) -> (u32, u32) {
    if rotation % 180 == 90 {
        fit(h, w, (sar.1, sar.0), out_w, out_h)
    } else {
        fit(w, h, sar, out_w, out_h)
    }
}

pub fn fills(v: &VideoInfo, frame: (u32, u32)) -> bool {
    let sar = (v.sar[0], v.sar[1]);
    upright_fit((v.width, v.height), sar, v.rotation, frame) == frame
}

pub fn fitted((w, h): (u32, u32), (fw, fh): (u32, u32)) -> [f64; 4] {
    [
        f64::from((w - fw.min(w)) / 2),
        f64::from((h - fh.min(h)) / 2),
        f64::from(fw),
        f64::from(fh),
    ]
}

pub fn detail(placed: &[Placed]) -> f64 {
    placed
        .iter()
        .map(|p| {
            let [x, y] = p.picture.scale();
            x.max(y)
        })
        .product()
}
