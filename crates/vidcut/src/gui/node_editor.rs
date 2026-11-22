use std::fmt::Debug;
use std::hash::Hash;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, PointerButton, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextureId, Ui, Vec2, pos2, vec2,
};

use vidcut_core::request::NodeZoom;

use super::theme;

const CELL: Vec2 = vec2(132.0, 64.0);
const BOX: Vec2 = vec2(104.0, 30.0);
const SHOT_BOX: Vec2 = vec2(104.0, 66.0);
const STRIP: f32 = 16.0;
const SHOT_MARGIN: f32 = 4.0;
const ROW_SHOT: Vec2 = vec2(46.0, 22.0);
pub(super) const SHOT_HEIGHT: f32 = SHOT_BOX.y - STRIP - SHOT_MARGIN;
const MARGIN: Vec2 = vec2(40.0, 40.0);
const KNOT: f32 = 5.0;
const PORT: f32 = KNOT * 3.0;
const ZOOMS: (f32, f32) = (0.3, 3.0);
const ZOOM_STEP: f32 = 1.25;
const ROW: f32 = 26.0;
const ROUND: f64 = 100.0;
const WIRE_HIT: f32 = 6.0;
const STEPS: usize = 24;
const FADED: f32 = 0.4;
const NOTE: Vec2 = vec2(140.0, 18.0);
const LIT: Color32 = Color32::from_rgb(110, 170, 255);
const CURRENT_ROW: Color32 = super::viewer::ACTIVE_ANGLE;
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
    pub(super) name: String,
}

pub(super) struct Badge {
    pub(super) icon: &'static str,
    pub(super) tip: &'static str,
}

const LOCKED: Badge = Badge {
    icon: egui_phosphor::regular::LOCK_SIMPLE,
    tip: "Locked",
};

pub(super) struct Outlet {
    pub(super) slot: usize,
    pub(super) name: String,
    pub(super) color: Color32,
}

pub(super) struct Shot {
    pub(super) picture: Option<(TextureId, Rect, Vec2)>,
}

pub(super) struct Tile<K> {
    pub(super) key: K,
    pub(super) label: String,
    pub(super) place: [f64; 2],
    pub(super) faded: bool,
    pub(super) switch: bool,
    pub(super) drops: bool,
    pub(super) note: String,
    pub(super) badges: Vec<Badge>,
    pub(super) locked: Option<bool>,
    pub(super) outlets: Vec<Outlet>,
    pub(super) inlets: Vec<Inlet>,
    pub(super) edge: Option<Edge>,
    pub(super) shot: Option<Shot>,
}

pub(super) struct Wire<K> {
    pub(super) from: K,
    pub(super) out: usize,
    pub(super) to: K,
    pub(super) slot: usize,
    pub(super) drops: bool,
    pub(super) line: Stroke,
}

pub(super) struct Graph<'a, K> {
    pub(super) salt: String,
    pub(super) tiles: Vec<Tile<K>>,
    pub(super) wires: Vec<Wire<K>>,
    pub(super) selected: &'a [K],
    pub(super) current: Option<&'a K>,
    pub(super) active: Color32,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum End<K> {
    Output(K, usize),
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
    pub(super) switched: Option<K>,
    pub(super) locked: Option<K>,
    pub(super) named: Option<(K, String)>,
    pub(super) pointed: Option<K>,
    pub(super) moved: Vec<(K, [f64; 2])>,
    pub(super) link: Option<Link<K>>,
    pub(super) band: Option<(Vec<K>, bool)>,
    pub(super) clicked: bool,
    pub(super) unlink: Option<(K, usize)>,
    pub(super) spot: Option<[f64; 2]>,
    pub(super) dropped: Option<Dropped<K>>,
}

pub(super) enum Dropped<K> {
    Swap(K, K),
    Copy(K, K),
    Insert {
        node: K,
        wire: (K, K, usize),
        place: [f64; 2],
    },
}

struct Naming<K> {
    key: K,
    text: String,
    started: bool,
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
    naming: Option<Naming<K>>,
    asked: Option<NodeZoom>,
    shown: Option<Rect>,
    cell: Vec2,
    tile: Vec2,
}

