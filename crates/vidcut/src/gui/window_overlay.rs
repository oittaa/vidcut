use std::f64::consts::TAU;

use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Response, Shape, Stroke, Vec2};
use vidcut_core::model::{CurvePoint, Grade, GradeWindow, WindowShape};
use vidcut_core::ops::grade::GradeOf;
use vidcut_core::ops::grade_windows::{POLYGON_LEAST, POLYGON_MOST};
use vidcut_core::place::{ahead, apply};
use vidcut_core::request::{GradePalette, Page};
use vidcut_core::window_key::{
    CIRCLE_RADIUS, GRADIENT_SOFT_STEP, Outline, PAN_STEP, SOFT_STEP, Shaped, circle_scale,
    circle_size, linear_scale, origin, shaped, traced,
};

use super::grade_windows::{PERCENT, TURN};
use super::workspace::Workspace;

const GRIP: f32 = 9.0;
const HANDLE: f32 = 4.0;
const DOT: f32 = 2.5;
const SOFT_GAP: f64 = 12.0;
const TURN_REACH: f64 = 0.125;
const GRADIENT_REACH: f64 = 1.0 / 6.0;
const ROUND: usize = 96;
const FAR: f64 = 1e5;
const SPLIT_STEPS: usize = 64;
const DASH: f32 = 4.0;
const DASH_GAP: f32 = 3.0;
const BOUND_GRIP: f32 = 3.5;
const BOUND_REACH: f32 = 6.0;
const BOUND_AT: [[f64; 2]; 8] = [
    [0.0, 0.0],
    [0.5, 0.0],
    [1.0, 0.0],
    [1.0, 0.5],
    [1.0, 1.0],
    [0.5, 1.0],
    [0.0, 1.0],
    [0.0, 0.5],
];
const SELECTED: Color32 = Color32::WHITE;
const RIM: Color32 = Color32::from_rgb(60, 130, 235);
const SOFTNESS: Color32 = Color32::from_rgb(235, 60, 225);
const OTHER: Color32 = Color32::from_rgb(200, 200, 200);
const OTHER_HANDLE: Color32 = Color32::from_rgb(80, 80, 80);
const OTHER_SOFT: Color32 = Color32::from_rgb(140, 140, 140);
const GUIDE: Color32 = Color32::from_rgb(150, 150, 150);

pub(super) struct Framing {
    pub(super) windows: [f64; 8],
    pub(super) frame: [f64; 2],
    pub(super) origin: Pos2,
    pub(super) scale: Vec2,
}

impl Framing {
    fn screen(&self, [qx, qy]: [f64; 2]) -> Pos2 {
        let [w, h] = self.frame;
        let [x, y] = ahead(&self.windows, [qx / w, qy / h]);
        let at = egui::vec2(
            (x + 0.5) as f32 * self.scale.x,
            (y + 0.5) as f32 * self.scale.y,
        );
        self.origin + at
    }

    fn window(&self, p: Pos2) -> [f64; 2] {
        let x = f64::from((p.x - self.origin.x) / self.scale.x) - 0.5;
        let y = f64::from((p.y - self.origin.y) / self.scale.y) - 0.5;
        let [u, v] = apply(&self.windows, [x, y]);
        [u * self.frame[0], v * self.frame[1]]
    }

    fn per_screen(&self, at: [f64; 2]) -> f64 {
        let a = self.screen(at);
        let b = self.screen([at[0] + 1.0, at[1]]);
        1.0 / f64::from((b - a).length()).max(1e-9)
    }
}

#[derive(Debug, Clone, Copy)]
struct Local {
    middle: [f64; 2],
    turn: [f64; 2],
}

impl Local {
    fn of(s: &Shaped) -> Self {
        Self {
            middle: s.middle,
            turn: s.turn,
        }
    }

    fn frame(&self, [u, v]: [f64; 2]) -> [f64; 2] {
        let ([mx, my], [c, s]) = (self.middle, self.turn);
        [mx + u * c + v * s, my + u * s - v * c]
    }

    fn local(&self, [qx, qy]: [f64; 2]) -> [f64; 2] {
        let ([mx, my], [c, s]) = (self.middle, self.turn);
        let (x, y) = (qx - mx, my - qy);
        [x * c - y * s, x * s + y * c]
    }

