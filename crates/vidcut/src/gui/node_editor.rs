use std::fmt::Debug;
use std::hash::Hash;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, PointerButton, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};

use super::theme;

const CELL: Vec2 = vec2(132.0, 64.0);
const BOX: Vec2 = vec2(104.0, 30.0);
const MARGIN: Vec2 = vec2(40.0, 40.0);
const KNOT: f32 = 5.0;
const PORT: f32 = KNOT * 3.0;
const ZOOMS: (f32, f32) = (0.3, 3.0);
const ROUND: f64 = 100.0;
const WIRE_HIT: f32 = 6.0;
const STEPS: usize = 24;
const FADED: f32 = 0.45;
const LIT: Color32 = Color32::from_rgb(110, 170, 255);
pub(super) const PICTURE_KNOT: Color32 = Color32::from_rgb(230, 200, 60);
pub(super) const MASK_KNOT: Color32 = Color32::from_rgb(90, 140, 230);
pub(super) const RGB_KNOT: Color32 = Color32::from_rgb(120, 210, 120);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Side {
    Left,
    Top,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Edge {
    Left,
    Right,
}

pub(super) struct Inlet {
    pub(super) slot: usize,
    pub(super) side: Side,
    pub(super) knot: Color32,
    pub(super) wire: Stroke,
    pub(super) name: String,
}

pub(super) struct Outlet {
    pub(super) name: String,
    pub(super) color: Color32,
}

pub(super) struct Tile<K> {
    pub(super) key: K,
    pub(super) label: String,
    pub(super) place: [f64; 2],
    pub(super) faded: bool,
    pub(super) outlet: Option<Outlet>,
    pub(super) inlets: Vec<Inlet>,
    pub(super) edge: Option<Edge>,
}

pub(super) struct Wire<K> {
    pub(super) from: K,
    pub(super) to: K,
    pub(super) slot: usize,
}

pub(super) struct Graph<'a, K> {
    pub(super) salt: String,
    pub(super) tiles: Vec<Tile<K>>,
    pub(super) wires: Vec<Wire<K>>,
    pub(super) selected: &'a [K],
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum End<K> {
    Output(K),
    Input(K, usize),
}

pub(super) enum Spot<K> {
    Knot(End<K>),
    Tile(K),
    Empty,
}

pub(super) struct Link<K> {
    pub(super) anchor: End<K>,
    pub(super) pulled: Option<(K, usize)>,
    pub(super) spot: Spot<K>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Pick {
    Only,
    Toggle,
    Active,
}

pub(super) struct Done<K> {
    pub(super) picked: Option<(K, Pick)>,
    pub(super) pointed: Option<K>,
    pub(super) moved: Vec<(K, [f64; 2])>,
    pub(super) link: Option<Link<K>>,
    pub(super) band: Option<(Vec<K>, bool)>,
    pub(super) clicked: bool,
    pub(super) unlink: Option<(K, usize)>,
    pub(super) spot: Option<[f64; 2]>,
}

struct Moving<K> {
    node: K,
    from: Vec<(K, [f64; 2])>,
    by: Vec2,
}

struct Linking<K> {
    anchor: End<K>,
    pulled: Option<(K, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Half {
    Output,
    Input,
}

pub(super) struct NodeView<K> {
    pan: Vec2,
    zoom: f32,
    moving: Option<Moving<K>>,
    linking: Option<Linking<K>>,
    band: Option<(Pos2, bool)>,
}

impl<K> Default for NodeView<K> {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            moving: None,
            linking: None,
            band: None,
        }
    }
}

impl<K> NodeView<K> {
    pub(super) fn calm(&mut self) {
        self.linking = None;
        self.band = None;
    }

    pub(super) fn stop(&mut self) {
        self.moving = None;
        self.calm();
    }
}

fn rounded(x: f64) -> f64 {
    (x * ROUND).round() / ROUND
}

fn along(side: Side) -> Vec2 {
    match side {
        Side::Left => vec2(1.0, 0.0),
        Side::Top => vec2(0.0, 1.0),
    }
}

fn size<K>(t: &Tile<K>) -> Vec2 {
    let left = t.inlets.iter().filter(|i| i.side == Side::Left).count();
    vec2(BOX.x, BOX.y.max(left as f32 * PORT + KNOT * 2.0))
}

fn outlet_at<K>(r: Rect, t: &Tile<K>) -> Pos2 {
    match t.edge {
        Some(_) => r.center(),
        None => r.right_center(),
    }
}

fn inlet_at<K>(r: Rect, t: &Tile<K>, slot: usize) -> Option<(Pos2, Side)> {
    let i = t.inlets.iter().find(|i| i.slot == slot)?;
    if t.edge.is_some() {
        return Some((r.center(), i.side));
    }
    let at = match i.side {
        Side::Top => r.center_top(),
        Side::Left => {
            let left: Vec<usize> = t
                .inlets
                .iter()
                .filter(|i| i.side == Side::Left)
                .map(|i| i.slot)
                .collect();
            let j = left.iter().position(|s| *s == slot).unwrap_or_default();
            let y = r.top() + r.height() * (j + 1) as f32 / (left.len() + 1) as f32;
            pos2(r.left(), y)
        }
    };
    Some((at, i.side))
}

fn edge_at(rect: Rect, edge: Edge) -> Pos2 {
    match edge {
        Edge::Left => pos2(rect.left() + MARGIN.x / 2.0, rect.center().y),
        Edge::Right => pos2(rect.right() - MARGIN.x / 2.0, rect.center().y),
    }
}

fn bezier((from, to): (Pos2, Pos2), into: Vec2) -> [Pos2; 4] {
    let bend = (from.distance(to) * 0.4).max(24.0);
    [from, from + vec2(bend, 0.0), to - into * bend, to]
}

fn on_curve(b: &[Pos2; 4], t: f32) -> Pos2 {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    let v = b
        .iter()
        .zip(w)
        .fold(Vec2::ZERO, |sum, (p, k)| sum + p.to_vec2() * k);
    v.to_pos2()
}

fn sampled_curve(b: &[Pos2; 4], (from, to): (f32, f32)) -> Vec<Pos2> {
    (0..=STEPS)
        .map(|k| from + (to - from) * k as f32 / STEPS as f32)
        .map(|t| on_curve(b, t))
        .collect()
}

fn closest(b: &[Pos2; 4], at: Pos2) -> (f32, Half) {
    let points = sampled_curve(b, (0.0, 1.0));
    let mut best = (f32::MAX, Half::Output);
    for (k, pair) in points.windows(2).enumerate() {
        let d = egui::emath::Rangef::new(0.0, 1.0);
        let seg = pair[1] - pair[0];
        let len = seg.length_sq().max(1e-6);
        let t = d.clamp((at - pair[0]).dot(seg) / len);
        let dist = at.distance(pair[0] + seg * t);
        if dist < best.0 {
            let half = if k < STEPS / 2 {
                Half::Output
            } else {
                Half::Input
            };
            best = (dist, half);
        }
    }
    best
}

fn knot(painter: &egui::Painter, tip: Pos2, along: Vec2, color: Color32) {
    let back = tip - along * KNOT * 1.6;
    let side = vec2(-along.y, along.x) * KNOT;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, back + side, back - side],
        color,
        Stroke::NONE,
    ));
}