impl<K> Default for NodeView<K> {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            moving: None,
            linking: None,
            band: None,
            naming: None,
            asked: None,
            shown: None,
            cell: CELL,
            tile: BOX,
        }
    }
}

impl<K> NodeView<K> {
    pub(super) fn ask(&mut self, zoom: NodeZoom) {
        self.asked = Some(zoom);
    }

    pub(super) fn zoom(&self) -> f32 {
        self.zoom
    }

    pub(super) fn area(&self) -> Option<[[f64; 2]; 2]> {
        let rect = self.shown?;
        let z = self.zoom;
        let origin = rect.min + MARGIN * z + self.pan;
        let inset = vec2(MARGIN.x * 2.0, MARGIN.y);
        let from = rect.min + inset;
        let to = (rect.max - inset - self.tile * z).max(from);
        let place = |at: Pos2| {
            let d = (at - origin) / (self.cell * z);
            [f64::from(d.x), f64::from(d.y)]
        };
        Some([place(from), place(to)])
    }

    fn zoomed(&mut self, rect: Rect, tiles: &[Tile<K>]) {
        let Some(asked) = self.asked.take() else {
            return;
        };
        let about = |view: &mut Self, to: f32| {
            let to = to.clamp(ZOOMS.0, ZOOMS.1);
            let at = (rect.center() - rect.min - MARGIN * view.zoom - view.pan) / view.zoom;
            view.pan = rect.center() - rect.min - MARGIN * to - at * to;
            view.zoom = to;
        };
        match asked {
            NodeZoom::In => about(self, self.zoom * ZOOM_STEP),
            NodeZoom::Out => about(self, self.zoom / ZOOM_STEP),
            NodeZoom::Original => about(self, 1.0),
            NodeZoom::Fit => {
                let cell = self.cell;
                let boxes = (tiles.iter().filter(|t| t.edge.is_none())).map(|t| {
                    let at = vec2(t.place[0] as f32 * cell.x, t.place[1] as f32 * cell.y);
                    Rect::from_min_size(at.to_pos2(), size(t))
                });
                let Some(all) = boxes.reduce(|a, b| a.union(b)) else {
                    return;
                };
                let room = rect.size() - MARGIN * 2.0;
                let fit = (room.x / all.width()).min(room.y / all.height());
                let to = fit.clamp(ZOOMS.0, ZOOMS.1);
                self.pan = rect.center() - rect.min - MARGIN * to - all.center().to_vec2() * to;
                self.zoom = to;
            }
        }
    }
    pub(super) fn calm(&mut self) {
        self.linking = None;
        self.band = None;
    }

    pub(super) fn stop(&mut self) {
        self.moving = None;
        self.naming = None;
        self.calm();
    }

    pub(super) fn name(&mut self, key: K, text: String) {
        self.naming = Some(Naming {
            key,
            text,
            started: false,
        });
    }
}

fn label_field<K: Clone + PartialEq>(
    ui: &mut Ui,
    naming: &mut Option<Naming<K>>,
    key: &K,
    field: Rect,
    font: FontId,
    named: &mut Option<(K, String)>,
) -> bool {
    let Some(n) = naming.as_mut().filter(|n| n.key == *key) else {
        return false;
    };
    let edit = egui::TextEdit::singleline(&mut n.text)
        .font(font)
        .margin(vec2(2.0, 0.0));
    let edit = ui.put(field, edit);
    if !n.started {
        edit.request_focus();
        n.started = true;
    } else if edit.lost_focus() {
        if !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *named = Some((n.key.clone(), n.text.clone()));
        }
        *naming = None;
    }
    true
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
    let rows = left.max(t.outlets.len());
    let body = if t.shot.is_some() { SHOT_BOX } else { BOX };
    vec2(body.x, body.y.max(rows as f32 * PORT + KNOT * 2.0))
}

fn shots_in<K>(tiles: &[Tile<K>]) -> bool {
    tiles.iter().any(|t| t.shot.is_some())
}

fn paint_shot(painter: &egui::Painter, area: Rect, shot: &Shot, fade: f32) {
    painter.rect_filled(area, CornerRadius::same(2), theme::colors().panel_dark);
    let Some((texture, uv, pixels)) = shot.picture else {
        return;
    };
    let scale = (area.width() / pixels.x).min(area.height() / pixels.y);
    let shown = Rect::from_center_size(area.center(), pixels * scale);
    let tint = Color32::from_gray((255.0 * fade).round() as u8);
    painter.image(texture, shown, uv, tint);
}