    fn bearing(&self, [qx, qy]: [f64; 2]) -> f64 {
        (qx - self.middle[0])
            .atan2(self.middle[1] - qy)
            .to_degrees()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Move,
    Turn,
    Size,
    Aspect(usize),
    Soft(usize),
    Side(usize),
    Corner(usize),
    Point(usize),
    Spread(usize, usize),
    Tangent(usize, usize, bool),
    Pull(usize),
    Arrow,
    Band,
    Picked,
    Bound(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    Handle,
    Softness,
    Middle,
    Tangent,
    Arrow,
}

type Placed = (Grip, Look, [f64; 2]);

struct Drawn {
    outline: Vec<[f64; 2]>,
    closed: bool,
    guides: Vec<Vec<[f64; 2]>>,
    grips: Vec<Placed>,
}

pub(super) struct Drawing {
    of: GradeOf,
    node: u32,
    index: usize,
}

pub(super) struct WindowGrab {
    of: GradeOf,
    node: u32,
    index: usize,
    grip: Grip,
    grade: Grade,
    from: [f64; 2],
}

pub(super) struct PointPick {
    of: GradeOf,
    node: u32,
    index: usize,
    points: Vec<usize>,
    bound: Option<[[f64; 2]; 2]>,
    held: Option<[[f64; 2]; 2]>,
}

fn ordered([a, b]: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    [
        [a[0].min(b[0]), a[1].min(b[1])],
        [a[0].max(b[0]), a[1].max(b[1])],
    ]
}

fn bound_point([lo, hi]: [[f64; 2]; 2], a: [f64; 2]) -> [f64; 2] {
    [
        lo[0] + (hi[0] - lo[0]) * a[0],
        lo[1] + (hi[1] - lo[1]) * a[1],
    ]
}

fn bound_map(
    grip: Grip,
    b: [[f64; 2]; 2],
    (from, to): ([f64; 2], [f64; 2]),
) -> impl Fn([f64; 2]) -> [f64; 2] {
    let by = [to[0] - from[0], to[1] - from[1]];
    let (anchor, k, shift) = match grip {
        Grip::Bound(h) => {
            let a = BOUND_AT[h % BOUND_AT.len()];
            let anchor = bound_point(b, [1.0 - a[0], 1.0 - a[1]]);
            let handle = bound_point(b, a);
            let ratio: [f64; 2] = std::array::from_fn(|j| {
                let long = handle[j] - anchor[j];
                match a[j] == 0.5 || long.abs() < 1e-9 {
                    true => 1.0,
                    false => (long + by[j]) / long,
                }
            });
            let even = (ratio.into_iter())
                .min_by(|x, y| x.abs().total_cmp(&y.abs()))
                .unwrap_or(1.0);
            let k = match a[0] == 0.5 || a[1] == 0.5 {
                true => ratio,
                false => [even; 2],
            };
            (anchor, k, [0.0, 0.0])
        }
        _ => ([0.0, 0.0], [1.0, 1.0], by),
    };
    move |q: [f64; 2]| {
        [
            anchor[0] + (q[0] - anchor[0]) * k[0] + shift[0],
            anchor[1] + (q[1] - anchor[1]) * k[1] + shift[1],
        ]
    }
}

fn picks_moved(
    w: &GradeWindow,
    picks: &[usize],
    grip: Grip,
    moved: ([f64; 2], [f64; 2]),
    (frame, held): ([f64; 2], [[f64; 2]; 2]),
) -> GradeWindow {
    let s = shaped(w, frame);
    let mut n = w.clone();
    let (Outline::Curve { controls, .. }, WindowShape::Curve { points, .. }) =
        (&s.outline, &mut n.shape)
    else {
        return n;
    };
    let map = bound_map(grip, held, moved);
    for &i in picks {
        if let (Some(c), Some(t)) = (controls.get(i), points.get_mut(i)) {
            *t = c.map(|q| model_point(w, &s, frame, map(q)));
        }
    }
    n
}

fn picked_in(controls: &[CurvePoint], framing: &Framing, band: egui::Rect) -> Vec<usize> {
    (controls.iter().enumerate())
        .filter(|(_, c)| band.contains(framing.screen(c[1])))
        .map(|(i, _)| i)
        .collect()
}

fn dashed(painter: &egui::Painter, corners: [Pos2; 4], color: Color32) {
    let ring = [corners[0], corners[1], corners[2], corners[3], corners[0]];
    painter.extend(Shape::dashed_line(
        &ring,
        Stroke::new(1.0, color),
        DASH,
        DASH_GAP,
    ));
}

fn paint_picks(
    painter: &egui::Painter,
    framing: &Framing,
    (controls, grips): (&[CurvePoint], &[Placed]),
    (picks, bound): (&[usize], Option<[[f64; 2]; 2]>),
) {
    for (g, _, q) in grips {
        if let Grip::Spread(i, _) = g
            && picks.contains(i)
        {
            painter.circle_filled(framing.screen(*q), HANDLE, RIM);
        }
    }
    for c in picks.iter().filter_map(|i| controls.get(*i)) {
        painter.circle(
            framing.screen(c[1]),
            HANDLE,
            RIM,
            Stroke::new(1.5, SELECTED),
        );
    }
    let Some(b) = bound else {
        return;
    };
    let corner = |a: [f64; 2]| framing.screen(bound_point(b, a));
    let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(corner);
    dashed(painter, corners, SELECTED);
    for a in BOUND_AT {
        let at = corner(a);
        let square = egui::Rect::from_center_size(at, Vec2::splat(2.0 * BOUND_GRIP));
        painter.rect(
            square,
            0.0,
            SELECTED,
            Stroke::new(1.0, RIM),
            egui::StrokeKind::Middle,
        );
    }
}

fn bound_hit(
    framing: &Framing,
    controls: &[CurvePoint],
    (picks, bound): (&[usize], Option<[[f64; 2]; 2]>),
    p: Pos2,
) -> Option<Grip> {
    if let Some(b) = bound {
        let near = (BOUND_AT.iter().enumerate())
            .map(|(h, a)| (h, framing.screen(bound_point(b, *a)).distance(p)))
            .filter(|(_, d)| *d <= BOUND_REACH)
            .min_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((h, _)) = near {
            return Some(Grip::Bound(h));
        }
    }
    (picks.iter().filter_map(|i| controls.get(*i)))
        .any(|c| framing.screen(c[1]).distance(p) <= GRIP)
        .then_some(Grip::Picked)
}

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn inward(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let e = [b[0] - a[0], b[1] - a[1]];
    let n = e[0].hypot(e[1]).max(1e-12);
    [e[1] / n, -e[0] / n]
}

fn inside_of(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    let n = inward(a, b);
    (p[0] - a[0]) * n[0] + (p[1] - a[1]) * n[1]
}

fn offset_quad(corners: &[[f64; 2]; 4], by: [f64; 4]) -> [[f64; 2]; 4] {
    let line = |i: usize| {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let n = inward(a, b);
        let p = [a[0] + n[0] * by[i], a[1] + n[1] * by[i]];
        (p, [b[0] - a[0], b[1] - a[1]])
    };
    std::array::from_fn(|i| {
        let ((p1, e1), (p2, e2)) = (line((i + 3) % 4), line(i));
        let d = cross(e1, e2);
        match d.abs() < 1e-12 {
            true => p2,
            false => {
                let t = cross([p2[0] - p1[0], p2[1] - p1[1]], e2) / d;
                [p1[0] + t * e1[0], p1[1] + t * e1[1]]
            }
        }
    })
}

fn holds_point(points: &[[f64; 2]], [x, y]: [f64; 2]) -> bool {
    let mut inside = false;
    let mut b = points[points.len() - 1];
    for &a in points {
        if (a[1] > y) != (b[1] > y) && x < a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            inside = !inside;
        }
        b = a;
    }
    inside
}

fn drawn(w: &GradeWindow, s: &Shaped, ([fw, fh], gap, open): ([f64; 2], f64, bool)) -> Drawn {
    let local = Local::of(s);
    let at = |uv: [f64; 2]| local.frame(uv);
    let middle = (Grip::Move, Look::Middle, s.middle);
    let turn = |k: f64| {
        let tip = at([0.0, TURN_REACH * fh * k]);
        ((Grip::Turn, Look::Handle, tip), vec![s.middle, tip])
    };
    match &s.outline {
        Outline::Circle {
            radius,
            soft,
            shrink,
        } => {
            let [sx, sy] = shrink.map(|v| v.max(1.0 / FAR));
            let ellipse = |r: f64| -> Vec<[f64; 2]> {
                (0..ROUND)
                    .map(|i| {
                        let t = TAU * i as f64 / ROUND as f64;
                        at([r / sx * t.cos(), r / sy * t.sin()])
                    })
                    .collect()
            };
            let (r, sf) = (*radius, *soft);
            let (a, b) = (r / sx, r / sy);
            let boxed = [[-a, -b], [-a, b], [a, b], [a, -b]].map(at);
            let mut guides = vec![[&boxed[..], &boxed[..1]].concat()];
            if sf > 0.0 {
                for o in [ellipse((r - sf).max(0.0)), ellipse(r + sf)] {
                    guides.push([&o[..], &o[..1]].concat());
                }
            }
            let (oa, ob) = (((r + sf) / sx).max(a + gap), ((r + sf) / sy).max(b + gap));
            let (tip, stem) = turn(circle_scale(w.size));
            guides.push(stem);
            let sides = [[-a, 0.0], [0.0, b], [a, 0.0], [0.0, -b]];
            let softs = [[-oa, 0.0], [0.0, ob], [oa, 0.0], [0.0, -ob]];
            let mut grips: Vec<_> = boxed
                .iter()
                .map(|c| (Grip::Size, Look::Handle, *c))
                .collect();
            grips.extend((0..4).map(|i| (Grip::Aspect(i), Look::Handle, at(sides[i]))));
            grips.extend((0..4).map(|i| (Grip::Soft(i), Look::Softness, at(softs[i]))));
            grips.extend([tip, middle]);
            Drawn {
                outline: ellipse(r),
                closed: true,
                guides,
                grips,
            }
        }
        Outline::Linear {
            corners,
            scale: [sx, sy],
            soft,
        } => {
            let scaled = |[x, y]: [f64; 2]| at([x * sx, y * sy]);
            let outline: Vec<[f64; 2]> = corners.iter().map(|c| scaled(*c)).collect();
            let mut guides = Vec::new();
            for sign in [-1.0, 1.0] {
                let quad = offset_quad(corners, soft.map(|v| v * sign));
                for i in (0..4).filter(|i| soft[*i] > 0.0) {
                    guides.push(vec![scaled(quad[i]), scaled(quad[(i + 1) % 4])]);
                }
            }
            let (tip, stem) = turn(linear_scale(w.size));
            guides.push(stem);
            let mut grips: Vec<_> = (0..4)
                .map(|i| (Grip::Corner(i), Look::Handle, outline[i]))
                .collect();
            grips.extend((0..4).map(|i| {
                (
                    Grip::Side(i),
                    Look::Handle,
                    mid(outline[i], outline[(i + 1) % 4]),
                )
            }));
            grips.extend((0..4).map(|i| {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                let n = inward(a, b);
                let side = mid(a, b);
                let away = scaled([side[0] - n[0], side[1] - n[1]]);
                let on = scaled(side);
                let out = [away[0] - on[0], away[1] - on[1]];
                let long = out[0].hypot(out[1]).max(1e-12);
                let far = (soft[i] * long).max(gap);
                let spot = [on[0] + out[0] / long * far, on[1] + out[1] / long * far];
                (Grip::Soft(i), Look::Softness, spot)
            }));
            grips.extend([tip, middle]);
            Drawn {
                outline,
                closed: true,
                guides,
                grips,
            }
        }
        Outline::Polygon { points, .. } => {
            let (tip, stem) = turn(linear_scale(w.size));
            let mut grips: Vec<_> = (points.iter().enumerate())
                .map(|(i, p)| (Grip::Point(i), Look::Handle, *p))
                .collect();
            grips.extend([tip, middle]);
            Drawn {
                outline: points.clone(),
                closed: true,
                guides: vec![stem],
                grips,
            }
        }
        Outline::Curve {
            controls,
            path,
            inner,
            outer,
            spread,
            bends,
            ..
        } => {
            let ring = |o: &Vec<[f64; 2]>| [&o[..], &o[..1]].concat();
            let mut guides: Vec<Vec<[f64; 2]>> = ([inner, outer].into_iter())
                .filter(|o| *o != path && !o.is_empty())
                .map(ring)
                .collect();
            let mut grips = Vec::new();
            for (i, c) in controls.iter().enumerate() {
                grips.push((Grip::Point(i), Look::Handle, c[1]));
                for side in [0, 2].into_iter().filter(|side| c[*side] != c[1]) {
                    guides.push(vec![c[1], c[side]]);
                    grips.push((Grip::Tangent(i, side, false), Look::Tangent, c[side]));
                }
            }
            if !open {
                for (i, (c, (at, bend))) in
                    controls.iter().zip(spread.iter().zip(bends)).enumerate()
                {
                    let long = bend[0].hypot(bend[1]);
                    for (side, sign) in [(0, -1.0), (1, 1.0)] {
                        let q = at[side];
                        let spot = match (q[0] - c[1][0]).hypot(q[1] - c[1][1]) < gap && long > 0.0
                        {
                            true => [
                                c[1][0] + sign * gap * bend[0] / long,
                                c[1][1] + sign * gap * bend[1] / long,
                            ],
                            false => q,
                        };
                        grips.push((Grip::Spread(i, side), Look::Softness, spot));
                    }
                }
            }
            if !controls.is_empty() {
                grips.push(middle);
            }
            Drawn {
                outline: match open {
                    true => traced(controls, controls.len().saturating_sub(1)),
                    false => path.clone(),
                },
                closed: !open,
                guides,
                grips,
            }
        }
        Outline::Gradient { soft } => {
            let half = GRADIENT_REACH * fw;
            let tip = at([0.0, -soft.max(gap)]);
            Drawn {
                outline: vec![at([-half, 0.0]), at([half, 0.0])],
                closed: false,
                guides: vec![vec![s.middle, tip]],
                grips: vec![(Grip::Arrow, Look::Arrow, tip), middle],
            }
        }
    }
}

fn holds(s: &Shaped, q: [f64; 2]) -> bool {
    let local = Local::of(s);
    match &s.outline {
        Outline::Circle { radius, shrink, .. } => {
            let [u, v] = local.local(q);
            (u * shrink[0]).hypot(v * shrink[1]) <= *radius
        }
        Outline::Linear { corners, scale, .. } => {
            let [u, v] = local.local(q);
            holds_point(corners, [u / scale[0], v / scale[1]])
        }
        Outline::Polygon { points, .. } => points.len() >= POLYGON_LEAST && holds_point(points, q),
        Outline::Curve { path, .. } => path.len() >= POLYGON_LEAST && holds_point(path, q),
        Outline::Gradient { .. } => false,
    }
}

fn wrapped(r: f64) -> f64 {
    ((r + 180.0).rem_euclid(360.0) - 180.0).clamp(TURN.0, TURN.1)
}

fn span(v: f64) -> f64 {
    v.clamp(PERCENT.0, PERCENT.1)
}

fn moved_corners(
    w: &GradeWindow,
    s: &Shaped,
    frame: [f64; 2],
    (moved, by): (&[usize], [f64; 2]),
) -> Option<[[f64; 2]; 4]> {
    let Outline::Linear { corners, scale, .. } = &s.outline else {
        return None;
    };
    let local = Local::of(s);
    let shown: [[f64; 2]; 4] = std::array::from_fn(|j| {
        let q = local.frame([corners[j][0] * scale[0], corners[j][1] * scale[1]]);
        match moved.contains(&j) {
            true => [q[0] + by[0], q[1] + by[1]],
            false => q,
        }
    });
    let middle = shown
        .iter()
        .fold([0.0, 0.0], |[x, y], q| [x + q[0] / 4.0, y + q[1] / 4.0]);
    let then = Local {
        middle,
        turn: s.turn,
    };
    let [ox, oy] = origin(w, frame);
    let pivot = [middle[0] - ox, oy - middle[1]];
    Some(shown.map(|q| {
        let [u, v] = then.local(q);
        [
            (pivot[0] + u / scale[0]) / frame[0],
            (pivot[1] + v / scale[1]) / frame[1],
        ]
    }))
}

fn model_point(w: &GradeWindow, s: &Shaped, frame: [f64; 2], q: [f64; 2]) -> [f64; 2] {
    let k = linear_scale(w.size);
    let [u, v] = Local::of(s).local(q);
    [u / (frame[0] * k), v / (frame[1] * k)]
}

fn turned_with(a: [f64; 2], (from, to): ([f64; 2], [f64; 2]), o: [f64; 2]) -> [f64; 2] {
    let (f, t) = (
        [from[0] - a[0], from[1] - a[1]],
        [to[0] - a[0], to[1] - a[1]],
    );
    let long = f[0] * f[0] + f[1] * f[1];
    if long < 1e-12 {
        return o;
    }
    let z = [
        (t[0] * f[0] + t[1] * f[1]) / long,
        (t[1] * f[0] - t[0] * f[1]) / long,
    ];
    let r = [o[0] - a[0], o[1] - a[1]];
    [
        a[0] + z[0] * r[0] - z[1] * r[1],
        a[1] + z[0] * r[1] + z[1] * r[0],
    ]
}

fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn segment(controls: &[CurvePoint], i: usize) -> [[f64; 2]; 4] {
    let (a, b) = (controls[i], controls[(i + 1) % controls.len()]);
    [a[1], a[2], b[0], b[1]]
}

fn split_at(c: [[f64; 2]; 4], t: f64) -> ([[f64; 2]; 4], [[f64; 2]; 4]) {
    let (ab, bc, cd) = (
        lerp(c[0], c[1], t),
        lerp(c[1], c[2], t),
        lerp(c[2], c[3], t),
    );
    let (abc, bcd) = (lerp(ab, bc, t), lerp(bc, cd, t));
    let m = lerp(abc, bcd, t);
    ([c[0], ab, abc, m], [m, bcd, cd, c[3]])
}

fn bezier_at(c: [[f64; 2]; 4], t: f64) -> [f64; 2] {
    split_at(c, t).0[3]
}

fn curve_split(controls: &[CurvePoint], framing: &Framing, p: Pos2) -> Option<(usize, f64)> {
    (0..controls.len())
        .flat_map(|i| {
            let c = segment(controls, i);
            (1..SPLIT_STEPS).map(move |k| {
                let t = k as f64 / SPLIT_STEPS as f64;
                (i, t, framing.screen(bezier_at(c, t)).distance(p))
            })
        })
        .filter(|(_, _, d)| *d <= GRIP)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, t, _)| (i, t))
}

