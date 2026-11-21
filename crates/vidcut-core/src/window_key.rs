use crate::model::{GradeNode, GradeWindow, KeyOutput, WindowShape};

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
        WindowShape::Gradient { .. } => 3,
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

fn linear_scales(size: f64, aspect: f64) -> [f64; 2] {
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