pub(super) fn empty(ui: &mut Ui, text: &str) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let painter = ui.painter_at(rect);
    let c = theme::colors();
    painter.rect_filled(rect, CornerRadius::same(2), c.panel_dark);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(14.0),
        c.text_dim,
    );
}

pub(super) fn show<K: Clone + PartialEq + Hash + Debug>(
    ui: &mut Ui,
    view: &mut NodeView<K>,
    graph: &Graph<'_, K>,
    mut node_menu: impl FnMut(&mut Ui, &K),
    field_menu: impl FnOnce(&mut Ui),
) -> Done<K> {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let c = theme::colors();
    painter.rect_filled(rect, CornerRadius::same(2), c.panel_dark);
    if resp.hovered() {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if zoom != 1.0 {
            view.zoom = (view.zoom * zoom).clamp(ZOOMS.0, ZOOMS.1);
        } else {
            view.pan += scroll;
        }
    }
    if resp.dragged_by(PointerButton::Middle) {
        view.pan += resp.drag_delta();
    }
    let z = view.zoom;
    let origin = rect.min + MARGIN * z + view.pan;
    let grid_at = |at: Pos2| {
        let d = (at - origin) / (CELL * z);
        [rounded(f64::from(d.x)), rounded(f64::from(d.y))]
    };
    let moving = view.moving.as_ref();
    let place = |t: &Tile<K>| -> [f64; 2] {
        let from = moving.and_then(|m| {
            let (_, at) = m.from.iter().find(|(key, _)| *key == t.key)?;
            Some((m.by, *at))
        });
        match from {
            Some((by, at)) => [
                at[0] + f64::from(by.x / (CELL.x * z)),
                at[1] + f64::from(by.y / (CELL.y * z)),
            ],
            None => t.place,
        }
    };
    let boxes: Vec<Rect> = graph
        .tiles
        .iter()
        .map(|t| match t.edge {
            Some(edge) => Rect::from_center_size(edge_at(rect, edge), Vec2::splat(KNOT * 4.0)),
            None => {
                let [x, y] = place(t);
                let at = origin + vec2(x as f32 * CELL.x, y as f32 * CELL.y) * z;
                Rect::from_min_size(at, size(t) * z)
            }
        })
        .collect();
    let index_of = |key: &K| graph.tiles.iter().position(|t| t.key == *key);
    let mut knots: Vec<(End<K>, Pos2, &str)> = Vec::new();
    for (t, r) in graph.tiles.iter().zip(&boxes) {
        if let Some(o) = &t.outlet {
            knots.push((End::Output(t.key.clone()), outlet_at(*r, t), &o.name));
        }
        for i in &t.inlets {
            if let Some((at, _)) = inlet_at(*r, t, i.slot) {
                knots.push((End::Input(t.key.clone(), i.slot), at, &i.name));
            }
        }
    }
    let knot_rect = |at: Pos2| Rect::from_center_size(at, Vec2::splat(KNOT * 3.0));
    let spot_at = |at: Pos2| -> Spot<K> {
        if let Some((end, _, _)) = knots.iter().find(|(_, k, _)| knot_rect(*k).contains(at)) {
            return Spot::Knot(end.clone());
        }
        match boxes.iter().position(|r| r.contains(at)) {
            Some(i) => Spot::Tile(graph.tiles[i].key.clone()),
            None => Spot::Empty,
        }
    };
    let wires = &graph.wires;
    let pulled = view.linking.as_ref().and_then(|l| l.pulled.clone());
    let end_of = |w: &Wire<K>| -> Option<(Pos2, Side, Stroke)> {
        let i = index_of(&w.to)?;
        let t = &graph.tiles[i];
        let (at, side) = inlet_at(boxes[i], t, w.slot)?;
        let line = t.inlets.iter().find(|i| i.slot == w.slot)?.wire;
        Some((at, side, line))
    };
    let curve = |w: &Wire<K>| -> Option<[Pos2; 4]> {
        let i = index_of(&w.from)?;
        let from = outlet_at(boxes[i], &graph.tiles[i]);
        let (to, side, _) = end_of(w)?;
        Some(bezier((from, to), along(side)))
    };
    let wire_at = |at: Pos2| -> Option<(usize, Half)> {
        if boxes.iter().any(|r| r.expand(KNOT * 2.0).contains(at)) {
            return None;
        }
        wires
            .iter()
            .enumerate()
            .filter_map(|(k, w)| {
                let (d, half) = closest(&curve(w)?, at);
                (d <= WIRE_HIT).then_some((k, half, d))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(k, half, _)| (k, half))
    };
    let busy = view.linking.is_some() || view.band.is_some() || view.moving.is_some();
    let pointer = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|at| rect.contains(*at));
    let near = pointer.filter(|_| !busy).and_then(wire_at);
    let lit = Stroke::new(2.5, LIT);
    for (k, w) in wires.iter().enumerate() {
        if pulled.as_ref() == Some(&(w.to.clone(), w.slot)) {
            continue;
        }
        let (Some(b), Some((_, _, line))) = (curve(w), end_of(w)) else {
            continue;
        };
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            b,
            false,
            Color32::TRANSPARENT,
            line,
        ));
        if let Some((_, half)) = near.filter(|(j, _)| *j == k) {
            let span = match half {
                Half::Output => (0.0, 0.5),
                Half::Input => (0.5, 1.0),
            };
            painter.add(egui::Shape::line(sampled_curve(&b, span), lit));
        }
    }
    let mut done = Done {
        picked: None,
        pointed: None,
        moved: Vec::new(),
        link: None,
        band: None,
        clicked: false,
        unlink: None,
        spot: None,
    };
    for (t, r) in graph.tiles.iter().zip(&boxes) {
        let r = *r;
        let used = wires.iter().any(|w| w.from == t.key);
        if t.edge.is_some() {
            let linked = used || wires.iter().any(|w| w.to == t.key);
            let color = match (&t.outlet, t.inlets.first()) {
                (Some(o), _) if linked => o.color,
                (None, Some(i)) if linked => i.knot,
                _ => c.text_dim,
            };
            painter.circle_filled(r.center(), KNOT * 1.6, color);
            painter.text(
                r.center_bottom() + vec2(0.0, 2.0),
                Align2::CENTER_TOP,
                &t.label,
                FontId::proportional(11.0),
                c.text_dim,
            );
            continue;
        }
        let id = ui.scope_id().with((&graph.salt, &t.key));
        let node_resp = ui.interact(r, id, Sense::click_and_drag());
        let selected = graph.selected.contains(&t.key);
        let active = graph.selected.last() == Some(&t.key);
        let fade = if t.faded { FADED } else { 1.0 };
        painter.rect_filled(r, CornerRadius::same(4), c.panel.gamma_multiply(fade));
        let edge = match (active, selected) {
            (true, _) => Stroke::new(2.0, c.accent),
            (false, true) => Stroke::new(1.5, Color32::WHITE),
            _ => Stroke::new(1.0, c.text_dim.gamma_multiply(0.6)),
        };
        painter.rect_stroke(r, CornerRadius::same(4), edge, StrokeKind::Inside);
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            &t.label,
            FontId::proportional(12.0 * z.clamp(0.6, 1.4)),
            c.text.gamma_multiply(fade),
        );
        if let Some(o) = &t.outlet {
            let out = if used { o.color } else { c.text_dim };
            painter.rect_filled(
                Rect::from_center_size(r.right_center(), Vec2::splat(KNOT * 2.0)),
                CornerRadius::ZERO,
                out,
            );
        }
        for i in &t.inlets {
            if let Some((at, side)) = inlet_at(r, t, i.slot) {
                knot(&painter, at, along(side), i.knot);
            }
        }
        if node_resp.clicked() {
            let (add, alt) = ui.input(|i| (i.modifiers.command, i.modifiers.alt));
            let how = match (alt && selected, add) {
                (true, _) => Pick::Active,
                (false, true) => Pick::Toggle,
                (false, false) => Pick::Only,
            };
            done.picked = Some((t.key.clone(), how));
        }
        if node_resp.secondary_clicked() && !selected {
            done.pointed = Some(t.key.clone());
        }
        node_resp.context_menu(|ui| node_menu(ui, &t.key));
        if node_resp.drag_started_by(PointerButton::Primary) {
            let group = match selected {
                true => graph.selected.to_vec(),
                false => vec![t.key.clone()],
            };
            let from = graph
                .tiles
                .iter()
                .filter(|g| g.edge.is_none() && group.contains(&g.key))
                .map(|g| (g.key.clone(), g.place))
                .collect();
            view.moving = Some(Moving {
                node: t.key.clone(),
                from,
                by: Vec2::ZERO,
            });
        }
        if node_resp.dragged_by(PointerButton::Primary)
            && let Some(m) = view.moving.as_mut().filter(|m| m.node == t.key)
        {
            m.by += node_resp.drag_delta();
        }
        if node_resp.drag_stopped()
            && let Some(m) = view.moving.take_if(|m| m.node == t.key)
        {
            for (key, from) in &m.from {
                let to = [
                    rounded(from[0] + f64::from(m.by.x / (CELL.x * z))),
                    rounded(from[1] + f64::from(m.by.y / (CELL.y * z))),
                ];
                if to != *from {
                    done.moved.push((key.clone(), to));
                }
            }
        }
    }
    let mut started: Option<Linking<K>> = None;
    for (k, (end, at, name)) in knots.iter().enumerate() {
        let id = ui.scope_id().with((&graph.salt, "knot", k));
        let knot_resp = ui
            .interact(knot_rect(*at), id, Sense::drag())
            .on_hover_text(*name);
        if !knot_resp.drag_started_by(PointerButton::Primary) {
            continue;
        }
        let from = match end {
            End::Input(dst, slot) => wires
                .iter()
                .find(|w| w.to == *dst && w.slot == *slot)
                .map(|w| w.from.clone()),
            End::Output(_) => None,
        };
        started = Some(match (end, from) {
            (End::Input(dst, slot), Some(src)) => Linking {
                anchor: End::Output(src),
                pulled: Some((dst.clone(), *slot)),
            },
            _ => Linking {
                anchor: end.clone(),
                pulled: None,
            },
        });
    }
    if resp.drag_started_by(PointerButton::Primary)
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        match wire_at(press).map(|(k, half)| (&wires[k], half)) {
            Some((w, Half::Input)) => {
                started = Some(Linking {
                    anchor: End::Output(w.from.clone()),
                    pulled: Some((w.to.clone(), w.slot)),
                });
            }
            Some((w, Half::Output)) => {
                started = Some(Linking {
                    anchor: End::Input(w.to.clone(), w.slot),
                    pulled: Some((w.to.clone(), w.slot)),
                });
            }
            None => {
                let off = ui.input(|i| i.modifiers.command);
                view.band = Some((press, off));
            }
        }
    }
    if started.is_some() {
        view.linking = started;
    }
    let released = ui.input(|i| i.pointer.primary_released());
    let at = ui.input(|i| i.pointer.interact_pos());
    if let Some(l) = &view.linking
        && let Some(to) = at
    {
        let from = match &l.anchor {
            End::Output(s) => index_of(s).map(|i| outlet_at(boxes[i], &graph.tiles[i])),
            End::Input(d, slot) => index_of(d)
                .and_then(|i| inlet_at(boxes[i], &graph.tiles[i], *slot))
                .map(|(p, _)| p),
        };
        if let Some(from) = from {
            painter.line_segment([from, to], lit);
        }
    }
    if released && let Some(l) = view.linking.take() {
        done.link = Some(Link {
            anchor: l.anchor,
            pulled: l.pulled,
            spot: at.map_or(Spot::Empty, spot_at),
        });
    }
    if let Some((start, off)) = view.band
        && let Some(to) = at
    {
        let band = Rect::from_two_pos(start, to);
        painter.rect_filled(band, CornerRadius::ZERO, c.accent.gamma_multiply(0.12));
        painter.rect_stroke(
            band,
            CornerRadius::ZERO,
            Stroke::new(1.0, c.accent),
            StrokeKind::Inside,
        );
        if released {
            let inside = graph
                .tiles
                .iter()
                .zip(&boxes)
                .filter(|(t, r)| t.edge.is_none() && band.intersects(**r))
                .map(|(t, _)| t.key.clone())
                .collect();
            done.band = Some((inside, off));
        }
    }
    if released {
        view.band = None;
    }
    if resp.secondary_clicked()
        && let Some(at) = ui.input(|i| i.pointer.interact_pos())
    {
        done.spot = Some(grid_at(at));
    }
    resp.context_menu(field_menu);
    if done.picked.is_none() && resp.clicked() {
        let press = ui.input(|i| i.pointer.interact_pos());
        if let Some(at) = press {
            done.spot = Some(grid_at(at));
        }
        done.clicked = true;
        done.unlink = press
            .and_then(wire_at)
            .filter(|(_, half)| *half == Half::Input)
            .map(|(k, _)| (wires[k].to.clone(), wires[k].slot));
    }
    done
}