fn curve_inserted(
    w: &GradeWindow,
    s: &Shaped,
    frame: [f64; 2],
    (i, t): (usize, f64),
) -> Option<WindowShape> {
    let (Outline::Curve { controls, .. }, WindowShape::Curve { points, .. }) =
        (&s.outline, &w.shape)
    else {
        return None;
    };
    let mut shape = w.shape.clone();
    let n = controls.len();
    let (left, right) = split_at(segment(controls, i), t);
    let model = |q: [f64; 2]| model_point(w, s, frame, q);
    let mut next = points.clone();
    next[i][2] = model(left[1]);
    let after = (i + 1) % n;
    next[after][0] = model(right[2]);
    next.insert(i + 1, [left[2], left[3], right[1]].map(model));
    if let WindowShape::Curve {
        points,
        inner,
        outer,
        ..
    } = &mut shape
    {
        *points = next;
        for moves in [inner, outer].into_iter().flatten() {
            let (a, b) = (moves[i % n], moves[after % n]);
            moves.insert(i + 1, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        }
    }
    Some(shape)
}

fn outward(i: usize, [u, v]: [f64; 2]) -> f64 {
    match i % 4 {
        0 => -u,
        1 => v,
        2 => u,
        _ => -v,
    }
}

fn dragged(
    w: &GradeWindow,
    grip: Grip,
    (from, to): ([f64; 2], [f64; 2]),
    frame: [f64; 2],
) -> GradeWindow {
    let s = shaped(w, frame);
    let local = Local::of(&s);
    let by = [to[0] - from[0], to[1] - from[1]];
    let (lf, lt) = (local.local(from), local.local(to));
    let mut n = w.clone();
    match (grip, &s.outline, &mut n.shape) {
        (Grip::Move, ..) => {
            n.pan = span(w.pan + by[0] / PAN_STEP);
            n.tilt = span(w.tilt - by[1] / PAN_STEP);
        }
        (Grip::Turn, ..) => n.rotate = wrapped(w.rotate + local.bearing(to) - local.bearing(from)),
        (Grip::Arrow, Outline::Gradient { soft: s0 }, WindowShape::Gradient { soft }) => {
            n.rotate = wrapped(w.rotate + local.bearing(to) - local.bearing(from));
            let reach = |[u, v]: [f64; 2]| u.hypot(v);
            *soft = span((s0 + reach(lt) - reach(lf)).max(0.0) / GRADIENT_SOFT_STEP);
        }
        (Grip::Size, Outline::Circle { .. }, _) => {
            let reach = |[u, v]: [f64; 2]| u.hypot(v);
            let k = circle_scale(w.size) * reach(lt) / reach(lf).max(1e-9);
            n.size = span(circle_size(k));
        }
        (Grip::Aspect(i), Outline::Circle { radius, shrink, .. }, _) => {
            let [sx, sy] = shrink.map(|v| v.max(1.0 / FAR));
            let (mut a, mut b) = (radius / sx, radius / sy);
            match i % 2 {
                0 => a = (a + outward(i, lt) - outward(i, lf)).max(1.0),
                _ => b = (b + outward(i, lt) - outward(i, lf)).max(1.0),
            }
            n.aspect = span(match a >= b {
                true => 100.0 - 50.0 * b / a,
                false => 50.0 * a / b,
            });
            n.size = span(circle_size(a.min(b) / (CIRCLE_RADIUS * frame[0])));
        }
        (Grip::Soft(i), Outline::Circle { shrink, .. }, WindowShape::Circle { soft }) => {
            let along = |l: [f64; 2]| outward(i, l) * shrink[i % 2];
            let k = circle_scale(w.size);
            let px = (*soft * SOFT_STEP * k + along(lt) - along(lf)).max(0.0);
            *soft = span(px / (SOFT_STEP * k));
        }
        (
            Grip::Soft(i),
            Outline::Linear {
                corners,
                scale: [sx, sy],
                soft: s0,
            },
            WindowShape::Linear { soft, .. },
        ) => {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            let out = |[u, v]: [f64; 2]| -inside_of(a, b, [u / sx, v / sy]);
            soft[i] = span((s0[i] + out(lt) - out(lf)).max(0.0) / SOFT_STEP);
        }
        (Grip::Corner(i), _, WindowShape::Linear { points, .. }) => {
            *points = moved_corners(w, &s, frame, (&[i], by)).unwrap_or(*points);
        }
        (Grip::Side(i), _, WindowShape::Linear { points, .. }) => {
            let pair = [i, (i + 1) % 4];
            *points = moved_corners(w, &s, frame, (&pair, by)).unwrap_or(*points);
        }
        (Grip::Point(i), Outline::Curve { controls, .. }, WindowShape::Curve { points, .. }) => {
            if let (Some(c), Some(t)) = (controls.get(i), points.get_mut(i)) {
                *t = c.map(|q| model_point(w, &s, frame, [q[0] + by[0], q[1] + by[1]]));
            }
        }
        (
            Grip::Tangent(i, side, alone),
            Outline::Curve { controls, .. },
            WindowShape::Curve { points, .. },
        ) => {
            if let (Some(c), Some(t)) = (controls.get(i), points.get_mut(i)) {
                let mut next = *c;
                next[side] = [c[side][0] + by[0], c[side][1] + by[1]];
                if !alone {
                    next[2 - side] = turned_with(c[1], (c[side], next[side]), c[2 - side]);
                }
                *t = next.map(|q| model_point(w, &s, frame, q));
            }
        }
        (
            Grip::Spread(i, side),
            Outline::Curve { spread, .. },
            WindowShape::Curve {
                points,
                inner,
                outer,
                ..
            },
        ) => {
            let model = |q: [f64; 2], p: &CurvePoint| {
                let m = model_point(w, &s, frame, q);
                [m[0] - p[1][0], m[1] - p[1][1]]
            };
            let list = match side {
                0 => inner,
                _ => outer,
            };
            let moves = list.get_or_insert_with(|| {
                (points.iter().zip(spread.iter()))
                    .map(|(p, at)| model(at[side], p))
                    .collect()
            });
            if let (Some(m), Some(at), Some(p)) = (moves.get_mut(i), spread.get(i), points.get(i)) {
                *m = model([at[side][0] + by[0], at[side][1] + by[1]], p);
            }
        }
        (Grip::Pull(i), Outline::Curve { controls, .. }, WindowShape::Curve { points, .. }) => {
            if let (Some(c), Some(t)) = (controls.get(i), points.get_mut(i)) {
                let a = c[1];
                let next = [[2.0 * a[0] - to[0], 2.0 * a[1] - to[1]], a, to];
                *t = next.map(|q| model_point(w, &s, frame, q));
            }
        }
        (Grip::Point(i), Outline::Polygon { .. }, WindowShape::Polygon { points, .. }) => {
            if let Some(p) = points.get(i).copied() {
                let k = linear_scale(w.size);
                let q = local.frame([p[0] * frame[0] * k, p[1] * frame[1] * k]);
                points[i] = model_point(w, &s, frame, [q[0] + by[0], q[1] + by[1]]);
            }
        }
        _ => {}
    }
    n
}

fn near_edge(points: &[Pos2], p: Pos2) -> Option<usize> {
    (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            let e = b - a;
            let t = ((p - a).dot(e) / e.length_sq().max(1e-6)).clamp(0.0, 1.0);
            (i, (a + e * t).distance(p))
        })
        .filter(|(_, d)| *d <= GRIP)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

struct Seen {
    index: usize,
    shaped: Shaped,
    drawn: Drawn,
}

fn grips_at<'a>(
    seen: impl Iterator<Item = &'a Seen>,
    framing: &Framing,
    p: Pos2,
) -> Option<(usize, Grip)> {
    seen.flat_map(|s| {
        (s.drawn.grips.iter()).map(move |(g, _, q)| (s.index, *g, framing.screen(*q).distance(p)))
    })
    .filter(|(_, _, d)| *d <= GRIP)
    .min_by(|a, b| a.2.total_cmp(&b.2))
    .map(|(i, g, _)| (i, g))
}