fn outlet_at<K>(r: Rect, t: &Tile<K>, slot: usize) -> Option<Pos2> {
    let j = t.outlets.iter().position(|o| o.slot == slot)?;
    if t.edge.is_some() {
        return Some(r.center());
    }
    let y = r.top() + r.height() * (j + 1) as f32 / (t.outlets.len() + 1) as f32;
    Some(pos2(r.right(), y))
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

impl<K> Done<K> {
    fn none() -> Self {
        Self {
            picked: None,
            switched: None,
            locked: None,
            named: None,
            pointed: None,
            moved: Vec::new(),
            link: None,
            band: None,
            clicked: false,
            unlink: None,
            spot: None,
            dropped: None,
        }
    }
}

pub(super) fn list<K: Clone + PartialEq + Hash + Debug>(
    ui: &mut Ui,
    view: &mut NodeView<K>,
    graph: &Graph<'_, K>,
    mut node_menu: impl FnMut(&mut Ui, &K),
) -> Done<K> {
    let mut done = Done::none();
    let mut naming = view.naming.take();
    let c = theme::colors();
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, CornerRadius::same(2), c.panel_dark);
    let font = FontId::proportional(12.0);
    let numbers = (graph.tiles.iter().filter(|t| t.switch))
        .map(|t| (ui.painter()).layout_no_wrap(t.label.clone(), font.clone(), c.text))
        .fold(0.0, |widest, shown| shown.size().x.max(widest));
    let shots = shots_in(&graph.tiles);
    egui::ScrollArea::vertical().show(ui, |ui| {
        for t in graph.tiles.iter().rev().filter(|t| t.edge.is_none()) {
            let width = ui.available_width();
            let (r, resp) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
            let painter = ui.painter_at(r);
            let fade = if t.faded { FADED } else { 1.0 };
            painter.rect_filled(r.shrink(1.0), CornerRadius::same(3), c.panel);
            let edge = match (
                graph.current == Some(&t.key),
                graph.selected.contains(&t.key),
            ) {
                (true, _) => Stroke::new(2.0, CURRENT_ROW),
                (false, true) => Stroke::new(1.5, Color32::WHITE),
                _ => Stroke::NONE,
            };
            painter.rect_stroke(
                r.shrink(1.0),
                CornerRadius::same(3),
                edge,
                StrokeKind::Inside,
            );
            let dim = c.text_dim.gamma_multiply(fade);
            let start = r.left() + 6.0;
            let (number, text) = match t.switch {
                true => (t.label.as_str(), t.note.clone()),
                false => ("", format!("{} {}", t.label, t.note).trim_end().to_string()),
            };
            painter.text(
                pos2(start, r.center().y),
                Align2::LEFT_CENTER,
                number,
                font.clone(),
                c.text.gamma_multiply(fade),
            );
            let eye = Rect::from_min_size(pos2(start + numbers + 4.0, r.top()), vec2(ROW, ROW));
            if t.switch {
                let icon = match t.faded {
                    true => egui_phosphor::regular::EYE_SLASH,
                    false => egui_phosphor::regular::EYE,
                };
                painter.text(eye.center(), Align2::CENTER_CENTER, icon, font.clone(), dim);
            }
            let lock = Rect::from_min_size(pos2(eye.right(), r.top()), vec2(ROW, ROW));
            if let Some(locked) = t.locked {
                let (icon, color) = match locked {
                    true => (
                        egui_phosphor::regular::LOCK_SIMPLE,
                        c.text.gamma_multiply(fade),
                    ),
                    false => (egui_phosphor::regular::LOCK_SIMPLE_OPEN, dim),
                };
                painter.text(
                    lock.center(),
                    Align2::CENTER_CENTER,
                    icon,
                    font.clone(),
                    color,
                );
            }
            let mut left = lock.right() + 4.0;
            if shots {
                let at = pos2(left, r.center().y - ROW_SHOT.y / 2.0);
                let area = Rect::from_min_size(at, ROW_SHOT);
                if let Some(shot) = &t.shot {
                    paint_shot(&painter, area, shot, fade);
                }
                left = area.right() + 6.0;
            }
            let mut right = r.right() - 8.0;
            for b in t.badges.iter().rev() {
                let at = pos2(right, r.center().y);
                let shown = painter.text(at, Align2::RIGHT_CENTER, b.icon, font.clone(), dim);
                right = shown.left() - 4.0;
            }
            let field = Rect::from_min_max(pos2(left, r.top()), pos2(right.max(left), r.bottom()));
            let editing = label_field(
                ui,
                &mut naming,
                &t.key,
                field,
                font.clone(),
                &mut done.named,
            );
            if !editing && !text.is_empty() {
                let color = c.text.gamma_multiply(fade);
                let width = right - left;
                let shown = super::history::one_line(ui, &text, width, font.clone(), color);
                painter.galley(
                    pos2(left, r.center().y - shown.size().y / 2.0),
                    shown,
                    color,
                );
            }
            let at = resp.interact_pointer_pos();
            let on_eye = at.is_some_and(|p| eye.contains(p));
            let on_lock = at.is_some_and(|p| lock.contains(p));
            if resp.clicked() && t.switch && on_eye {
                done.switched = Some(t.key.clone());
            } else if resp.clicked() && t.locked.is_some() && on_lock {
                done.locked = Some(t.key.clone());
            } else if resp.clicked() {
                let add = ui.input(|i| i.modifiers.command);
                done.picked = Some((t.key.clone(), if add { Pick::Toggle } else { Pick::Only }));
            }
            resp.context_menu(|ui| node_menu(ui, &t.key));
        }
    });
    view.naming = naming.filter(|n| graph.tiles.iter().any(|t| t.key == n.key));
    done
}

