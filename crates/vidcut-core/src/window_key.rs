use crate::model::{CurvePoint, GradeNode, GradeWindow, KeyOutput, WindowShape};

pub const PAN_STEP: f64 = 81.92;
pub const SOFT_STEP: f64 = 16.0;
pub const GRADIENT_SOFT_STEP: f64 = 100.0;
pub const POLYGON_SOFT_STEP: f64 = 1.0 / 25.0;
pub const CIRCLE_RADIUS: f64 = 0.15;
pub const CIRCLE_LEAST: f64 = 0.002;
pub const CIRCLE_GROWTH: f64 = 0.08;
pub const LINEAR_MOST: f64 = 4.0;
pub const FAR_OUTSIDE: f64 = 1000.0;
pub const EVEN: f64 = 1e-6;
pub const TRIG_ZERO: f64 = 1e-12;
pub const CURVE_INSIDE_STEP: f64 = std::f64::consts::SQRT_2 / 1000.0;
pub const CURVE_OUTSIDE_STEP: f64 = 3.0 * CURVE_INSIDE_STEP;
pub const CURVE_BLUR_STEP: f64 = 0.005;
pub const CURVE_SAMPLE: f64 = 0.5;
pub const CURVE_HANDLE: f64 = 0.5519;
pub const FLAT_ERROR: f64 = 0.05;
pub const FLAT_DEPTH: u32 = 8;
pub const MITER_MOST: f64 = 4.0;
pub const SAME_POINT: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub enum Outline {
    Circle {
        radius: f64,
        soft: f64,
        shrink: [f64; 2],
    },
    Linear {
        corners: [[f64; 2]; 4],
        scale: [f64; 2],
        soft: [f64; 4],
    },
    Polygon {
        points: Vec<[f64; 2]>,
        inside: f64,
        outside: f64,
    },
    Curve {
        controls: Vec<CurvePoint>,
        path: Vec<[f64; 2]>,
        inner: Vec<[f64; 2]>,
        outer: Vec<[f64; 2]>,
        spread: Vec<[[f64; 2]; 2]>,
        bends: Vec<[f64; 2]>,
        blur: f64,
    },
    Gradient {
        soft: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shaped {
    pub outline: Outline,
    pub middle: [f64; 2],
    pub turn: [f64; 2],
    pub invert: bool,
    pub mask: bool,
    pub opacity: f64,
    pub starts: bool,
    pub ends: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keying {
    pub windows: Vec<GradeWindow>,
    pub output: KeyOutput,
}

fn joined_first(w: &GradeWindow) -> (bool, u8) {
    let order = match w.shape {
        WindowShape::Circle { .. } => 0,
        WindowShape::Linear { .. } => 1,
        WindowShape::Polygon { .. } => 2,
        WindowShape::Curve { .. } => 3,
        WindowShape::Gradient { .. } => 4,
    };
    (w.mask, order)
}

pub fn linear_scale(size: f64) -> f64 {
    match size <= 50.0 {
        true => 0.025 + 0.975 * size / 50.0,
        false => 1.0 + 0.06 * (size - 50.0),
    }
}

pub fn circle_scale(size: f64) -> f64 {
    match size <= 50.0 {
        true => CIRCLE_LEAST + (1.0 - CIRCLE_LEAST) * size / 50.0,
        false => 1.0 + CIRCLE_GROWTH * (size - 50.0),
    }
}

pub fn circle_size(scale: f64) -> f64 {
    match scale <= 1.0 {
        true => (scale - CIRCLE_LEAST) / (1.0 - CIRCLE_LEAST) * 50.0,
        false => 50.0 + (scale - 1.0) / CIRCLE_GROWTH,
    }
}

pub fn origin(window: &GradeWindow, [w, h]: [f64; 2]) -> [f64; 2] {
    [
        w / 2.0 + (window.pan - 50.0) * PAN_STEP,
        h / 2.0 - (window.tilt - 50.0) * PAN_STEP,
    ]
}

pub fn aspect_shrink(aspect: f64) -> [f64; 2] {
    [
        if aspect > 50.0 {
            (100.0 - aspect) / 50.0
        } else {
            1.0
        },
        if aspect < 50.0 { aspect / 50.0 } else { 1.0 },
    ]
}

impl Keying {
    pub fn of(node: &GradeNode) -> Option<Self> {
        let windows: Vec<GradeWindow> = node.windows.iter().filter(|w| w.on).cloned().collect();
        (!windows.is_empty()).then_some(Self {
            windows,
            output: node.key,
        })
    }

    pub fn shaped(&self, [w, h]: [f64; 2]) -> Vec<Shaped> {
        let mut order: Vec<&GradeWindow> = self.windows.iter().collect();
        order.sort_by_key(|w| joined_first(w));
        let groups: Vec<(bool, u8)> = order.iter().map(|w| joined_first(w)).collect();
        (order.iter().enumerate())
            .map(|(i, window)| {
                let mut s = shaped(window, [w, h]);
                s.starts = i == 0 || groups[i - 1] != groups[i];
                s.ends = i + 1 == groups.len() || groups[i + 1] != groups[i];
                s
            })
            .collect()
    }
}

pub fn linear_scales(size: f64, aspect: f64) -> [f64; 2] {
    let k = linear_scale(size);
    aspect_shrink(aspect).map(|s| match s * LINEAR_MOST > k {
        true => k / s,
        false => LINEAR_MOST,
    })
}

fn pivot(points: &[[f64; 2]; 4], [w, h]: [f64; 2]) -> [f64; 2] {
    let n = points.len() as f64;
    let [sx, sy] = (points.iter()).fold([0.0, 0.0], |[x, y], [px, py]| [x + px, y + py]);
    [sx * w / n, sy * h / n]
}

pub fn shaped(window: &GradeWindow, [w, h]: [f64; 2]) -> Shaped {
    let origin = origin(window, [w, h]);
    let turned = match &window.shape {
        WindowShape::Linear { points, .. } => pivot(points, [w, h]),
        _ => [0.0, 0.0],
    };
    let middle = [origin[0] + turned[0], origin[1] - turned[1]];
    let (sin, cos) = window.rotate.to_radians().sin_cos();
    let [cos, sin] = [cos, sin].map(|v| if v.abs() < TRIG_ZERO { 0.0 } else { v });
    let outline = match &window.shape {
        WindowShape::Circle { soft } => {
            let k = circle_scale(window.size);
            Outline::Circle {
                radius: CIRCLE_RADIUS * w * k,
                soft: SOFT_STEP * soft * k,
                shrink: aspect_shrink(window.aspect),
            }
        }
        WindowShape::Linear { soft, points } => Outline::Linear {
            corners: points.map(|[px, py]| [px * w - turned[0], py * h - turned[1]]),
            scale: linear_scales(window.size, window.aspect),
            soft: soft.map(|s| SOFT_STEP * s),
        },
        WindowShape::Polygon {
            points,
            inside,
            outside,
        } => {
            let k = linear_scale(window.size);
            let diagonal = w.hypot(h);
            Outline::Polygon {
                points: (points.iter())
                    .map(|[px, py]| {
                        let (u, v) = (px * w * k, py * h * k);
                        let (x, y) = (u * cos + v * sin, v * cos - u * sin);
                        [middle[0] + x, middle[1] - y].map(|v| (v + 0.5 + EVEN).floor())
                    })
                    .collect(),
                inside: inside * POLYGON_SOFT_STEP * diagonal,
                outside: outside * POLYGON_SOFT_STEP * diagonal,
            }
        }
        WindowShape::Curve {
            points,
            soft,
            inside,
            outside,
            inner,
            outer,
        } => {
            let k = linear_scale(window.size);
            let placed = |[px, py]: [f64; 2]| {
                let (u, v) = (px * w * k, py * h * k);
                let (x, y) = (u * cos + v * sin, v * cos - u * sin);
                [middle[0] + x, middle[1] - y]
            };
            let controls: Vec<CurvePoint> = points.iter().map(|p| p.map(placed)).collect();
            let path = flattened(&controls);
            let bends = bends(&controls);
            let edge = |moves: &Option<Vec<[f64; 2]>>, by: f64| match moves {
                Some(m) if m.len() == points.len() => {
                    let moved = |(p, d): (&CurvePoint, &[f64; 2])| {
                        p.map(|[x, y]| placed([x + d[0], y + d[1]]))
                    };
                    let edge: Vec<CurvePoint> = points.iter().zip(m).map(moved).collect();
                    let at = edge.iter().map(|c| c[1]).collect();
                    (flattened(&edge), at)
                }
                _ => {
                    let at = (controls.iter().zip(&bends))
                        .map(|(c, b)| [c[1][0] + by * b[0], c[1][1] + by * b[1]])
                        .collect();
                    (mitred(&path, by), at)
                }
            };
            let (inner, inner_at): (_, Vec<[f64; 2]>) =
                edge(inner, -inside * CURVE_INSIDE_STEP * w);
            let (outer, outer_at): (_, Vec<[f64; 2]>) =
                edge(outer, outside * CURVE_OUTSIDE_STEP * w);
            Outline::Curve {
                inner,
                outer,
                spread: inner_at
                    .into_iter()
                    .zip(outer_at)
                    .map(|(a, b)| [a, b])
                    .collect(),
                bends,
                path,
                controls,
                blur: soft * CURVE_BLUR_STEP * h,
            }
        }
        WindowShape::Gradient { soft } => Outline::Gradient {
            soft: GRADIENT_SOFT_STEP * soft,
        },
    };
    Shaped {
        outline,
        middle,
        turn: [cos, sin],
        invert: window.invert,
        mask: window.mask,
        opacity: window.opacity / 100.0,
        starts: true,
        ends: true,
    }
}

fn split(c: [[f64; 2]; 4]) -> ([[f64; 2]; 4], [[f64; 2]; 4]) {
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let (ab, bc, cd) = (mid(c[0], c[1]), mid(c[1], c[2]), mid(c[2], c[3]));
    let (abc, bcd) = (mid(ab, bc), mid(bc, cd));
    let m = mid(abc, bcd);
    ([c[0], ab, abc, m], [m, bcd, cd, c[3]])
}

fn off_chord(p: [f64; 2], a: [f64; 2], d: [f64; 2]) -> f64 {
    let (ex, ey) = (d[0] - a[0], d[1] - a[1]);
    let (px, py) = (p[0] - a[0], p[1] - a[1]);
    let long = ex.hypot(ey);
    match long < SAME_POINT {
        true => px.hypot(py),
        false => (px * ey - py * ex).abs() / long,
    }
}

fn flatten_into(c: [[f64; 2]; 4], depth: u32, out: &mut Vec<[f64; 2]>) {
    let flat = off_chord(c[1], c[0], c[3]).max(off_chord(c[2], c[0], c[3])) <= FLAT_ERROR;
    if flat || depth == 0 {
        out.push(c[3]);
        return;
    }
    let (a, b) = split(c);
    flatten_into(a, depth - 1, out);
    flatten_into(b, depth - 1, out);
}

pub fn traced(controls: &[CurvePoint], segments: usize) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = controls.first().map(|c| vec![c[1]]).unwrap_or_default();
    for (i, [_, p, after]) in controls.iter().enumerate().take(segments) {
        let [before, q, _] = controls[(i + 1) % controls.len()];
        flatten_into([*p, *after, before, q], FLAT_DEPTH, &mut out);
    }
    out.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < SAME_POINT);
    out
}

pub fn flattened(controls: &[CurvePoint]) -> Vec<[f64; 2]> {
    let mut out = traced(controls, controls.len());
    while out.len() > 1 && {
        let (a, b) = (out[0], out[out.len() - 1]);
        (a[0] - b[0]).hypot(a[1] - b[1]) < SAME_POINT
    } {
        out.pop();
    }
    out
}

fn area(path: &[[f64; 2]]) -> f64 {
    (0..path.len())
        .map(|i| {
            let (a, b) = (path[i], path[(i + 1) % path.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

pub fn bends(controls: &[CurvePoint]) -> Vec<[f64; 2]> {
    let n = controls.len();
    if n < 2 {
        return vec![[0.0; 2]; n];
    }
    let path = flattened(controls);
    let turn = if area(&path) < 0.0 { -1.0 } else { 1.0 };
    let apart = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) >= SAME_POINT;
    let normal = |[ex, ey]: [f64; 2]| {
        let long = ex.hypot(ey);
        match long < SAME_POINT {
            true => [0.0, 0.0],
            false => [turn * ey / long, -turn * ex / long],
        }
    };
    (0..n)
        .map(|i| {
            let (before, c, after) = (
                controls[(i + n - 1) % n],
                controls[i],
                controls[(i + 1) % n],
            );
            let p = c[1];
            let back = [c[0], before[2], before[1]]
                .into_iter()
                .find(|q| apart(*q, p));
            let ahead = [c[2], after[0], after[1]]
                .into_iter()
                .find(|q| apart(*q, p));
            let a = back.map_or([0.0; 2], |q| normal([p[0] - q[0], p[1] - q[1]]));
            let b = ahead.map_or([0.0; 2], |q| normal([q[0] - p[0], q[1] - p[1]]));
            let along = 1.0 + a[0] * b[0] + a[1] * b[1];
            match along > 2.0 / (MITER_MOST * MITER_MOST) {
                true => [(a[0] + b[0]) / along, (a[1] + b[1]) / along],
                false => b,
            }
        })
        .collect()
}

pub fn mitred(path: &[[f64; 2]], by: f64) -> Vec<[f64; 2]> {
    let n = path.len();
    if n < 3 || by == 0.0 {
        return path.to_vec();
    }
    let turn = if area(path) < 0.0 { -1.0 } else { 1.0 };
    let normal = |a: [f64; 2], b: [f64; 2]| {
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let long = ex.hypot(ey);
        [turn * ey / long, -turn * ex / long]
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (before, at, after) = (path[(i + n - 1) % n], path[i], path[(i + 1) % n]);
        let (a, b) = (normal(before, at), normal(at, after));
        let along = 1.0 + a[0] * b[0] + a[1] * b[1];
        match along > 2.0 / (MITER_MOST * MITER_MOST) {
            true => out.push([
                at[0] + by * (a[0] + b[0]) / along,
                at[1] + by * (a[1] + b[1]) / along,
            ]),
            false => out.extend([a, b].map(|m| [at[0] + by * m[0], at[1] + by * m[1]])),
        }
    }
    out
}

pub fn texel_map(back: &[f64; 8], [w, h]: [f64; 2]) -> [f64; 9] {
    let [a, b, e, c, d, f, g, k] = *back;
    [
        w * a,
        w * b,
        w * (e - 0.5 * (a + b)),
        h * c,
        h * d,
        h * (f - 0.5 * (c + d)),
        g,
        k,
        1.0 - 0.5 * (g + k),
    ]
}