fn target(seen: &[Seen], current: usize, framing: &Framing, p: Pos2) -> Option<(usize, Grip)> {
    let order = (seen.iter().filter(|s| s.index == current))
        .chain(seen.iter().rev().filter(|s| s.index != current));
    let grip = grips_at(order.clone(), framing, p);
    let q = framing.window(p);
    grip.or_else(|| {
        order
            .into_iter()
            .find(|s| holds(&s.shaped, q))
            .map(|s| (s.index, Grip::Move))
    })
}

fn paint(painter: &egui::Painter, framing: &Framing, seen: &Seen, current: bool) {
    let screen = |q: &[f64; 2]| framing.screen(*q);
    let line = match current {
        true => Stroke::new(1.5, SELECTED),
        false => Stroke::new(1.0, OTHER),
    };
    let guide = Stroke::new(1.0, GUIDE);
    for g in &seen.drawn.guides {
        painter.add(Shape::line(g.iter().map(screen).collect(), guide));
    }
    let outline: Vec<Pos2> = seen.drawn.outline.iter().map(screen).collect();
    painter.add(match seen.drawn.closed {
        true => Shape::closed_line(outline, line),
        false => Shape::line(outline, line),
    });
    for (_, look, q) in &seen.drawn.grips {
        let at = screen(q);
        match (look, current) {
            (Look::Handle, true) => {
                painter.circle(at, HANDLE, SELECTED, Stroke::new(1.5, RIM));
            }
            (Look::Handle, false) => {
                painter.circle_stroke(at, HANDLE, Stroke::new(1.5, OTHER_HANDLE));
            }
            (Look::Softness, true) => {
                painter.circle_filled(at, HANDLE, SOFTNESS);
            }
            (Look::Softness, false) => {
                painter.circle_filled(at, HANDLE, OTHER_SOFT);
            }
            (Look::Middle, _) => {
                painter.circle_filled(at, DOT, line.color);
            }
            (Look::Tangent, true) => {
                painter.circle_filled(at, DOT, SELECTED);
            }
            (Look::Tangent, false) => {
                painter.circle_filled(at, DOT, OTHER_HANDLE);
            }
            (Look::Arrow, _) => {
                let from = screen(&seen.shaped.middle);
                let way = (at - from).normalized();
                let side = egui::vec2(-way.y, way.x) * HANDLE;
                let fill = if current { SOFTNESS } else { OTHER_SOFT };
                let tip = at + way * HANDLE * 1.5;
                painter.add(Shape::convex_polygon(
                    vec![tip, at + side, at - side],
                    fill,
                    Stroke::NONE,
                ));
            }
        }
    }
}