pub(super) fn show<K: Clone + PartialEq + Hash + Debug>(
    ui: &mut Ui,
    view: &mut NodeView<K>,
    graph: &Graph<'_, K>,
    mut node_menu: impl FnMut(&mut Ui, &K),
    field_menu: impl FnOnce(&mut Ui),
) -> Done<K> {
    let rect = ui.available_rect_before_wrap();
    view.shown = Some(rect);
    view.tile = if shots_in(&graph.tiles) {
        SHOT_BOX
    } else {
        BOX
    };
    view.cell = CELL + vec2(0.0, view.tile.y - BOX.y);
    let cell = view.cell;
    view.zoomed(rect, &graph.tiles);
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
        let d = (at - origin) / (cell * z);
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
                at[0] + f64::from(by.x / (cell.x * z)),
                at[1] + f64::from(by.y / (cell.y * z)),
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
                let at = origin + vec2(x as f32 * cell.x, y as f32 * cell.y) * z;
                Rect::from_min_size(at, size(t) * z)
            }
        })
        .collect();
    let index_of = |key: &K| graph.tiles.iter().position(|t| t.key == *key);
    let mut knots: Vec<(End<K>, Pos2, Option<&str>)> = Vec::new();
    for (t, r) in graph.tiles.iter().zip(&boxes) {
        for o in &t.outlets {
            if let Some(at) = outlet_at(*r, t, o.slot) {
                let name = (o.name != t.label).then_some(o.name.as_str());
                knots.push((End::Output(t.key.clone(), o.slot), at, name));
            }
        }
        for i in &t.inlets {
            if let Some((at, _)) = inlet_at(*r, t, i.slot) {
                let name = (i.name != t.label).then_some(i.name.as_str());
                knots.push((End::Input(t.key.clone(), i.slot), at, name));
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
        Some((at, side, w.line))
    };
    let curve = |w: &Wire<K>| -> Option<[Pos2; 4]> {
        let i = index_of(&w.from)?;
        let from = outlet_at(boxes[i], &graph.tiles[i], w.out)?;
        let (to, side, _) = end_of(w)?;
        Some(bezier((from, to), along(side)))
    };
    let wire_near = |at: Pos2, skip: Option<usize>| -> Option<(usize, Half)> {
        let over = |(i, r): (usize, &Rect)| Some(i) != skip && r.expand(KNOT * 2.0).contains(at);
        if boxes.iter().enumerate().any(over) {
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
    let wire_at = |at: Pos2| wire_near(at, None);
    let near = pointer.filter(|_| !busy).and_then(wire_at);
    let loose = |key: &K| !wires.iter().any(|w| w.from == *key || w.to == *key);
    let inserting = (view.moving.as_ref())
        .filter(|m| m.from.len() == 1)
        .and_then(|m| index_of(&m.node))
        .filter(|i| graph.tiles[*i].drops && loose(&graph.tiles[*i].key))
        .and_then(|i| pointer.and_then(|p| wire_near(p, Some(i))))
        .map(|(k, _)| k)
        .filter(|k| wires[*k].drops);
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
    let mut done = Done::none();
    let mut naming = view.naming.take();
    for (t, r) in graph.tiles.iter().zip(&boxes) {
        let r = *r;
        let used = wires.iter().any(|w| w.from == t.key);
        if t.edge.is_some() {
            let linked = used || wires.iter().any(|w| w.to == t.key);
            let color = match (t.outlets.first(), t.inlets.first()) {
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
        let mut node_resp = ui.interact(r, id, Sense::click_and_drag());
        let selected = graph.selected.contains(&t.key);
        let active = graph.current == Some(&t.key);
        let fade = if t.faded { FADED } else { 1.0 };
        painter.rect_filled(r, CornerRadius::same(4), c.panel.gamma_multiply(fade));
        let edge = match (active, selected) {
            (true, _) => Stroke::new(2.0, graph.active),
            (false, true) => Stroke::new(1.5, Color32::WHITE),
            _ => Stroke::new(1.0, c.text_dim.gamma_multiply(0.6 * fade)),
        };
        painter.rect_stroke(r, CornerRadius::same(4), edge, StrokeKind::Inside);
        let (at, align, line) = match &t.shot {
            Some(_) => {
                let line = r.bottom() - STRIP * z / 2.0;
                (
                    pos2(r.left() + 2.0 * KNOT * z, line),
                    Align2::LEFT_CENTER,
                    line,
                )
            }
            None => (r.center(), Align2::CENTER_CENTER, r.center().y),
        };
        let label = painter.text(
            at,
            align,
            &t.label,
            FontId::proportional(12.0 * z.clamp(0.6, 1.4)),
            c.text.gamma_multiply(fade),
        );
        if let Some(shot) = &t.shot {
            let top = vec2(SHOT_MARGIN, SHOT_MARGIN) * z;
            let inset = vec2(SHOT_MARGIN, STRIP) * z;
            paint_shot(
                &painter,
                Rect::from_min_max(r.min + top, r.max - inset),
                shot,
                fade,
            );
        }
        let mark = FontId::proportional(10.0 * z.clamp(0.6, 1.4));
        let mut right = r.right() - 2.0 * KNOT * z;
        let hover = node_resp.hover_pos();
        let lock = (t.locked == Some(true)).then_some(LOCKED);
        for b in t.badges.iter().chain(lock.iter()).rev() {
            let at = pos2(right, line);
            let color = c.text_dim.gamma_multiply(fade);
            let shown = painter.text(at, Align2::RIGHT_CENTER, b.icon, mark.clone(), color);
            right = shown.left() - 3.0 * z;
            if hover.is_some_and(|p| shown.contains(p)) {
                node_resp = node_resp.on_hover_text(b.tip);
            }
        }
        let font = FontId::proportional(11.0 * z.clamp(0.6, 1.4));
        let field = Rect::from_min_size(
            r.left_top() - vec2(0.0, NOTE.y * z + 2.0),
            vec2(r.width().max(NOTE.x * z), NOTE.y * z),
        );
        let editing = label_field(
            ui,
            &mut naming,
            &t.key,
            field,
            font.clone(),
            &mut done.named,
        );
        if !editing && !t.note.is_empty() {
            let color = c.text.gamma_multiply(fade);
            let shown = super::history::one_line(ui, &t.note, r.width(), font, color);
            let at = r.left_top() - vec2(0.0, shown.size().y + 2.0);
            let place = Rect::from_min_size(at, shown.size());
            painter.galley(at, shown.clone(), color);
            let note = ui.interact(place, id.with("note"), Sense::click());
            super::history::full_on_hover(&note, &shown, &t.note);
            if note.double_clicked() {
                naming = Some(Naming {
                    key: t.key.clone(),
                    text: t.note.clone(),
                    started: false,
                });
            }
        }
        for o in &t.outlets {
            let Some(at) = outlet_at(r, t, o.slot) else {
                continue;
            };
            let used = wires.iter().any(|w| w.from == t.key && w.out == o.slot);
            let out = if used { o.color } else { c.text_dim };
            painter.rect_filled(
                Rect::from_center_size(at, Vec2::splat(KNOT * 2.0)),
                CornerRadius::ZERO,
                out,
            );
        }
        for i in &t.inlets {
            if let Some((at, side)) = inlet_at(r, t, i.slot) {
                knot(&painter, at, along(side), i.knot);
            }
        }
        let on_label = node_resp
            .interact_pointer_pos()
            .is_some_and(|at| label.expand(2.0 * z).contains(at));
        if node_resp.clicked() && t.switch && on_label {
            done.switched = Some(t.key.clone());
        } else if node_resp.clicked() {
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
        if inserting.is_some() && view.moving.as_ref().is_some_and(|m| m.node == t.key) {
            let plus = FontId::proportional(18.0 * z.clamp(0.6, 1.4));
            painter.text(r.center_top(), Align2::CENTER_BOTTOM, "+", plus, LIT);
        }
        if node_resp.drag_started_by(PointerButton::Primary) {
            let adds = t.drops && ui.input(|i| i.modifiers.command);
            let group = match (selected, adds) {
                (true, _) => graph.selected.to_vec(),
                (false, true) => {
                    done.picked = Some((t.key.clone(), Pick::Toggle));
                    let mut all = graph.selected.to_vec();
                    all.push(t.key.clone());
                    all
                }
                (false, false) => vec![t.key.clone()],
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
            let (mods, at) = ui.input(|i| (i.modifiers, i.pointer.latest_pos()));
            let onto = at
                .filter(|_| m.from.len() == 1 && t.drops)
                .and_then(|p| {
                    let mut all = graph.tiles.iter().zip(&boxes);
                    all.find(|(o, b)| o.key != t.key && o.drops && b.contains(p))
                })
                .map(|(o, _)| o.key.clone());
            done.dropped = match onto {
                Some(o) if mods.command => Some(Dropped::Swap(t.key.clone(), o)),
                Some(o) if mods.alt => Some(Dropped::Copy(t.key.clone(), o)),
                _ => None,
            };
            let moved = (m.from.iter())
                .map(|(key, from)| {
                    let to = [
                        rounded(from[0] + f64::from(m.by.x / (cell.x * z))),
                        rounded(from[1] + f64::from(m.by.y / (cell.y * z))),
                    ];
                    (key.clone(), *from, to)
                })
                .filter(|(_, from, to)| to != from);
            match (done.dropped.is_some(), inserting.map(|k| &wires[k])) {
                (true, _) => {}
                (false, Some(w)) => {
                    done.dropped = moved.last().map(|(node, _, place)| Dropped::Insert {
                        node,
                        wire: (w.from.clone(), w.to.clone(), w.slot),
                        place,
                    });
                }
                (false, None) => done.moved.extend(moved.map(|(key, _, to)| (key, to))),
            }
        }
    }
    view.naming = naming.filter(|n| graph.tiles.iter().any(|t| t.key == n.key));
    let mut started: Option<Linking<K>> = None;
    for (k, (end, at, name)) in knots.iter().enumerate() {
        let id = ui.scope_id().with((&graph.salt, "knot", k));
        let knot_resp = ui.interact(knot_rect(*at), id, Sense::drag());
        let knot_resp = match name {
            Some(name) => knot_resp.on_hover_text(*name),
            None => knot_resp,
        };
        if !knot_resp.drag_started_by(PointerButton::Primary) {
            continue;
        }
        let from = match end {
            End::Input(dst, slot) => wires
                .iter()
                .find(|w| w.to == *dst && w.slot == *slot)
                .map(|w| (w.from.clone(), w.out)),
            End::Output(..) => None,
        };
        started = Some(match (end, from) {
            (End::Input(dst, slot), Some((src, out))) => Linking {
                anchor: End::Output(src, out),
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
                    anchor: End::Output(w.from.clone(), w.out),
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
            End::Output(s, o) => index_of(s).and_then(|i| outlet_at(boxes[i], &graph.tiles[i], *o)),
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
