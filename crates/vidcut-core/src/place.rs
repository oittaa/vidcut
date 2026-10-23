use crate::model::{Picture, VideoInfo};
use crate::render::Placed;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub back: [f64; 8],
    pub shown: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Placing {
    pub levels: Vec<Level>,
}

pub fn flipped(p: &Picture, [x, y]: [f64; 2]) -> [f64; 2] {
    let sx = if p.flip_horizontal { -1.0 } else { 1.0 };
    let sy = if p.flip_vertical { -1.0 } else { 1.0 };
    [0.5 + sx * (x - 0.5), 0.5 + sy * (y - 0.5)]
}

pub fn anchored(p: &Picture, (w, h): (f64, f64), [fx, fy, fw, fh]: [f64; 4]) -> [f64; 2] {
    let ([px, py], [ax, ay]) = (p.place(), p.pivot());
    [fx + ax * fw + (px - 0.5) * w, fy + ay * fh + (py - 0.5) * h]
}

fn level(placed: &Placed, frame: (f64, f64), fitted: [f64; 4]) -> Level {
    let p = &placed.picture;
    let [_, _, fw, fh] = fitted;
    let [zx, zy] = p.scale();
    let [ax, ay] = p.pivot();
    let (sin, cos) = p.turn().to_radians().sin_cos();
    let sx = if p.flip_horizontal { -1.0 } else { 1.0 };
    let sy = if p.flip_vertical { -1.0 } else { 1.0 };
    let [qx, qy] = anchored(p, frame, fitted);
    let (dx, dy) = (zx * (ax - 0.5) * fw, zy * (ay - 0.5) * fh);
    let (ox, oy) = (qx - cos * dx - sin * dy, qy + sin * dx - cos * dy);
    let (k, [u0, v0]) = placed
        .zoomed()
        .map_or((1.0, Picture::MIDDLE), |b| (b.size, flipped(p, b.center)));
    let (a, b) = (k * sx * cos / (zx * fw), -k * sx * sin / (zx * fw));
    let (c, d) = (k * sy * sin / (zy * fh), k * sy * cos / (zy * fh));
    let c0 = p.crop;
    let affine = [
        a,
        b,
        u0 - a * ox - b * oy,
        c,
        d,
        v0 - c * ox - d * oy,
        0.0,
        0.0,
    ];
    Level {
        back: leaned(
            affine,
            [qx, qy],
            [zx * fw / 2.0, zy * fh / 2.0],
            p.pitch(),
            p.yaw(),
        ),
        shown: [c0.left, c0.top, 1.0 - c0.right, 1.0 - c0.bottom],
    }
}

fn leaned(
    affine: [f64; 8],
    [qx, qy]: [f64; 2],
    [hw, hh]: [f64; 2],
    pitch: f64,
    yaw: f64,
) -> [f64; 8] {
    if (pitch == 0.0 && yaw == 0.0) || hw.abs() < 1e-12 || hh.abs() < 1e-12 {
        return affine;
    }
    let [a, b, e, c, d, f, _, _] = affine;
    let g0 = -yaw / hw;
    let h0 = pitch / hh;
    let w0 = 1.0 + yaw * qx / hw - pitch * qy / hh;
    let row = |p: f64, q: f64, r: f64| {
        [
            p * (qx * g0 + 1.0) + q * (qy * g0) + r * g0,
            p * (qx * h0) + q * (qy * h0 + 1.0) + r * h0,
            p * qx * (w0 - 1.0) + q * qy * (w0 - 1.0) + r * w0,
        ]
    };
    let [a, b, e] = row(a, b, e);
    let [c, d, f] = row(c, d, f);
    unit([a, b, e, c, d, f, g0, h0, w0])
}

fn unit([a, b, e, c, d, f, g, h, i]: [f64; 9]) -> [f64; 8] {
    let s = if i.abs() < 1e-8 {
        if i.is_sign_negative() { -1e-8 } else { 1e-8 }
    } else {
        i
    };
    [a / s, b / s, e / s, c / s, d / s, f / s, g / s, h / s]
}

pub const SAME: [f64; 8] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

pub fn chain(first: &[f64; 8], next: &[f64; 8]) -> [f64; 8] {
    let [a1, b1, e1, c1, d1, f1, g1, h1] = *first;
    let [a2, b2, e2, c2, d2, f2, g2, h2] = *next;
    unit([
        a2 * a1 + b2 * c1 + e2 * g1,
        a2 * b1 + b2 * d1 + e2 * h1,
        a2 * e1 + b2 * f1 + e2,
        c2 * a1 + d2 * c1 + f2 * g1,
        c2 * b1 + d2 * d1 + f2 * h1,
        c2 * e1 + d2 * f1 + f2,
        g2 * a1 + h2 * c1 + g1,
        g2 * b1 + h2 * d1 + h1,
        g2 * e1 + h2 * f1 + 1.0,
    ])
}

fn scaled(m: &[f64; 8], (w, h): (f64, f64)) -> [f64; 8] {
    let [a, b, e, c, d, f, g, k] = *m;
    [a * w, b * w, e * w, c * h, d * h, f * h, g, k]
}

fn then(outer: &[f64; 8], inner: &[f64; 8], frame: (f64, f64)) -> [f64; 8] {
    chain(&scaled(outer, frame), inner)
}