fn with_window(
    grade: &Grade,
    n: u32,
    (frame, index): (i64, usize),
    change: impl FnOnce(&mut GradeWindow),
) -> Grade {
    let mut next = grade.clone();
    if let Some(d) = next.node_mut(n) {
        d.change_window_at(frame, index, |w| {
            let mut w = w.clone();
            change(&mut w);
            w
        });
    }
    next
}

fn with_shape(grade: &Grade, n: u32, at: (i64, usize), shape: WindowShape) -> Grade {
    with_window(grade, n, at, |w| w.shape = shape)
}

fn with_points(grade: &Grade, n: u32, at: (i64, usize), points: Vec<CurvePoint>) -> Grade {
    with_window(grade, n, at, |w| {
        if let WindowShape::Curve { points: to, .. } = &mut w.shape {
            *to = points;
        }
    })
}

impl Workspace {
    pub(super) fn window_controls(
        &mut self,
        ui: &egui::Ui,
        resp: &Response,
        picture: egui::Rect,
    ) -> bool {
        if !ui.input(|i| i.pointer.primary_down()) {
            self.effects_page.window_grab = None;
        }
        if self.effects_page.page != Page::Color
            || self.effects_page.palette != GradePalette::Window
        {
            self.effects_page.window_drawing = None;
            self.effects_page.window_points = None;
            return false;
        }
        let Some((of, _, stored)) = self.grade_shown() else {
            return true;
        };
        let Some(n) = self.current_grade_node(&stored) else {
            return true;
        };
        let grade = self.held_grade(&of, n).cloned().unwrap_or(stored);
        let Some(node) = grade.node(n) else {
            return true;
        };
        let Some(framing) = self.window_framing(&of, picture) else {
            return true;
        };
        let keyed = self.grade_frame(&of);
        let (windows, locked) = (node.windows_changed_at(keyed).to_vec(), node.locked);
        let current = self
            .effects_page
            .window
            .min(windows.len().saturating_sub(1));
        let frame = framing.frame;
        let curve = windows
            .get(current)
            .filter(|w| w.on)
            .and_then(|w| match &w.shape {
                WindowShape::Curve { points, .. } => Some(points.len()),
                _ => None,
            });
        let drawing = curve.is_some()
            && (self.effects_page.window_drawing.as_ref())
                .is_some_and(|d| d.of == of && d.node == n && d.index == current);
        if !drawing {
            self.effects_page.window_drawing = None;
        }
        let adding = !locked && (drawing || curve == Some(0));
        let mut seen: Vec<Seen> = (windows.iter().enumerate())
            .filter(|(_, w)| w.on)
            .map(|(index, w)| {
                let shaped = shaped(w, frame);
                let gap = SOFT_GAP * framing.per_screen(shaped.middle);
                let open = drawing && index == current;
                let drawn = drawn(w, &shaped, (frame, gap, open));
                Seen {
                    index,
                    shaped,
                    drawn,
                }
            })
            .collect();
        seen.sort_by_key(|s| s.index == current);
        let painter = ui.painter_at(resp.rect);
        for s in &seen {
            paint(&painter, &framing, s, s.index == current);
        }
        let controls: Vec<CurvePoint> = match seen.iter().find(|s| s.index == current) {
            Some(Seen {
                shaped:
                    Shaped {
                        outline: Outline::Curve { controls, .. },
                        ..
                    },
                ..
            }) if !adding => controls.clone(),
            _ => Vec::new(),
        };
        if self.effects_page.window_points.is_some()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.effects_page.window_points = None;
        }
        let (picks, bound): (Vec<usize>, _) = (self.effects_page.window_points.as_ref())
            .filter(|k| k.of == of && k.node == n && k.index == current)
            .map(|k| {
                let kept = k.points.iter().copied().filter(|i| *i < controls.len());
                (kept.collect(), k.bound)
            })
            .unwrap_or_default();
        if picks.is_empty() {
            self.effects_page.window_points = None;
        }
        let grips = (seen.iter().find(|s| s.index == current))
            .map(|s| s.drawn.grips.as_slice())
            .unwrap_or_default();
        paint_picks(&painter, &framing, (&controls, grips), (&picks, bound));
        let own = |p: Pos2| grips_at(seen.iter().filter(|s| s.index == current), &framing, p);
        let hover = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| resp.rect.contains(*p));
        if let Some(g) = &self.effects_page.window_grab
            && g.grip == Grip::Band
            && let Some(p) = hover
        {
            let band = egui::Rect::from_two_pos(framing.screen(g.from), p);
            let corners = [
                band.left_top(),
                band.right_top(),
                band.right_bottom(),
                band.left_bottom(),
            ];
            dashed(&painter, corners, SELECTED);
        }
        let grabbed = self.effects_page.window_grab.is_some();
        let on_picks = |p: Pos2| bound_hit(&framing, &controls, (&picks, bound), p);
        let hovered = match adding {
            true => hover.map(|p| own(p).map(|(_, g)| g)),
            false => hover
                .and_then(|p| {
                    (on_picks(p).map(|g| (current, g)))
                        .or_else(|| target(&seen, current, &framing, p))
                })
                .map(|(_, g)| Some(g)),
        };
        if let Some(grip) = hovered {
            ui.ctx().set_cursor_icon(match (grip, grabbed) {
                (_, true) => CursorIcon::Grabbing,
                (None, false) => CursorIcon::Crosshair,
                (Some(Grip::Move), false) => CursorIcon::Move,
                (Some(_), false) => CursorIcon::Grab,
            });
        }
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        let press = ui.input(|i| i.pointer.press_origin());
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let started =
            (press.filter(|_| resp.drag_started_by(PointerButton::Primary))).and_then(|p| {
                let band = shift && !controls.is_empty();
                (on_picks(p).or(band.then_some(Grip::Band))).map(|g| (p, g))
            });
        if let Some((p, grip)) = started {
            match grip {
                Grip::Band => self.effects_page.window_points = None,
                _ => {
                    if let Some(k) = self.effects_page.window_points.as_mut() {
                        k.held = k.bound;
                    }
                }
            }
            self.effects_page.window_grab = (!locked).then(|| WindowGrab {
                of: of.clone(),
                node: n,
                index: current,
                grip,
                grade: grade.clone(),
                from: framing.window(p),
            });
        }
        if resp.drag_started_by(PointerButton::Primary)
            && started.is_none()
            && let Some(p) = press
        {
            self.effects_page.window_points = None;
            let hit = match adding {
                true => own(p),
                false => target(&seen, current, &framing, p),
            };
            let corner = |index: usize, i: usize| {
                matches!(windows.get(index).map(|w| &w.shape),
                    Some(WindowShape::Curve { points, .. })
                        if points.get(i).is_some_and(|t| t[0] == t[1] && t[2] == t[1]))
            };
            let (index, grip, start) = match hit {
                Some((index, Grip::Point(0))) if drawing && curve.unwrap_or(0) >= 2 => {
                    self.effects_page.window_drawing = None;
                    (index, Grip::Pull(0), grade.clone())
                }
                Some((index, Grip::Point(i))) if alt && corner(index, i) => {
                    (index, Grip::Pull(i), grade.clone())
                }
                Some((index, Grip::Tangent(i, side, _))) => {
                    (index, Grip::Tangent(i, side, alt), grade.clone())
                }
                Some((index, grip)) => (index, grip, grade.clone()),
                None if adding => {
                    let w = &windows[current];
                    let point = model_point(w, &shaped(w, frame), frame, framing.window(p));
                    let mut made = 0;
                    let next = with_window(&grade, n, (keyed, current), |w| {
                        if let WindowShape::Curve { points, .. } = &mut w.shape {
                            points.push([point; 3]);
                            made = points.len() - 1;
                        }
                    });
                    self.effects_page.window_drawing = Some(Drawing {
                        of: of.clone(),
                        node: n,
                        index: current,
                    });
                    self.hold_grade(&of, n, next.clone());
                    (current, Grip::Pull(made), next)
                }
                None => (current, Grip::Move, grade.clone()),
            };
            if hit.is_some() || adding {
                self.effects_page.window = index;
                self.effects_page.window_grab = (!locked).then(|| WindowGrab {
                    of: of.clone(),
                    node: n,
                    index,
                    grip,
                    grade: start,
                    from: framing.window(p),
                });
            }
        }
        if resp.dragged_by(PointerButton::Primary)
            && let Some(to) = pointer
            && let Some(g) = &self.effects_page.window_grab
            && g.of == of
            && g.node == n
            && let Some(start) =
                (g.grade.node(n)).and_then(|d| d.windows_changed_at(keyed).get(g.index))
        {
            let moved = (g.from, framing.window(to));
            let held = (self.effects_page.window_points.as_ref()).and_then(|k| k.held);
            let next_window = match g.grip {
                Grip::Band => None,
                Grip::Picked | Grip::Bound(_) => {
                    if let (Some(held), Some(k)) = (held, self.effects_page.window_points.as_mut())
                    {
                        let map = bound_map(g.grip, held, moved);
                        k.bound = Some(ordered([map(held[0]), map(held[1])]));
                    }
                    let held = held.unwrap_or_default();
                    Some(picks_moved(start, &picks, g.grip, moved, (frame, held)))
                }
                grip => Some(dragged(start, grip, moved, frame)),
            };
            let band = (g.grip == Grip::Band)
                .then(|| egui::Rect::from_two_pos(framing.screen(g.from), to));
            let next =
                next_window.map(|next| with_window(&g.grade, n, (keyed, g.index), |w| *w = next));
            if let Some(next) = next {
                self.hold_grade(&of, n, next);
            }
            if let Some(band) = band {
                let bound = ordered([framing.window(band.min), framing.window(band.max)]);
                self.effects_page.window_points = Some(PointPick {
                    of: of.clone(),
                    node: n,
                    index: current,
                    points: picked_in(&controls, &framing, band),
                    bound: Some(bound),
                    held: Some(bound),
                });
            }
        }
        if resp.drag_stopped() {
            self.effects_page.window_grab = None;
        }
        let clicked = pointer.filter(|_| resp.clicked_by(PointerButton::Primary));
        if clicked.is_some_and(|p| on_picks(p).is_none()) {
            self.effects_page.window_points = None;
        }
        if let Some(p) = clicked.filter(|p| on_picks(*p).is_none()) {
            match adding {
                true => self.curve_click(&of, n, (&grade, current), (own(p), p, &framing)),
                false => self.window_click(&of, n, (&grade, current, locked), (&seen, p, &framing)),
            }
            if adding && self.effects_page.window_drawing.is_none() {
                ui.ctx().request_repaint();
            }
        }
        if !adding
            && !controls.is_empty()
            && let Some(p) = clicked.filter(|p| on_picks(*p).is_none())
            && let Some((index, Grip::Point(i))) = target(&seen, current, &framing, p)
            && index == current
        {
            self.effects_page.window_points = Some(PointPick {
                of: of.clone(),
                node: n,
                index,
                points: vec![i],
                bound: None,
                held: None,
            });
        }
        if resp.double_clicked()
            && alt
            && !locked
            && let Some(p) = pointer
            && let Some((index, Grip::Point(i))) = target(&seen, current, &framing, p)
            && let Some(WindowShape::Curve { points, .. }) = windows.get(index).map(|w| &w.shape)
            && let Some(t) = points.get(i)
        {
            let mut next = points.clone();
            next[i] = [t[1]; 3];
            self.effects_page.window = index;
            self.hold_grade(&of, n, with_points(&grade, n, (keyed, index), next));
        }
        if resp.clicked_by(PointerButton::Middle)
            && !locked
            && let Some(p) = pointer
            && let Some((index, Grip::Point(i))) = target(&seen, current, &framing, p)
        {
            let mut removed = false;
            let next = with_window(&grade, n, (keyed, index), |w| {
                removed = match &mut w.shape {
                    WindowShape::Polygon { points, .. } if points.len() > POLYGON_LEAST => {
                        points.remove(i);
                        true
                    }
                    WindowShape::Curve {
                        points,
                        inner,
                        outer,
                        ..
                    } if i < points.len() => {
                        points.remove(i);
                        for moves in [inner, outer].into_iter().flatten() {
                            if i < moves.len() {
                                moves.remove(i);
                            }
                        }
                        true
                    }
                    _ => false,
                }
            });
            if removed {
                self.effects_page.window = index;
                self.effects_page.window_points = None;
                self.hold_grade(&of, n, next);
            }
        }
        if self.grade_unsent() {
            self.sync_picture_preview();
            if !ui.input(|i| i.pointer.any_down()) {
                ui.ctx().request_repaint();
            }
        }
        true
    }

    pub(super) fn delete_window_points(&mut self) -> bool {
        if self.effects_page.palette != GradePalette::Window {
            return false;
        }
        let Some(pick) = self.effects_page.window_points.take() else {
            return false;
        };
        let Some((of, _, stored)) = self.grade_shown() else {
            return false;
        };
        let n = pick.node;
        if pick.of != of || self.current_grade_node(&stored) != Some(n) {
            return false;
        }
        let grade = self.held_grade(&of, n).cloned().unwrap_or(stored);
        let Some(node) = grade.node(n) else {
            return false;
        };
        if node.locked {
            return true;
        }
        let keyed = self.grade_frame(&of);
        let mut gone = pick.points;
        gone.sort_unstable_by(|a, b| b.cmp(a));
        gone.dedup();
        let mut removed = false;
        let next = with_window(&grade, n, (keyed, pick.index), |w| {
            if let WindowShape::Curve {
                points,
                inner,
                outer,
                ..
            } = &mut w.shape
            {
                let had = points.len();
                for &i in gone.iter().filter(|i| **i < had) {
                    points.remove(i);
                    for moves in [&mut *inner, &mut *outer].into_iter().flatten() {
                        if i < moves.len() {
                            moves.remove(i);
                        }
                    }
                    removed = true;
                }
            }
        });
        if removed {
            self.hold_grade(&of, n, next);
        }
        removed
    }

    fn curve_click(
        &mut self,
        of: &GradeOf,
        n: u32,
        (grade, current): (&Grade, usize),
        (hit, p, framing): (Option<(usize, Grip)>, Pos2, &Framing),
    ) {
        let keyed = self.grade_frame(of);
        let Some(w) = grade
            .node(n)
            .and_then(|d| d.windows_changed_at(keyed).get(current))
        else {
            return;
        };
        let WindowShape::Curve { points, .. } = &w.shape else {
            return;
        };
        match hit {
            Some((_, Grip::Point(0))) if points.len() >= 2 => {
                self.effects_page.window_drawing = None;
            }
            Some(_) => {}
            None => {
                let frame = framing.frame;
                let point = model_point(w, &shaped(w, frame), frame, framing.window(p));
                let mut next = points.clone();
                next.push([point; 3]);
                self.effects_page.window_drawing = Some(Drawing {
                    of: of.clone(),
                    node: n,
                    index: current,
                });
                self.hold_grade(of, n, with_points(grade, n, (keyed, current), next));
            }
        }
    }

    fn window_click(
        &mut self,
        of: &GradeOf,
        n: u32,
        (grade, current, locked): (&Grade, usize, bool),
        (seen, p, framing): (&[Seen], Pos2, &Framing),
    ) {
        let frame = framing.frame;
        let keyed = self.grade_frame(of);
        let windows = grade
            .node(n)
            .map_or(&[][..], |d| d.windows_changed_at(keyed));
        let on_grip = target(seen, current, framing, p).filter(|(_, g)| *g != Grip::Move);
        let mine = seen
            .iter()
            .find(|s| s.index == current)
            .filter(|_| on_grip.is_none() && !locked);
        if let Some(s) = mine
            && let Some(w) = windows.get(s.index)
        {
            match (&s.shaped.outline, &w.shape) {
                (Outline::Polygon { .. }, WindowShape::Polygon { points, .. })
                    if points.len() < POLYGON_MOST =>
                {
                    let shown: Vec<Pos2> =
                        s.drawn.outline.iter().map(|q| framing.screen(*q)).collect();
                    if let Some(i) = near_edge(&shown, p) {
                        let point = model_point(w, &s.shaped, frame, framing.window(p));
                        let next = with_window(grade, n, (keyed, s.index), |w| {
                            if let WindowShape::Polygon { points, .. } = &mut w.shape {
                                points.insert(i + 1, point);
                            }
                        });
                        self.hold_grade(of, n, next);
                        return;
                    }
                }
                (Outline::Curve { controls, .. }, WindowShape::Curve { .. }) => {
                    if let Some(at) = curve_split(controls, framing, p)
                        && let Some(next) = curve_inserted(w, &s.shaped, frame, at)
                    {
                        self.hold_grade(of, n, with_shape(grade, n, (keyed, s.index), next));
                        return;
                    }
                }
                _ => {}
            }
        }
        if let Some((index, _)) = target(seen, current, framing, p) {
            self.effects_page.window = index;
        }
    }
}
