use crate::model::{GradeNode, GradeWindow, KeyOutput, WindowShape};

pub const PAN_STEP: f64 = 81.92;
pub const SOFT_STEP: f64 = 16.0;
pub const GRADIENT_SOFT_STEP: f64 = 100.0;
pub const POLYGON_SOFT_STEP: f64 = 1.0 / 25.0;
pub const CIRCLE_RADIUS: f64 = 0.15;
pub const LINEAR_HALF: f64 = 0.2;
pub const LINEAR_MOST: f64 = 4.0;
pub const FAR_OUTSIDE: f64 = 1000.0;
pub const EVEN: f64 = 1e-6;
pub const TRIG_ZERO: f64 = 1e-12;

#[derive(Debug, Clone, PartialEq)]
pub enum Outline {
    Circle {
        radius: f64,
        soft: f64,
        axes: [f64; 2],
    },
    Linear {
        half: [f64; 2],
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
        true => 0.002 + 0.998 * size / 50.0,
        false => 1.0 + 0.08 * (size - 50.0),
    }
}

pub fn aspect_axes(aspect: f64) -> [f64; 2] {
    [
        if aspect > 50.0 {
            50.0 / (100.0 - aspect)
        } else {
            1.0
        },
        if aspect < 50.0 { 50.0 / aspect } else { 1.0 },
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

fn shaped(window: &GradeWindow, [w, h]: [f64; 2]) -> Shaped {
    let middle = [
        w / 2.0 + (window.pan - 50.0) * PAN_STEP,
        h / 2.0 - (window.tilt - 50.0) * PAN_STEP,
    ];
    let (sin, cos) = window.rotate.to_radians().sin_cos();
    let [cos, sin] = [cos, sin].map(|v| if v.abs() < TRIG_ZERO { 0.0 } else { v });
    let outline = match &window.shape {
        WindowShape::Circle { soft } => {
            let k = circle_scale(window.size);
            Outline::Circle {
                radius: CIRCLE_RADIUS * w * k,
                soft: SOFT_STEP * soft * k,
                axes: aspect_axes(window.aspect),
            }
        }
        WindowShape::Linear { soft } => {
            let k = linear_scale(window.size);
            let [ax, ay] = aspect_axes(window.aspect).map(|a| (k * a).min(LINEAR_MOST));
            let axis = [ax, ay, ax, ay];
            Outline::Linear {
                half: [LINEAR_HALF * w * ax, LINEAR_HALF * h * ay],
                soft: std::array::from_fn(|i| SOFT_STEP * soft[i] * axis[i]),
            }
        }
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

fn inverse(m: [f64; 9]) -> [f64; 9] {
    let [a, b, c, d, e, f, g, h, i] = m;
    let (x, y, z) = (e * i - f * h, f * g - d * i, d * h - e * g);
    let det = a * x + b * y + c * z;
    [
        x / det,
        (c * h - b * i) / det,
        (b * f - c * e) / det,
        y / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        z / det,
        (b * g - a * h) / det,
        (a * e - b * d) / det,
    ]
}

fn times(m: [f64; 9], n: [f64; 9]) -> [f64; 9] {
    std::array::from_fn(|k| {
        let (r, c) = (k / 3, k % 3);
        (0..3).map(|j| m[3 * r + j] * n[3 * j + c]).sum()
    })
}

pub fn texel_map(
    back: &[f64; 8],
    texture: [f64; 2],
    render: [f64; 2],
    frame: [f64; 2],
) -> [f64; 9] {
    let [a, b, e, c, d, f, g, h] = *back;
    let ahead = inverse([a, b, e, c, d, f, g, h, 1.0]);
    let to_uv = [
        1.0 / texture[0],
        0.0,
        0.0,
        0.0,
        1.0 / texture[1],
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    let (sx, sy) = (frame[0] / render[0], frame[1] / render[1]);
    let to_pixels = [sx, 0.0, -0.5, 0.0, sy, -0.5, 0.0, 0.0, 1.0];
    times(to_pixels, times(ahead, to_uv))
}
