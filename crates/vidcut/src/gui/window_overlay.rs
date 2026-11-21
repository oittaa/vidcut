use std::f64::consts::TAU;

use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Response, Shape, Stroke, Vec2};
use vidcut_core::model::{Grade, GradeWindow, WindowShape};
use vidcut_core::ops::grade::GradeOf;
use vidcut_core::ops::grade_windows::{POLYGON_LEAST, POLYGON_MOST};
use vidcut_core::place::{ahead, apply};
use vidcut_core::request::{GradePalette, Page};
use vidcut_core::window_key::{
    CIRCLE_RADIUS, GRADIENT_SOFT_STEP, Outline, PAN_STEP, SOFT_STEP, Shaped, circle_scale,
    circle_size, linear_scale, origin, shaped,
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
    Arrow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    Handle,
    Softness,
    Middle,
    Arrow,
}

struct Drawn {
    outline: Vec<[f64; 2]>,
    closed: bool,
    guides: Vec<Vec<[f64; 2]>>,
    grips: Vec<(Grip, Look, [f64; 2])>,
}

pub(super) struct WindowGrab {
    of: GradeOf,
    node: u32,
    index: usize,
    grip: Grip,
    grade: Grade,
    from: [f64; 2],
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

fn drawn(w: &GradeWindow, s: &Shaped, [fw, fh]: [f64; 2], gap: f64) -> Drawn {
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
                guides.push(ellipse((r - sf).max(0.0)));
                guides.push(ellipse(r + sf));
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

fn polygon_point(w: &GradeWindow, s: &Shaped, frame: [f64; 2], q: [f64; 2]) -> [f64; 2] {
    let k = linear_scale(w.size);
    let [u, v] = Local::of(s).local(q);
    [u / (frame[0] * k), v / (frame[1] * k)]
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
                0 => a = (a + lt[0].abs() - lf[0].abs()).max(1.0),
                _ => b = (b + lt[1].abs() - lf[1].abs()).max(1.0),
            }
            n.aspect = span(match a >= b {
                true => 100.0 - 50.0 * b / a,
                false => 50.0 * a / b,
            });
            n.size = span(circle_size(a.min(b) / (CIRCLE_RADIUS * frame[0])));
        }
        (Grip::Soft(i), Outline::Circle { shrink, .. }, WindowShape::Circle { soft }) => {
            let along = |l: [f64; 2]| match i % 2 {
                0 => l[0].abs() * shrink[0],
                _ => l[1].abs() * shrink[1],
            };
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
        (Grip::Point(i), Outline::Polygon { .. }, WindowShape::Polygon { points, .. }) => {
            if let Some(p) = points.get(i).copied() {
                let k = linear_scale(w.size);
                let q = local.frame([p[0] * frame[0] * k, p[1] * frame[1] * k]);
                points[i] = polygon_point(w, &s, frame, [q[0] + by[0], q[1] + by[1]]);
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

fn target(seen: &[Seen], current: usize, framing: &Framing, p: Pos2) -> Option<(usize, Grip)> {
    let order = (seen.iter().filter(|s| s.index == current))
        .chain(seen.iter().rev().filter(|s| s.index != current));
    let grip = order
        .clone()
        .flat_map(|s| {
            (s.drawn.grips.iter())
                .map(move |(g, _, q)| (s.index, *g, framing.screen(*q).distance(p)))
        })
        .filter(|(_, _, d)| *d <= GRIP)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(i, g, _)| (i, g));
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

impl Workspace {
    pub(super) fn window_controls(
        &mut self,
        ui: &egui::Ui,
        resp: &Response,
        picture: egui::Rect,
    ) -> bool {
        if self.effects_page.page != Page::Color
            || self.effects_page.palette != GradePalette::Window
        {
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
        let (windows, locked) = (node.windows.clone(), node.locked);
        let current = self
            .effects_page
            .window
            .min(windows.len().saturating_sub(1));
        let frame = framing.frame;
        let mut seen: Vec<Seen> = (windows.iter().enumerate())
            .filter(|(_, w)| w.on)
            .map(|(index, w)| {
                let shaped = shaped(w, frame);
                let gap = SOFT_GAP * framing.per_screen(shaped.middle);
                let drawn = drawn(w, &shaped, frame, gap);
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
        let hover = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| resp.rect.contains(*p));
        let grabbed = self.effects_page.window_grab.is_some();
        if let Some((_, grip)) = hover.and_then(|p| target(&seen, current, &framing, p)) {
            ui.ctx().set_cursor_icon(match (grip, grabbed) {
                (_, true) => CursorIcon::Grabbing,
                (Grip::Move, false) => CursorIcon::Move,
                (_, false) => CursorIcon::Grab,
            });
        }
        let press = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(p) = press
            && let Some((index, grip)) = target(&seen, current, &framing, p)
        {
            self.effects_page.window = index;
            self.effects_page.window_grab = (!locked).then(|| WindowGrab {
                of: of.clone(),
                node: n,
                index,
                grip,
                grade: grade.clone(),
                from: framing.window(p),
            });
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if resp.dragged_by(PointerButton::Primary)
            && let Some(to) = pointer
            && let Some(g) = &self.effects_page.window_grab
            && g.of == of
            && g.node == n
            && let Some(start) = g.grade.node(n).and_then(|d| d.windows.get(g.index))
        {
            let next_window = dragged(start, g.grip, (g.from, framing.window(to)), frame);
            let mut next = g.grade.clone();
            let index = g.index;
            if let Some(w) = next.node_mut(n).and_then(|d| d.windows.get_mut(index)) {
                *w = next_window;
            }
            self.hold_grade(&of, n, next);
        }
        if resp.drag_stopped() {
            self.effects_page.window_grab = None;
        }
        if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = pointer
        {
            let on_grip = target(&seen, current, &framing, p).filter(|(_, g)| *g != Grip::Move);
            let edge = (seen.iter())
                .find(|s| s.index == current)
                .filter(|s| {
                    matches!(s.shaped.outline, Outline::Polygon { .. }) && on_grip.is_none()
                })
                .and_then(|s| {
                    let shown: Vec<Pos2> =
                        s.drawn.outline.iter().map(|q| framing.screen(*q)).collect();
                    near_edge(&shown, p).map(|i| (s, i))
                });
            match edge {
                Some((s, i)) if !locked => {
                    let w = &windows[s.index];
                    if let WindowShape::Polygon { points, .. } = &w.shape
                        && points.len() < POLYGON_MOST
                    {
                        let point = polygon_point(w, &s.shaped, frame, framing.window(p));
                        let mut next = grade.clone();
                        if let Some(WindowShape::Polygon { points, .. }) = (next.node_mut(n))
                            .and_then(|d| d.windows.get_mut(s.index))
                            .map(|w| &mut w.shape)
                        {
                            points.insert(i + 1, point);
                        }
                        self.hold_grade(&of, n, next);
                    }
                }
                _ => {
                    if let Some((index, _)) = target(&seen, current, &framing, p) {
                        self.effects_page.window = index;
                    }
                }
            }
        }
        if resp.clicked_by(PointerButton::Middle)
            && !locked
            && let Some(p) = pointer
            && let Some((index, Grip::Point(i))) = target(&seen, current, &framing, p)
            && let Some(WindowShape::Polygon { points, .. }) = windows.get(index).map(|w| &w.shape)
            && points.len() > POLYGON_LEAST
        {
            let mut next = grade.clone();
            if let Some(WindowShape::Polygon { points, .. }) = (next.node_mut(n))
                .and_then(|d| d.windows.get_mut(index))
                .map(|w| &mut w.shape)
            {
                points.remove(i);
            }
            self.effects_page.window = index;
            self.hold_grade(&of, n, next);
        }
        true
    }
}