pub fn apply(m: &[f64; 8], [x, y]: [f64; 2]) -> [f64; 2] {
    let [a, b, e, c, d, f, g, h] = *m;
    let w = g * x + h * y + 1.0;
    [(a * x + b * y + e) / w, (c * x + d * y + f) / w]
}

pub fn ahead(m: &[f64; 8], [u, v]: [f64; 2]) -> [f64; 2] {
    let [a, b, e, c, d, f, g, h] = *m;
    let (a, b, c, d) = (a - u * g, b - u * h, c - v * g, d - v * h);
    let det = a * d - b * c;
    let (u, v) = (u - e, v - f);
    [(d * u - b * v) / det, (a * v - c * u) / det]
}

pub fn lettered([px, py]: [f64; 2], zoom: f64, radians: f64, [ox, oy]: [f64; 2]) -> [f64; 8] {
    let (sin, cos) = radians.sin_cos();
    let (a, b, c, d) = (cos / zoom, -sin / zoom, sin / zoom, cos / zoom);
    [
        a,
        b,
        px - a * px - b * py - ox,
        c,
        d,
        py - c * px - d * py - oy,
        0.0,
        0.0,
    ]
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
            let at = if i == 0 { fitted } else { whole };
            if i == 0 && !levels.is_empty() && at == whole && p.picture.is_usual() {
                continue;
            }
            let own = level(p, frame, at);
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

    pub fn whole(placed: &[Placed], frame: (f64, f64)) -> Self {
        Self::new(placed, frame, [0.0, 0.0, frame.0, frame.1])
    }

    pub fn inner(&self, frame: (f64, f64)) -> [f64; 8] {
        self.levels.last().map_or(SAME, |l| scaled(&l.back, frame))
    }

    pub fn at(&self, x: f64, y: f64) -> Option<([f64; 2], f64)> {
        let mut cover = 1.0;
        let mut uv = [0.0, 0.0];
        for l in &self.levels {
            let [a, b, e, c, d, f, g, h] = l.back;
            let w = g * x + h * y + 1.0;
            if !w.is_finite() || w.abs() < 1e-12 {
                return None;
            }
            let (u, v) = ((a * x + b * y + e) / w, (c * x + d * y + f) / w);
            if !u.is_finite() || !v.is_finite() {
                return None;
            }
            let (du, dv) = (
                (a - u * g).hypot(b - u * h) / w.abs(),
                (c - v * g).hypot(d - v * h) / w.abs(),
            );
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
            let [a, b, _, c, d, _, _, _] = l.back;
            [a, b, c, d]
        })
    }

    pub fn share(&self, (w, h): (f64, f64)) -> f64 {
        let Some(l) = self.levels.last() else {
            return 0.0;
        };
        let [u0, v0, u1, v1] = l.shown;
        let [a, b, _, c, d, _, g, k] = l.back;
        let corners = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|p| ahead(&l.back, p));
        let quad = if g == 0.0 && k == 0.0 {
            (u1 - u0) * (v1 - v0) / (a * d - b * c).abs()
        } else {
            quad_area(corners).unwrap_or(0.0)
        };
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
            if corners
                .iter()
                .any(|c| !c[0].is_finite() || !c[1].is_finite())
            {
                continue;
            }
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

fn quad_area(c: [[f64; 2]; 4]) -> Option<f64> {
    if c.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return None;
    }
    let mut s = 0.0;
    for i in 0..4 {
        let [x0, y0] = c[i];
        let [x1, y1] = c[(i + 1) % 4];
        s += x0 * y1 - x1 * y0;
    }
    Some((s * 0.5).abs())
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
            let near = p.picture.dynamic_zoom.map_or(1.0, |z| z.closest());
            x.max(y) / near
        })
        .product()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing(picture: Picture, uv: [f64; 2]) -> [f64; 2] {
        let placed = Placed {
            clip: "c1".into(),
            picture,
            run: None,
        };
        let frame = (1920.0, 1080.0);
        let level = Placing::whole(&[placed], frame).levels[0];
        level.forward(uv).map(|v| (v * 1e6).round() / 1e6)
    }

    #[test]
    fn a_zoom_works_about_the_anchor() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.5, 0.5]), [480.0, 540.0]);
    }

    #[test]
    fn a_rotation_works_about_the_anchor() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            rotation: Some(90.0),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.5, 0.5]), [1440.0, 1020.0]);
    }

    #[test]
    fn the_anchor_stays_where_the_position_puts_it() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(90.0),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.75, 0.5]), [1440.0, 540.0]);
    }

    #[test]
    fn the_anchor_is_a_point_of_the_flipped_picture() {
        let p = Picture {
            anchor: Some([0.75, 0.5]),
            zoom: Some([2.0, 2.0]),
            flip_horizontal: true,
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.25, 0.5]), [1440.0, 540.0]);
    }

    #[test]
    fn a_yaw_of_one_half_lands_the_top_left_corner() {
        let p = Picture {
            yaw: Some(0.5),
            ..Picture::default()
        };
        assert_eq!(landing(p, [0.0, 0.0]), [-960.0, -540.0]);
    }

    #[test]
    fn a_pitch_of_one_half_lands_the_bottom_right_corner() {
        let p = Picture {
            pitch: Some(0.5),
            ..Picture::default()
        };
        assert_eq!(landing(p, [1.0, 1.0]), [2880.0, 1620.0]);
    }
}
