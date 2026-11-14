use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, PointerButton, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2, pos2, vec2,
};
use serde_json::{Value, json};
use vidcut_core::composition::Frames;
use vidcut_core::effect_keys::Keyed;
use vidcut_core::model::{
    Clip, Composition, EffectValue, Mask, Node, Pattern, Project, Tool, ToolType,
};
use vidcut_core::motion::Area;
use vidcut_core::place::{Figure, MaskBase, Outline, Shaping, turned};
use vidcut_core::render::RenderPlan;
use vidcut_core::request::{NodeInput, Page};
use vidcut_core::shortcuts::{Action, Panel};

use super::shortcuts::Fired;
use super::shuttle::Jkl;
use super::theme::{self, Deco};
use super::workspace::{Workspace, dim, fit};
use crate::live::{Answer, PageClock, PageTime, PlayerId};

const CELL: Vec2 = vec2(132.0, 64.0);
const BOX: Vec2 = vec2(104.0, 30.0);
const MARGIN: Vec2 = vec2(40.0, 40.0);
const KNOT: f32 = 5.0;
const GRIP: f32 = 9.0;
const DASH: f32 = 4.0;
const MAGNIFIED: f32 = 120.0;
const HANDLE: f32 = 4.0;
const RING: f32 = 27.0;
const RING_STEPS: usize = 48;
const RULER: f32 = 24.0;
const BAR: f32 = 10.0;
const LABEL_GAP: f64 = 60.0;
const MIN_FRAMES: f64 = 10.0;
const TICKS: [i64; 19] = [
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000,
    200_000, 500_000, 1_000_000,
];
const RANGE: Color32 = Color32::from_rgb(232, 196, 44);
const PLAYHEAD: Color32 = Color32::from_rgb(230, 60, 60);
const TOOLS: f32 = 30.0;
const TRANSPORT: f32 = 36.0;
const SETTLE: f64 = 2.0;
const SCREEN: usize = 0;
const ZOOMS: (f32, f32) = (0.3, 3.0);
const ROUND: f64 = 100.0;
const WIRE_HIT: f32 = 6.0;
const STEPS: usize = 24;
const LIBRARY: [(&str, &[ToolType]); 4] = [
    ("Tools", &[ToolType::Blur]),
    (
        "Masks",
        &[ToolType::Rectangle, ToolType::Ellipse, ToolType::Polygon],
    ),
    ("Tracking", &[ToolType::Tracker]),
    (
        "Effects",
        &[
            ToolType::GaussianBlur,
            ToolType::MosaicBlur,
            ToolType::Glow,
            ToolType::FilmGrain,
            ToolType::Vignette,
            ToolType::Sharpen,
        ],
    ),
];
const TOOL_ROW: [&[ToolType]; 2] = [
    &[ToolType::Blur],
    &[ToolType::Rectangle, ToolType::Ellipse, ToolType::Polygon],
];

pub(super) struct EffectsPage {
    pub(super) page: Page,
    pub(super) nodes: Vec<String>,
    seen: Option<String>,
    alone: Option<Alone>,
    cells: Option<super::cells::Cells>,
    held: Option<Held>,
    boxed: Option<Boxed>,
    pan: Vec2,
    zoom: f32,
    moving: Option<Moving>,
    asked: Vec<(Answer, Then)>,
    linking: Option<Linking>,
    band: Option<(Pos2, bool)>,
    renaming: Option<Renaming>,
    replacing: Option<(String, String)>,
    spot: Option<(String, [f64; 2])>,
    copied: Vec<Node>,
    away: Option<Away>,
    view: (f64, f64),
    zooming: Option<Zoom>,
    bounds: Option<((i64, i64), (bool, bool))>,
}

impl Default for EffectsPage {
    fn default() -> Self {
        Self {
            page: Page::Cut,
            nodes: Vec::new(),
            seen: None,
            alone: None,
            cells: None,
            held: None,
            boxed: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            moving: None,
            asked: Vec::new(),
            linking: None,
            band: None,
            renaming: None,
            replacing: None,
            spot: None,
            copied: Vec::new(),
            away: None,
            view: (0.0, 1.0),
            zooming: None,
            bounds: None,
        }
    }
}

impl EffectsPage {
    pub(super) fn picked(&self) -> Option<&str> {
        self.nodes.last().map(String::as_str)
    }

    pub(super) fn leave(&mut self) {
        self.alone = None;
        self.held = None;
        self.boxed = None;
        self.moving = None;
        self.linking = None;
        self.band = None;
        self.renaming = None;
        self.replacing = None;
        self.spot = None;
        self.away = None;
        self.zooming = None;
        if let Some(cells) = &mut self.cells {
            cells.close();
        }
    }
}

struct Alone {
    rev: u64,
    clip: String,
    wanted: Option<Composition>,
    plan: RenderPlan,
}

#[derive(Debug, Clone, Copy)]
struct Run {
    speed: f64,
    from: f64,
    since: Instant,
}

impl Run {
    fn at(&self, fps: f64) -> f64 {
        self.from + self.since.elapsed().as_secs_f64() * fps * self.speed
    }
}

struct Away {
    clip: String,
    at: f64,
    run: Option<Run>,
    landing: Option<Instant>,
    on: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    Left,
    Right,
    Both,
}

struct Held {
    clip: String,
    node: String,
    grip: Grip,
    composition: Composition,
    sent: Option<(u64, Instant)>,
    turn: f64,
    last: [f64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Center,
    Width,
    Height,
    Angle,
    Point(usize),
}

struct Boxed {
    clip: String,
    node: String,
    pattern: usize,
    edge: Edge,
    area: Area,
    grab: [f64; 2],
    sent: Option<(u64, Instant)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Edge {
    Corner,
    Left,
    Right,
    Top,
    Bottom,
}

fn edge_at(r: Rect, press: Pos2) -> Option<Edge> {
    if r.left_top().distance(press) <= GRIP {
        return Some(Edge::Corner);
    }
    let across = |lo: f32, hi: f32, v: f32| (lo - GRIP..=hi + GRIP).contains(&v);
    let near = |a: f32, b: f32| (a - b).abs() <= HANDLE;
    let sides = [
        (
            Edge::Left,
            near(press.x, r.left()) && across(r.top(), r.bottom(), press.y),
        ),
        (
            Edge::Right,
            near(press.x, r.right()) && across(r.top(), r.bottom(), press.y),
        ),
        (
            Edge::Top,
            near(press.y, r.top()) && across(r.left(), r.right(), press.x),
        ),
        (
            Edge::Bottom,
            near(press.y, r.bottom()) && across(r.left(), r.right(), press.x),
        ),
    ];
    sides.into_iter().find(|(_, on)| *on).map(|(e, _)| e)
}

fn magnified(painter: &egui::Painter, texture: egui::TextureId, r: Rect, picture: Rect) {
    let part = |p: Pos2| {
        let d = p - picture.min;
        pos2(d.x / picture.width(), d.y / picture.height())
    };
    let uv = Rect::from_min_max(part(r.min), part(r.max));
    let aspect = (r.width() / r.height().max(1.0)).clamp(0.25, 4.0);
    let size = match aspect >= 1.0 {
        true => vec2(MAGNIFIED, MAGNIFIED / aspect),
        false => vec2(MAGNIFIED * aspect, MAGNIFIED),
    };
    let shown = Rect::from_min_size(r.right_top() + vec2(GRIP, 0.0), size);
    let stroke = Stroke::new(1.0, theme::colors().accent);
    painter.rect_filled(shown, CornerRadius::ZERO, Color32::BLACK);
    painter.image(texture, shown, uv, Color32::WHITE);
    painter.rect_stroke(shown, CornerRadius::ZERO, stroke, StrokeKind::Outside);
    let c = shown.center();
    painter.line_segment([pos2(shown.left(), c.y), pos2(shown.right(), c.y)], stroke);
    painter.line_segment([pos2(c.x, shown.top()), pos2(c.x, shown.bottom())], stroke);
}

fn boxed_to(b: &Boxed, to: [f64; 2]) -> Area {
    let (lo, hi) = Pattern::SIDES;
    let mut a = b.area;
    match b.edge {
        Edge::Corner => {
            a.center = [
                to[0] - b.grab[0] + a.size[0] / 2.0,
                to[1] - b.grab[1] + a.size[1] / 2.0,
            ];
        }
        Edge::Left | Edge::Right => {
            a.size[0] = (2.0 * (to[0] - a.center[0]).abs()).clamp(lo, hi);
        }
        Edge::Top | Edge::Bottom => {
            a.size[1] = (2.0 * (to[1] - a.center[1]).abs()).clamp(lo, hi);
        }
    }
    let (lo, hi) = Pattern::PLACES;
    a.center = [
        a.center[0].clamp(lo, hi),
        a.center[1].clamp(1.0 - hi, 1.0 - lo),
    ];
    a
}

struct Shown {
    name: String,
    base: MaskBase,
    middle: [f64; 2],
    turn: f64,
    spots: Vec<(Grip, Pos2)>,
}

struct Moving {
    node: String,
    from: Vec<(String, [f64; 2])>,
    by: Vec2,
}

#[derive(Debug, Clone, PartialEq)]
enum End {
    Output(String),
    Input(String, NodeInput),
}

struct Linking {
    anchor: End,
    pulled: Option<(String, NodeInput)>,
}

struct Renaming {
    clip: String,
    queue: Vec<String>,
    name: String,
    focus: bool,
}

#[derive(Debug, Clone)]
enum Then {
    Select,
    Renamed(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Pick {
    Only,
    Toggle,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Half {
    Output,
    Input,
}

struct Wire {
    to: String,
    which: NodeInput,
    from: String,
}

enum Spot {
    Knot(End),
    Tile(String),
    Empty,
}

#[derive(Debug, Clone, Copy)]
enum MenuAct {
    Rename,
    PassThrough,
    Copy,
    Paste,
    Delete,
}

fn joined(n: &Node, which: NodeInput) -> Option<String> {
    match which {
        NodeInput::Input => n.input.clone(),
        NodeInput::EffectMask => n.effect_mask.clone(),
    }
}

fn joins(comp: &Composition) -> Vec<Wire> {
    let mut out = Vec::new();
    for n in &comp.nodes {
        for which in [NodeInput::Input, NodeInput::EffectMask] {
            if let Some(from) = joined(n, which) {
                out.push(Wire {
                    to: n.name.clone(),
                    which,
                    from,
                });
            }
        }
    }
    out
}

fn inlet(r: Rect, which: NodeInput) -> Pos2 {
    match which {
        NodeInput::Input => r.left_center(),
        NodeInput::EffectMask => r.center_top(),
    }
}

fn along(which: NodeInput) -> Vec2 {
    match which {
        NodeInput::Input => vec2(1.0, 0.0),
        NodeInput::EffectMask => vec2(0.0, 1.0),
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

fn knot_name(end: &End) -> &'static str {
    match end {
        End::Output(_) => "Output",
        End::Input(_, NodeInput::Input) => "Input",
        End::Input(_, NodeInput::EffectMask) => "Effect Mask",
    }
}

fn default_input(d: &Node, s: &Node) -> Option<NodeInput> {
    if s.tool.is_mask() {
        return d.tool.takes_mask().then_some(NodeInput::EffectMask);
    }
    (d.tool.takes_input() && d.input.is_none()).then_some(NodeInput::Input)
}

fn command(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn join_command(clip: &str, (dst, which): (&str, NodeInput), src: Option<&str>) -> Value {
    let mut params = json!({ "clip": clip, "node": dst, "input": which });
    if let Some(src) = src {
        params["from"] = json!(src);
    }
    command("comp.connect", params)
}

fn linked(comp: &Composition, clip: &str, l: &Linking, spot: Spot) -> Vec<Value> {
    let new = match (&l.anchor, spot) {
        (End::Output(src), Spot::Knot(End::Input(dst, which))) => Some((dst, which, src.clone())),
        (End::Output(src), Spot::Tile(dst)) => comp
            .node(src)
            .zip(comp.node(&dst))
            .and_then(|(s, d)| default_input(d, s))
            .map(|which| (dst, which, src.clone())),
        (End::Input(dst, which), Spot::Knot(End::Output(src)) | Spot::Tile(src)) => {
            Some((dst.clone(), *which, src))
        }
        _ => None,
    };
    let new = new.filter(|(d, _, s)| d != s);
    let mut out = Vec::new();
    if let Some((pd, pw)) = &l.pulled
        && new.as_ref().is_none_or(|(d, w, _)| d != pd || w != pw)
    {
        out.push(join_command(clip, (pd, *pw), None));
    }
    if let Some((d, w, s)) = new {
        let now = comp.node(&d).and_then(|n| joined(n, w));
        if now.as_deref() != Some(s.as_str()) {
            out.push(join_command(clip, (&d, w), Some(&s)));
        }
    }
    out
}

pub(super) fn stored_composition(c: &Clip) -> Composition {
    c.picture
        .as_ref()
        .and_then(|p| p.composition.clone())
        .unwrap_or_else(Composition::plain)
}

fn plus(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn rounded(x: f64) -> f64 {
    (x * ROUND).round() / ROUND
}

fn grips(f: &Figure) -> Vec<(Grip, [f64; 2])> {
    let mut out = vec![(Grip::Center, f.middle)];
    match &f.outline {
        Outline::Ellipse { half, turn } | Outline::Rectangle { half, turn, .. } => {
            out.push((Grip::Width, plus(f.middle, turned([half[0], 0.0], *turn))));
            out.push((Grip::Height, plus(f.middle, turned([0.0, -half[1]], *turn))));
            out.push((Grip::Angle, f.middle));
        }
        Outline::Polygon { points, .. } => {
            out.extend(points.iter().enumerate().map(|(i, p)| (Grip::Point(i), *p)));
        }
    }
    out
}

fn inverse(a: [[f64; 2]; 2]) -> Option<[[f64; 2]; 2]> {
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    (det.abs() > 1e-9).then(|| {
        [
            [a[1][1] / det, -a[0][1] / det],
            [-a[1][0] / det, a[0][0] / det],
        ]
    })
}

fn gripped(m: &mut Mask, tool: &Tool, grip: Grip, at: [f64; 2], base: &MaskBase) {
    let [w, h] = base.size;
    if !(w > 0.0 && h > 0.0) {
        return;
    }
    let s = Shaping::figure(m, tool, &base.unturned());
    let at = base.own(at);
    let (lo, hi) = Mask::PLACES;
    match (grip, &s.outline) {
        (Grip::Center, _) => {
            m.center = Some([(at[0] / w).clamp(lo, hi), (1.0 - at[1] / h).clamp(lo, hi)]);
        }
        (
            Grip::Width | Grip::Height,
            Outline::Ellipse { turn, .. } | Outline::Rectangle { turn, .. },
        ) => {
            let [x, y] = turned([at[0] - s.middle[0], at[1] - s.middle[1]], -turn);
            let ellipse = matches!(s.outline, Outline::Ellipse { .. });
            match grip {
                Grip::Width => m.width = Some(rounded_size(2.0 * x.max(0.0) / w)),
                _ => {
                    let side = if ellipse { base.round } else { h };
                    m.height = Some(rounded_size(2.0 * (-y).max(0.0) / side));
                }
            }
        }
        (Grip::Point(i), _) if i < m.points.len() => {
            let zero = Shaping::tilted(m, [0.0, 0.0], s.middle, (w, h));
            let across = Shaping::tilted(m, [1.0, 0.0], s.middle, (w, h));
            let down = Shaping::tilted(m, [0.0, 1.0], s.middle, (w, h));
            let a = [
                [across[0] - zero[0], down[0] - zero[0]],
                [across[1] - zero[1], down[1] - zero[1]],
            ];
            if let Some(b) = inverse(a) {
                let d = [at[0] - zero[0], at[1] - zero[1]];
                m.points[i] = [
                    b[0][0] * d[0] + b[0][1] * d[1],
                    b[1][0] * d[0] + b[1][1] * d[1],
                ];
            }
        }
        _ => {}
    }
}

fn rounded_size(x: f64) -> f64 {
    let (lo, hi) = Mask::SIZES;
    x.clamp(lo, hi)
}

fn grip_change(m: &Mask, grip: Grip) -> (&'static str, Value) {
    match grip {
        Grip::Center => ("center", json!(m.middle())),
        Grip::Width => ("width", json!(m.wide())),
        Grip::Height => ("height", json!(m.high())),
        Grip::Angle => ("angle", json!(m.turn())),
        Grip::Point(_) => ("points", json!(m.points)),
    }
}

fn grip_value(grip: Grip) -> Option<EffectValue> {
    match grip {
        Grip::Center => Some(EffectValue::Center),
        Grip::Width => Some(EffectValue::Width),
        Grip::Height => Some(EffectValue::Height),
        Grip::Angle => Some(EffectValue::Angle),
        Grip::Point(_) => None,
    }
}

fn reach(grip: Grip, at: Pos2, press: Pos2) -> f32 {
    match grip {
        Grip::Angle => (at.distance(press) - RING).abs(),
        _ => at.distance(press),
    }
}

fn swept(middle: [f64; 2], from: [f64; 2], to: [f64; 2]) -> f64 {
    use std::f64::consts::{PI, TAU};
    let at = |p: [f64; 2]| (middle[1] - p[1]).atan2(p[0] - middle[0]);
    ((at(to) - at(from) + PI).rem_euclid(TAU) - PI).to_degrees()
}

fn draw_grips(painter: &egui::Painter, f: &Figure, spots: &[(Grip, Pos2)], turning: bool) {
    let colors = theme::colors();
    let stroke = Stroke::new(1.5, colors.accent);
    for (grip, s) in spots {
        let s = *s;
        match grip {
            Grip::Center => {
                painter.line_segment([s - vec2(6.0, 0.0), s + vec2(6.0, 0.0)], stroke);
                painter.line_segment([s - vec2(0.0, 6.0), s + vec2(0.0, 6.0)], stroke);
            }
            Grip::Angle => {
                let color = if turning { colors.error } else { colors.accent };
                let ring: Vec<Pos2> = (0..=RING_STEPS)
                    .map(|i| {
                        let a = std::f32::consts::TAU * i as f32 / RING_STEPS as f32;
                        s + RING * vec2(a.cos(), a.sin())
                    })
                    .collect();
                painter.extend(egui::Shape::dashed_line(
                    &ring,
                    Stroke::new(1.0, color),
                    4.0,
                    3.0,
                ));
                let turn = match &f.outline {
                    Outline::Ellipse { turn, .. } | Outline::Rectangle { turn, .. } => *turn,
                    Outline::Polygon { .. } => 0.0,
                };
                if turning {
                    let to = s + RING * vec2(turn.cos() as f32, -turn.sin() as f32);
                    painter.line_segment([s, to], Stroke::new(1.5, color));
                }
            }
            _ => {
                painter.rect_filled(
                    Rect::from_center_size(s, Vec2::splat(HANDLE * 2.0)),
                    CornerRadius::ZERO,
                    colors.accent,
                );
            }
        }
    }
}

impl Workspace {
    pub(super) fn effects_clip(&self) -> Option<String> {
        let p = self.project()?;
        super::placing::topmost(p, self.snap.timeline.position)
    }

    pub(super) fn set_page(&mut self, page: Page) {
        if page == self.effects_page.page {
            return;
        }
        self.leave_inspector();
        if page == Page::Cut {
            self.effects_page.leave();
        }
        self.effects_page.page = page;
        self.sync_bounds();
    }

    pub(super) fn pick_nodes(&mut self, nodes: Vec<String>) -> Result<(), String> {
        let p = self.project_shared().ok_or("Open a project first")?;
        let clip = self
            .effects_clip()
            .and_then(|id| p.clip(&id))
            .ok_or("There is no clip at the playhead")?;
        let c = stored_composition(clip);
        if let Some(missing) = nodes.iter().find(|n| c.node(n).is_none()) {
            return Err(format!("{} has no node {missing}", clip.id));
        }
        let mut kept: Vec<String> = Vec::new();
        for n in nodes {
            kept.retain(|k| *k != n);
            kept.push(n);
        }
        self.effects_page.nodes = kept;
        self.effects_page.seen = Some(clip.id.clone());
        Ok(())
    }

    pub(super) fn page_composition(&self, clip: &Clip) -> Composition {
        if let Some(h) = self
            .effects_page
            .held
            .as_ref()
            .filter(|h| h.clip == clip.id)
        {
            return h.composition.clone();
        }
        let mut c = stored_composition(clip);
        self.node_preview(&mut c, self.page_at() as f64);
        c
    }

    fn settle_page(&mut self, ctx: &egui::Context) {
        let rev = self.project().map_or(0, |p| p.rev);
        let clip = self.effects_clip();
        if clip != self.effects_page.seen {
            self.effects_page.seen = clip;
            self.effects_page.nodes.clear();
            self.effects_page.moving = None;
            self.effects_page.linking = None;
            self.effects_page.band = None;
            self.effects_page.away = None;
            self.effects_page.replacing = None;
            self.effects_page.spot = None;
            self.effects_page.view = (0.0, 1.0);
            self.leave_inspector();
        }
        self.sync_bounds();
        if let Some((at, since)) = self.effects_page.held.as_ref().and_then(|h| h.sent)
            && (at != rev || since.elapsed().as_secs_f64() > SETTLE)
        {
            self.effects_page.held = None;
        }
        let mut done = Vec::new();
        self.effects_page
            .asked
            .retain(|(answer, then)| match answer.take() {
                Some(r) => {
                    done.push((r, then.clone()));
                    false
                }
                None => true,
            });
        for (r, then) in done {
            let Some(result) = r.ok().map(|r| r.result) else {
                continue;
            };
            let Some(node) = result["node"].as_str().map(str::to_owned) else {
                continue;
            };
            let all: Option<Vec<String>> = result["nodes"].as_array().map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            });
            self.effects_page.nodes = match then {
                Then::Select => all.unwrap_or_else(|| vec![node]),
                Then::Renamed(old) => self
                    .effects_page
                    .nodes
                    .iter()
                    .map(|n| if *n == old { node.clone() } else { n.clone() })
                    .collect(),
            };
        }
        if !self.effects_page.asked.is_empty() || self.effects_page.held.is_some() {
            ctx.request_repaint();
        }
    }

    pub(super) fn add_node(&mut self, tool: ToolType) {
        let Some(clip) = self.effects_clip() else {
            return;
        };
        let mut params = json!({ "clip": clip, "tool": tool });
        if let Some(after) = self.effects_page.picked() {
            params["after"] = json!(after);
        }
        let answer = self.live.request_answered("comp.add", params);
        self.effects_page.asked.push((answer, Then::Select));
    }

    fn page_clip(&self) -> Option<(Clip, Composition)> {
        let p = self.project()?;
        let c = p.clip(&self.effects_clip()?)?.clone();
        let comp = stored_composition(&c);
        Some((c, comp))
    }

    fn before_nodes(&self, comp: &Composition, gone: &[String]) -> Vec<String> {
        let mut before = self
            .effects_page
            .picked()
            .and_then(|n| comp.node(n))
            .and_then(|n| n.input.clone());
        while let Some(b) = before.clone().filter(|b| gone.contains(b)) {
            before = comp.node(&b).and_then(|n| n.input.clone());
        }
        before.into_iter().collect()
    }

    fn delete_nodes(&mut self, gone: Vec<String>) {
        let Some((clip, comp)) = self.page_clip() else {
            return;
        };
        let gone: Vec<String> = gone
            .into_iter()
            .filter(|n| comp.node(n).is_some())
            .collect();
        if gone.is_empty() {
            return;
        }
        let commands = gone
            .iter()
            .map(|n| command("comp.remove", json!({ "clip": clip.id, "node": n })))
            .collect();
        self.effects_page.nodes = self.before_nodes(&comp, &gone);
        self.send_all(commands, "Delete Nodes");
    }

    fn pass_through(&mut self) {
        let Some((clip, comp)) = self.page_clip() else {
            return;
        };
        let commands = self
            .effects_page
            .nodes
            .iter()
            .filter_map(|n| comp.node(n))
            .filter(|n| n.tool.flows() || n.tool.is_mask())
            .map(|n| {
                let params = json!({
                    "clip": clip.id,
                    "node": n.name,
                    "pass_through": !n.pass_through,
                });
                command("comp.set", params)
            })
            .collect();
        self.send_all(commands, "Pass Through");
    }

    fn start_rename(&mut self) {
        let Some(clip) = self.effects_clip() else {
            return;
        };
        let queue = self.effects_page.nodes.clone();
        let Some(first) = queue.first().cloned() else {
            return;
        };
        self.effects_page.renaming = Some(Renaming {
            clip,
            queue,
            name: first,
            focus: true,
        });
    }

    fn rename_window(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.effects_page.renaming else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        super::dialog::window(ctx, "Rename Node", &mut open, |ui| {
            let mut enter = false;
            super::dialog::form(ui, "node-name-form", |ui| {
                super::dialog::row(ui, "Name", |ui| {
                    let output = egui::TextEdit::singleline(&mut r.name)
                        .desired_width(260.0)
                        .show(ui);
                    let edit = output.response;
                    if r.focus {
                        edit.request_focus();
                        let mut state = output.state;
                        let all = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(r.name.chars().count()),
                        );
                        state.cursor.set_char_range(Some(all));
                        state.store(ui.ctx(), edit.id);
                        r.focus = false;
                    }
                    enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
            });
            let named = !r.name.trim().is_empty();
            super::dialog::footer(ui, |ui| {
                apply = super::dialog::main_button(ui, "Rename", named) || (enter && named);
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if !(apply || cancel || !open) {
            return;
        }
        let Some(r) = self.effects_page.renaming.take() else {
            return;
        };
        let mut queue = r.queue;
        let old = queue.remove(0);
        let to = r.name.trim().to_string();
        if apply && to != old {
            let params = json!({ "clip": r.clip, "node": old, "to": to });
            let answer = self.live.request_answered("comp.rename", params);
            self.effects_page.asked.push((answer, Then::Renamed(old)));
        }
        if let Some(next) = queue.first().cloned() {
            self.effects_page.renaming = Some(Renaming {
                clip: r.clip,
                queue,
                name: next,
                focus: true,
            });
        }
    }

    fn copy_node(&mut self, cut: bool) {
        let Some((_, comp)) = self.page_clip() else {
            return;
        };
        let copied: Vec<Node> = comp
            .nodes
            .iter()
            .filter(|n| self.effects_page.nodes.contains(&n.name))
            .filter(|n| n.tool.flows() || n.tool.is_mask())
            .cloned()
            .collect();
        if copied.is_empty() {
            return;
        }
        let names = copied.iter().map(|n| n.name.clone()).collect();
        self.effects_page.copied = copied;
        if cut {
            self.delete_nodes(names);
        }
    }

    fn paste_node(&mut self) {
        let clip = self.effects_clip();
        let spot = (self.effects_page.spot.as_ref()).filter(|(c, _)| Some(c) == clip.as_ref());
        let to = match (self.effects_page.picked(), spot) {
            (Some(after), _) => json!({ "after": after }),
            (None, Some((_, at))) => json!({ "place": at }),
            (None, None) => json!({}),
        };
        self.paste_to(to);
    }

    fn paste_to(&mut self, mut params: Value) {
        let copy = self.effects_page.copied.clone();
        let Some(clip) = self.effects_clip().filter(|_| !copy.is_empty()) else {
            return;
        };
        params["clip"] = json!(clip);
        params["copy"] = json!(copy);
        let answer = self.live.request_answered("comp.paste", params);
        self.effects_page.asked.push((answer, Then::Select));
    }

    fn replace_window(&mut self, ctx: &egui::Context) {
        let Some((clip, name)) = self.effects_page.replacing.clone() else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        super::dialog::window(ctx, "Replace Node", &mut open, |ui| {
            super::dialog::form(ui, "replace-node-form", |ui| {
                super::dialog::row(ui, "Node", |ui| ui.label(name.as_str()));
            });
            super::dialog::footer(ui, |ui| {
                apply = super::dialog::main_button(ui, "Replace", true);
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if !(apply || cancel || !open) {
            return;
        }
        self.effects_page.replacing = None;
        if apply && self.effects_clip().as_deref() == Some(clip.as_str()) {
            self.paste_to(json!({ "replace": name }));
        }
    }

    fn node_key_step(&mut self, dir: i64) {
        let Some((_, comp)) = self.page_clip() else {
            return;
        };
        let Some(n) = self.effects_page.picked().and_then(|n| comp.node(n)) else {
            return;
        };
        let times = n.keys.iter().flat_map(|k| k.keys.iter()).map(|k| k.frame);
        if let Some(t) = super::inspector::next_key(times, self.page_at(), dir) {
            self.page_ask(json!({ "at": t }));
        }
    }

    fn select_all(&mut self, on: bool) {
        self.effects_page.nodes = match (on, self.page_clip()) {
            (true, Some((_, comp))) => comp.nodes.iter().map(|n| n.name.clone()).collect(),
            _ => Vec::new(),
        };
    }

    fn menu_act(&mut self, act: MenuAct, node: String) {
        match act {
            MenuAct::Rename => self.start_rename(),
            MenuAct::PassThrough => self.pass_through(),
            MenuAct::Copy => self.copy_node(false),
            MenuAct::Paste => {
                let clip = self.effects_clip();
                self.effects_page.replacing = clip.map(|c| (c, node));
            }
            MenuAct::Delete => self.delete_nodes(self.effects_page.nodes.clone()),
        }
    }

    pub(super) fn effects_page_ui(&mut self, ui: &mut egui::Ui) {
        self.settle_page(ui.ctx());
        self.rename_window(ui.ctx());
        self.replace_window(ui.ctx());
        let panel = theme::colors().panel;
        egui::Panel::left("effects-library")
            .resizable(true)
            .default_size(220.0)
            .min_size(160.0)
            .frame(theme::card(panel))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.effects_library(ui))
            });
        egui::Panel::right("effects-inspector")
            .resizable(true)
            .default_size(380.0)
            .min_size(260.0)
            .frame(theme::card(panel))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.node_inspector(ui))
            });
        egui::Panel::bottom("node-editor")
            .resizable(true)
            .default_size(300.0)
            .min_size(140.0)
            .frame(theme::card(panel))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.node_editor(ui))
            });
        egui::CentralPanel::default()
            .frame(theme::card(theme::colors().panel_dark))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Viewer, |ui| self.effects_viewer(ui))
            });
    }

    fn effects_library(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Effects Library").strong());
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (title, tools) in LIBRARY {
                ui.label(dim(title));
                for tool in tools {
                    if ui.selectable_label(false, tool.label()).clicked() {
                        self.add_node(*tool);
                    }
                }
                ui.add_space(8.0);
            }
        });
    }

    fn effects_viewer(&mut self, ui: &mut egui::Ui) {
        let p = self.project_shared();
        let clip = self
            .effects_clip()
            .and_then(|id| p.as_ref().and_then(|p| p.clip(&id)).cloned());
        if let (Some(p), Some(c)) = (&p, &clip) {
            self.page_tick(ui.ctx(), p, c);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(vidcut_core::composition::MEDIA_OUT).strong());
            if let (Some(p), Some(c)) = (&p, &clip) {
                ui.label(dim(clip_name(p, c)));
            }
        });
        let avail = ui.available_rect_before_wrap();
        let below = RULER + BAR + TRANSPORT + TOOLS + 12.0;
        let area = Rect::from_min_max(
            avail.min,
            pos2(avail.max.x, (avail.max.y - below).max(avail.min.y + 40.0)),
        );
        let area_resp = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(12, 12, 13));
        match (&p, &clip) {
            (Some(p), Some(c)) => self.effects_picture(ui, &area_resp, area, (p, c)),
            _ => {
                painter.text(
                    area.center(),
                    Align2::CENTER_CENTER,
                    "No clip at the playhead",
                    FontId::proportional(14.0),
                    theme::colors().text_dim,
                );
            }
        }
        ui.add_space(4.0);
        self.time_ruler(ui, p.as_deref(), clip.as_ref());
        self.page_transport(ui, p.as_deref(), clip.as_ref());
        self.tool_row(ui);
    }

    fn alone_plan(&mut self, p: &Project, c: &Clip, comp: &Composition) -> Option<RenderPlan> {
        let stored = c.picture.as_ref().and_then(|x| x.composition.as_ref());
        let wanted = (stored.unwrap_or(&Composition::plain()) != comp).then(|| comp.clone());
        if let Some(a) = &self.effects_page.alone
            && a.rev == p.rev
            && a.clip == c.id
            && a.wanted == wanted
        {
            return Some(a.plan.clone());
        }
        let plan = match &wanted {
            None => vidcut_core::render::composition_alone(p, &c.id),
            Some(comp) => {
                let mut q = p.clone();
                if let Some((ti, ci)) = q.find_clip(&c.id) {
                    let x = &mut q.tracks[ti].clips[ci];
                    x.picture.get_or_insert_with(Default::default).composition = Some(comp.clone());
                }
                vidcut_core::render::composition_alone(&q, &c.id)
            }
        }
        .ok()?;
        self.effects_page.alone = Some(Alone {
            rev: p.rev,
            clip: c.id.clone(),
            wanted,
            plan: plan.clone(),
        });
        Some(plan)
    }

    fn effects_picture(
        &mut self,
        ui: &mut egui::Ui,
        area_resp: &egui::Response,
        area: Rect,
        (p, c): (&Project, &Clip),
    ) {
        let comp = self.page_composition(c);
        let Some(plan) = self.alone_plan(p, c, &comp) else {
            return;
        };
        let size = (plan.width, plan.height);
        let ppp = ui.ctx().pixels_per_point();
        let picture = {
            use eframe::emath::GuiRounding;
            fit(area.shrink(2.0), size.0 as f32 / size.1.max(1) as f32).round_to_pixels(ppp)
        };
        let speed = self.page_speed();
        let first = Frames::of(p, c).first;
        let frame = (self.page_at() - first).clamp(0, (plan.frames - 1).max(0));
        let gpu_pictures = self.screens.as_ref().map(|s| s.gpu_pictures());
        let ctx = ui.ctx().clone();
        let cells = self
            .effects_page
            .cells
            .get_or_insert_with(|| super::cells::Cells::new(ctx, gpu_pictures.unwrap_or_default()));
        let got = cells.show_plan(0, plan, frame, speed);
        let pixels = [picture.width(), picture.height()].map(|x| (x * ppp).round() as u32);
        let texture = got
            .as_ref()
            .zip(self.screens.as_mut())
            .and_then(|(g, s)| s.draw(SCREEN, &g.picture, pixels));
        let painter = ui.painter_at(area);
        painter.rect_filled(picture, CornerRadius::ZERO, Color32::BLACK);
        if let Some(texture) = texture {
            painter.image(
                texture,
                picture,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        self.pattern_handles(ui, area_resp, (picture, texture), (c, &comp));
        self.mask_handles(ui, area_resp, picture, c, &comp, size);
    }

    fn pattern_handles(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        (picture, texture): (Rect, Option<egui::TextureId>),
        (clip, comp): (&Clip, &Composition),
    ) {
        let rev = self.project().map_or(0, |p| p.rev);
        if let Some((at, since)) = self.effects_page.boxed.as_ref().and_then(|b| b.sent)
            && (at != rev || since.elapsed().as_secs_f64() > SETTLE)
        {
            self.effects_page.boxed = None;
        }
        let at = self.page_at();
        let frame = self
            .project_shared()
            .map_or(at as f64, |p| Frames::of(&p, clip).source(at as f64));
        let size = picture.size();
        let screen = |[x, y]: [f64; 2]| picture.min + vec2(x as f32 * size.x, y as f32 * size.y);
        let place = |p: Pos2| {
            let d = p - picture.min;
            [f64::from(d.x / size.x), f64::from(d.y / size.y)]
        };
        let painter = ui.painter_at(resp.rect);
        let colors = theme::colors();
        let picked = self.inspector.pattern_picked.clone();
        let mut spots: Vec<(String, usize, Rect, Area)> = Vec::new();
        for name in &self.effects_page.nodes {
            let Some(t) = comp.node(name).and_then(|n| n.tool.tracker()) else {
                continue;
            };
            for (k, pattern) in t.patterns.iter().enumerate() {
                let held = (self.effects_page.boxed.as_ref())
                    .filter(|b| b.clip == clip.id && b.node == *name && b.pattern == k);
                let area = match held {
                    Some(b) => b.area,
                    None => (pattern.area_at(&clip.motion, frame)).unwrap_or_else(|| pattern.own()),
                };
                let half = [area.size[0] / 2.0, area.size[1] / 2.0];
                let r = Rect::from_min_max(
                    screen([area.center[0] - half[0], area.center[1] - half[1]]),
                    screen([area.center[0] + half[0], area.center[1] + half[1]]),
                );
                let on = picked
                    .as_ref()
                    .is_some_and(|(c, n, i)| *c == clip.id && n == name && *i == k);
                let color = if on { colors.accent } else { Color32::WHITE };
                painter.rect_stroke(
                    r,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, color),
                    StrokeKind::Middle,
                );
                let corner = Rect::from_center_size(r.left_top(), Vec2::splat(2.0 * HANDLE));
                painter.rect_filled(corner, CornerRadius::ZERO, color);
                painter.text(
                    r.left_top() - vec2(0.0, HANDLE + 2.0),
                    Align2::LEFT_BOTTOM,
                    &pattern.name,
                    FontId::proportional(11.0),
                    color,
                );
                let [x, y] = pattern.shift();
                if x != 0.0 || y != 0.0 {
                    let to = screen([area.center[0] + x, area.center[1] - y]);
                    let line = [r.center(), to];
                    let stroke = Stroke::new(1.0, color);
                    painter.extend(egui::Shape::dashed_line(&line, stroke, DASH, DASH));
                    painter.circle_stroke(to, HANDLE, stroke);
                }
                if let (Some(b), Some(texture)) = (held.filter(|b| b.sent.is_none()), texture)
                    && b.edge == Edge::Corner
                {
                    magnified(&painter, texture, r, picture);
                }
                spots.push((name.clone(), k, r, area));
            }
        }
        if resp.drag_started_by(PointerButton::Primary)
            && self.effects_page.held.is_none()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let hit = spots
                .iter()
                .rev()
                .find_map(|(name, k, r, area)| Some((name, *k, edge_at(*r, press)?, *area)));
            if let Some((name, k, edge, area)) = hit {
                let start = place(press);
                let corner = [
                    area.center[0] - area.size[0] / 2.0,
                    area.center[1] - area.size[1] / 2.0,
                ];
                self.effects_page.boxed = Some(Boxed {
                    clip: clip.id.clone(),
                    node: name.clone(),
                    pattern: k,
                    edge,
                    area,
                    grab: [start[0] - corner[0], start[1] - corner[1]],
                    sent: None,
                });
                self.inspector.pattern_picked = Some((clip.id.clone(), name.clone(), k));
            }
        }
        let Some(b) = self
            .effects_page
            .boxed
            .as_mut()
            .filter(|b| b.sent.is_none() && b.clip == clip.id)
        else {
            return;
        };
        if resp.dragged()
            && let Some(to) = ui.input(|i| i.pointer.interact_pos())
        {
            b.area = boxed_to(b, place(to));
        }
        if !resp.drag_stopped() {
            return;
        }
        b.sent = Some((rev, Instant::now()));
        let a = b.area;
        let params = json!({
            "clip": clip.id,
            "node": b.node,
            "pattern": b.pattern + 1,
            "pattern_center": [a.center[0], 1.0 - a.center[1]],
            "pattern_width": a.size[0],
            "pattern_height": a.size[1],
            "frame": at,
        });
        self.send("comp.set", params);
    }

    fn mask_handles(
        &mut self,
        ui: &mut egui::Ui,
        resp: &egui::Response,
        picture: Rect,
        clip: &Clip,
        comp: &Composition,
        (w, h): (u32, u32),
    ) {
        let at = self.page_at();
        let source = at as f64;
        let frame = self
            .project_shared()
            .map_or(source, |p| Frames::of(&p, clip).source(source));
        let areas = |pattern: &Pattern| pattern.area_at(&clip.motion, frame);
        let (w, h) = (f64::from(w), f64::from(h));
        let scale = f64::from(picture.width()) / w;
        let screen = |[x, y]: [f64; 2]| picture.min + vec2((x * scale) as f32, (y * scale) as f32);
        let pixel = |p: Pos2| {
            let d = p - picture.min;
            [f64::from(d.x) / scale, f64::from(d.y) / scale]
        };
        let painter = ui.painter_at(resp.rect);
        let accent = theme::colors().accent;
        let turning = self
            .effects_page
            .held
            .as_ref()
            .filter(|x| x.sent.is_none() && x.clip == clip.id && x.grip == Grip::Angle)
            .map(|x| x.node.clone());
        let mut shown = Vec::new();
        for name in &self.effects_page.nodes {
            let Some(node) = comp.node(name).map(|n| n.sampled(source)) else {
                continue;
            };
            let Some(m) = node.tool.mask().filter(|m| m.show_view_controls) else {
                continue;
            };
            let bases = MaskBase::all(m, comp, (w, h), &areas);
            let shaping = Shaping::of(m, &node.tool, w, &bases);
            for f in &shaping.figures {
                let line: Vec<Pos2> = f.line().into_iter().map(screen).collect();
                painter.add(egui::Shape::line(line, Stroke::new(1.5, accent)));
            }
            let (Some(first), Some(base)) = (shaping.figures.first(), bases.first()) else {
                continue;
            };
            let spots: Vec<(Grip, Pos2)> = grips(first)
                .into_iter()
                .filter(|(g, _)| *g != Grip::Center || m.connect.is_none())
                .map(|(g, s)| (g, screen(s)))
                .collect();
            let turned = turning.as_deref() == Some(name.as_str());
            draw_grips(&painter, first, &spots, turned);
            shown.push(Shown {
                name: name.clone(),
                base: *base,
                middle: first.middle,
                turn: m.turn(),
                spots,
            });
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if resp.drag_started_by(PointerButton::Primary)
            && self
                .effects_page
                .boxed
                .as_ref()
                .is_none_or(|b| b.sent.is_some())
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let near = shown
                .iter()
                .rev()
                .flat_map(|s| {
                    s.spots
                        .iter()
                        .map(move |(g, at)| (s, *g, reach(*g, *at, press)))
                })
                .filter(|(_, _, d)| *d <= GRIP)
                .min_by(|a, b| a.2.total_cmp(&b.2));
            if let Some((s, grip, _)) = near {
                self.effects_page.held = Some(Held {
                    clip: clip.id.clone(),
                    node: s.name.clone(),
                    grip,
                    composition: comp.clone(),
                    sent: None,
                    turn: s.turn,
                    last: pixel(press),
                });
            }
        }
        let Some(name) = self
            .effects_page
            .held
            .as_ref()
            .filter(|x| x.sent.is_none() && x.clip == clip.id)
            .map(|x| x.node.clone())
        else {
            return;
        };
        let Some(s) = shown.iter().find(|s| s.name == name) else {
            self.effects_page.held = None;
            return;
        };
        let rev = self.project().map_or(0, |p| p.rev);
        let Some(held) = self.effects_page.held.as_mut() else {
            return;
        };
        if resp.dragged()
            && let Some(to) = pointer
            && let Some(i) = held.composition.index(&name)
        {
            let n = &mut held.composition.nodes[i];
            if let Some(v) = grip_value(held.grip) {
                n.key_list_mut().retain(|k| k.value != v);
            }
            let tool = n.sampled(source).tool;
            let to = pixel(to);
            if let (Some(m), Some(now)) = (n.tool.mask_mut(), tool.mask()) {
                *m = now.clone();
                match held.grip {
                    Grip::Angle => {
                        held.turn += swept(s.middle, held.last, to);
                        let (lo, hi) = Mask::TURNS;
                        m.angle = Some(held.turn.clamp(lo, hi));
                    }
                    grip => gripped(m, &tool, grip, to, &s.base),
                }
            }
            held.last = to;
        }
        if !resp.drag_stopped() {
            return;
        }
        let change = held
            .composition
            .node(&name)
            .and_then(|n| n.tool.mask())
            .map(|m| grip_change(m, held.grip));
        held.sent = Some((rev, Instant::now()));
        if let Some((key, value)) = change {
            let mut params = json!({ "clip": clip.id, "node": name, "frame": at });
            params[key] = value;
            self.send("comp.set", params);
        }
    }

    pub(super) fn page_at(&self) -> i64 {
        match &self.effects_page.away {
            Some(a) if self.effects_page.seen.as_ref() == Some(&a.clip) => a.at.floor() as i64,
            _ => self
                .page_frames()
                .map_or(0, |(_, _, f)| f.at(self.snap.timeline.position)),
        }
    }

    fn page_frames(&self) -> Option<(Arc<Project>, Clip, Frames)> {
        let (p, c) = self.page_source()?;
        let f = Frames::of(&p, &c);
        Some((p, c, f))
    }

    fn page_rate(&self, f: &Frames) -> f64 {
        f.clock.rate(self.fps().as_f64())
    }

    fn page_speed(&self) -> f64 {
        if let Some(r) = self.effects_page.away.as_ref().and_then(|a| a.run) {
            return r.speed;
        }
        let p = self.snap.player(PlayerId::Timeline);
        if p.playing { p.speed } else { 0.0 }
    }

    fn page_source(&self) -> Option<(Arc<Project>, Clip)> {
        let p = self.project_shared()?;
        let c = p.clip(&self.effects_clip()?)?.clone();
        Some((p, c))
    }

    pub(super) fn page_clock(&self) -> Option<PageClock> {
        if self.effects_page.page != Page::Effects {
            return None;
        }
        let (_, c, f) = self.page_frames()?;
        let comp = stored_composition(&c);
        Some(PageClock {
            at: self.page_at(),
            first: f.first,
            last: f.last,
            mark_in: f.mark_in,
            out: f.mark_out,
            speed: self.page_speed(),
            looping: comp.loop_play,
            ping_pong: comp.ping_pong,
        })
    }

    pub(super) fn page_time(&mut self, t: PageTime) -> Result<(), String> {
        if self.effects_page.page != Page::Effects {
            return Err("Show the effects page first".into());
        }
        let (_, c, f) = self
            .page_frames()
            .ok_or("There is no clip at the playhead")?;
        if let Some(at) = t.at {
            self.page_go(&c, &f, at.clamp(f.first, f.last), t.fast);
        }
        if let Some(speed) = t.speed {
            self.page_play(&c, &f, speed);
        }
        Ok(())
    }

    fn page_ask(&self, params: Value) {
        self.send("gui.page_time", params);
    }

    fn page_go(&mut self, c: &Clip, f: &Frames, n: i64, fast: bool) {
        let away = self
            .effects_page
            .away
            .as_ref()
            .is_some_and(|a| a.clip == c.id);
        if f.inside(n) {
            let t = f.time(n);
            let held = f.at(t) != n;
            self.effects_page.away = (away || held).then(|| Away {
                clip: c.id.clone(),
                at: n as f64,
                run: None,
                landing: Some(Instant::now()),
                on: Some(t),
            });
            self.seek(PlayerId::Timeline, t, fast);
            return;
        }
        if self.snap.player(PlayerId::Timeline).playing {
            self.send("player.pause", json!({ "viewer": "timeline" }));
        }
        self.effects_page.away = Some(Away {
            clip: c.id.clone(),
            at: n as f64,
            run: None,
            landing: None,
            on: None,
        });
    }

    fn page_play(&mut self, c: &Clip, f: &Frames, speed: f64) {
        let at = self.page_at();
        if speed == 0.0 {
            let rate = self.page_rate(f);
            if let Some(a) = self.effects_page.away.as_mut()
                && let Some(r) = a.run.take()
            {
                a.at = if r.speed > 0.0 {
                    r.at(rate).min((f.mark_in - 1) as f64)
                } else {
                    r.at(rate).max((f.mark_out + 1) as f64)
                };
            }
            if self.snap.player(PlayerId::Timeline).playing {
                self.send("player.pause", json!({ "viewer": "timeline" }));
            }
            return;
        }
        let (head, tail) = (at < f.mark_in, at > f.mark_out);
        if (head && speed > 0.0) || (tail && speed < 0.0) {
            self.effects_page.away = Some(Away {
                clip: c.id.clone(),
                at: at as f64,
                run: Some(Run {
                    speed,
                    from: at as f64,
                    since: Instant::now(),
                }),
                landing: None,
                on: None,
            });
        } else if head {
            self.hand_off(c, f, f.mark_out, speed);
        } else if tail {
            self.hand_off(c, f, f.mark_in, speed);
        } else {
            self.effects_page.away = None;
            self.send(
                "player.play",
                json!({ "speed": speed, "viewer": "timeline" }),
            );
        }
    }

    fn hand_off(&mut self, c: &Clip, f: &Frames, n: i64, speed: f64) {
        let t = f.time(n);
        self.effects_page.away = Some(Away {
            clip: c.id.clone(),
            at: n as f64,
            run: None,
            landing: Some(Instant::now()),
            on: Some(t),
        });
        self.seek(PlayerId::Timeline, t, false);
        self.send(
            "player.play",
            json!({ "speed": speed, "viewer": "timeline" }),
        );
    }

    fn page_tick(&mut self, ctx: &egui::Context, p: &Project, c: &Clip) {
        let f = Frames::of(p, c);
        let rate = self.page_rate(&f);
        let player = self.snap.player(PlayerId::Timeline).clone();
        let Some(a) = self.effects_page.away.as_mut() else {
            return;
        };
        if a.clip != c.id {
            self.effects_page.away = None;
            return;
        }
        if let Some(since) = a.landing {
            let landed = a.on == Some(player.position);
            let exact = a.on.is_some_and(|t| f.at(t) == a.at as i64);
            if player.playing || (landed && exact) || since.elapsed().as_secs_f64() > SETTLE {
                self.effects_page.away = None;
            } else if landed {
                a.landing = None;
            } else {
                ctx.request_repaint();
            }
            return;
        }
        let Some(r) = a.run else {
            if a.on.is_some_and(|t| t != player.position) || player.playing {
                self.effects_page.away = None;
            }
            return;
        };
        ctx.request_repaint();
        let t = r.at(rate);
        let crossed = if r.speed > 0.0 {
            t >= f.mark_in as f64
        } else {
            t < (f.mark_out + 1) as f64
        };
        if !crossed {
            a.at = t;
            return;
        }
        let edge = if r.speed > 0.0 { f.mark_in } else { f.mark_out };
        self.hand_off(c, &f, edge, r.speed);
    }

    fn sync_bounds(&mut self) {
        let want = (self.effects_page.page == Page::Effects)
            .then(|| self.page_source())
            .flatten()
            .map(|(_, c)| {
                let comp = stored_composition(&c);
                ((c.start, c.end()), (comp.loop_play, comp.ping_pong))
            });
        if want == self.effects_page.bounds {
            return;
        }
        self.effects_page.bounds = want;
        let params = match want {
            Some(((from, to), (looping, ping_pong))) => json!({
                "from": from,
                "to": to,
                "loop": looping,
                "ping_pong": ping_pong,
            }),
            None => json!({}),
        };
        self.send("player.bounds", params);
    }

    fn time_ruler(&mut self, ui: &mut egui::Ui, p: Option<&Project>, clip: Option<&Clip>) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), RULER), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let colors = theme::colors();
        painter.rect_filled(rect, CornerRadius::same(2), colors.panel_dark);
        let (Some(p), Some(clip)) = (p, clip) else {
            ui.add_space(BAR);
            return;
        };
        let f = Frames::of(p, clip);
        let (first, last) = (f.first, f.last);
        let whole = (last + 1 - first).max(1) as f64;
        let (v0, v1) = self.effects_page.view;
        let a = first as f64 + v0 * whole;
        let span = ((v1 - v0) * whole).max(1.0);
        let width = f64::from(rect.width().max(1.0));
        let x_of = |t: f64| rect.left() + ((t - a) / span * width) as f32;
        let (xi, xo) = (x_of(f.mark_in as f64), x_of(f.mark_out as f64));
        let shade = Color32::from_black_alpha(110);
        if xi > rect.left() {
            let r = Rect::from_min_max(rect.min, pos2(xi.min(rect.right()), rect.bottom()));
            painter.rect_filled(r, CornerRadius::ZERO, shade);
        }
        if xo < rect.right() {
            let r = Rect::from_min_max(pos2(xo.max(rect.left()), rect.top()), rect.max);
            painter.rect_filled(r, CornerRadius::ZERO, shade);
        }
        let step = TICKS
            .into_iter()
            .find(|s| *s as f64 * width / span >= LABEL_GAP)
            .unwrap_or(TICKS[TICKS.len() - 1]);
        let minor = match step {
            1 => 1,
            s if s % 5 == 0 => s / 5,
            s => s / 2,
        };
        let mut n = (a / minor as f64).floor() as i64 * minor;
        let end = (a + span).ceil() as i64;
        while n <= end {
            let x = x_of(n as f64);
            let major = n % step == 0;
            let tall = if major { 8.0 } else { 4.0 };
            painter.line_segment(
                [pos2(x, rect.bottom() - tall), pos2(x, rect.bottom())],
                Stroke::new(1.0, colors.text_dim),
            );
            if major {
                painter.text(
                    pos2(x + 3.0, rect.top() + 2.0),
                    Align2::LEFT_TOP,
                    n.to_string(),
                    FontId::proportional(10.0),
                    colors.text_dim,
                );
            }
            n += minor;
        }
        for x in [xi, xo] {
            if rect.x_range().contains(x) {
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.5, RANGE),
                );
            }
        }
        if let Some(n) = self
            .effects_page
            .picked()
            .and_then(|name| stored_composition(clip).node(name).cloned())
        {
            for k in n.keys.iter().flat_map(|k| k.keys.iter()) {
                let x = x_of(k.frame as f64);
                if rect.x_range().contains(x) {
                    painter.line_segment(
                        [pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)],
                        Stroke::new(2.0, Color32::WHITE),
                    );
                }
            }
        }
        let x = x_of(self.page_at() as f64);
        if rect.x_range().contains(x) {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(2.0, PLAYHEAD),
            );
        }
        if resp.dragged_by(PointerButton::Middle) {
            let by = f64::from(resp.drag_delta().x) / width * (v1 - v0);
            self.effects_page.view = slid((v0, v1), -by);
        } else if (resp.clicked()
            || (resp.dragged_by(PointerButton::Primary)
                && (resp.drag_started() || resp.drag_delta().x != 0.0)))
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let t = (a + f64::from(pos.x - rect.left()) / width * span).round() as i64;
            let t = t.clamp(first, last);
            if t != self.page_at() {
                self.page_ask(json!({ "at": t, "fast": resp.dragged() }));
            }
        }
        self.zoom_bar(ui, whole);
    }

    fn zoom_bar(&mut self, ui: &mut egui::Ui, whole: f64) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), BAR), Sense::drag());
        let painter = ui.painter_at(rect);
        let colors = theme::colors();
        let width = f64::from(rect.width().max(1.0));
        let (v0, v1) = self.effects_page.view;
        let x_of = |v: f64| rect.left() + (v * width) as f32;
        let bar = Rect::from_min_max(
            pos2(x_of(v0), rect.top() + 2.0),
            pos2(x_of(v1), rect.bottom() - 2.0),
        );
        painter.rect_filled(
            bar,
            CornerRadius::same(3),
            colors.text_dim.gamma_multiply(0.45),
        );
        for x in [bar.left(), bar.right()] {
            let grip = Rect::from_center_size(pos2(x, bar.center().y), vec2(4.0, bar.height()));
            painter.rect_filled(grip, CornerRadius::same(1), colors.text_dim);
        }
        if resp.drag_started()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let near = |x: f32| (at.x - x).abs() <= GRIP;
            self.effects_page.zooming = if near(bar.left()) {
                Some(Zoom::Left)
            } else if near(bar.right()) {
                Some(Zoom::Right)
            } else if bar.x_range().contains(at.x) {
                Some(Zoom::Both)
            } else {
                None
            };
        }
        if resp.dragged()
            && let Some(z) = self.effects_page.zooming
        {
            let by = f64::from(resp.drag_delta().x) / width;
            let least = (MIN_FRAMES / whole).min(1.0);
            self.effects_page.view = match z {
                Zoom::Left => ((v0 + by).min(v1 - least).max(0.0), v1),
                Zoom::Right => (v0, (v1 + by).max(v0 + least).min(1.0)),
                Zoom::Both => slid((v0, v1), by),
            };
        }
        if resp.drag_stopped() {
            self.effects_page.zooming = None;
        }
    }

    fn page_transport(&mut self, ui: &mut egui::Ui, p: Option<&Project>, clip: Option<&Clip>) {
        ui.allocate_ui(vec2(ui.available_width(), TRANSPORT), |ui| {
            ui.horizontal_centered(|ui| {
                let (Some(p), Some(c)) = (p, clip) else {
                    return;
                };
                let f = Frames::of(p, c);
                let (first, last) = (f.first, f.last);
                let speed = self.page_speed();
                let comp = stored_composition(c);
                let big = |s: &str| RichText::new(s).size(16.0);
                let mut ask = None;
                if ui.button(big("⏮")).clicked() {
                    ask = Some(json!({ "at": first }));
                }
                if ui.selectable_label(speed < 0.0, big("◀")).clicked() {
                    ask = Some(json!({ "speed": -1.0 }));
                }
                if ui.button(big("⏹")).clicked() {
                    ask = Some(json!({ "speed": 0.0 }));
                }
                if ui.selectable_label(speed > 0.0, big("▶")).clicked() {
                    ask = Some(json!({ "speed": 1.0 }));
                }
                if ui.button(big("⏭")).clicked() {
                    ask = Some(json!({ "at": last }));
                }
                let icon = match comp.ping_pong {
                    true => egui_phosphor::regular::ARROWS_LEFT_RIGHT,
                    false => egui_phosphor::regular::REPEAT,
                };
                let r = ui.selectable_label(comp.loop_play, big(icon));
                let mut looped = None;
                if r.clicked() {
                    looped = Some(json!({ "on": !comp.loop_play }));
                }
                r.context_menu(|ui| {
                    for (label, pp) in [("Loop", false), ("Ping Pong", true)] {
                        if ui.radio(comp.ping_pong == pp, label).clicked() {
                            looped = Some(json!({ "ping_pong": pp }));
                            ui.close();
                        }
                    }
                });
                if let Some(mut params) = looped {
                    params["clip"] = json!(c.id);
                    self.send("comp.loop", params);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut n = self.page_at();
                    let field = egui::DragValue::new(&mut n).range(first..=last).speed(0.25);
                    let r = ui.add(field);
                    if r.changed() {
                        ask = Some(json!({ "at": n, "fast": r.dragged() }));
                    }
                });
                if let Some(params) = ask {
                    self.page_ask(params);
                }
            });
        });
    }

    fn tool_row(&mut self, ui: &mut egui::Ui) {
        ui.allocate_ui(vec2(ui.available_width(), TOOLS), |ui| {
            ui.horizontal_centered(|ui| {
                for (i, group) in TOOL_ROW.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    for tool in *group {
                        if ui.button(tool.label()).clicked() {
                            self.add_node(*tool);
                        }
                    }
                }
            });
        });
    }

    fn node_editor(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let c = theme::colors();
        painter.rect_filled(rect, CornerRadius::same(2), c.panel_dark);
        let p = self.project_shared();
        let clip = self
            .effects_clip()
            .and_then(|id| p.as_ref().and_then(|p| p.clip(&id)).cloned());
        let Some(clip) = clip else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No clip at the playhead",
                FontId::proportional(14.0),
                c.text_dim,
            );
            return;
        };
        if resp.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if zoom != 1.0 {
                self.effects_page.zoom = (self.effects_page.zoom * zoom).clamp(ZOOMS.0, ZOOMS.1);
            } else {
                self.effects_page.pan += scroll;
            }
        }
        if resp.dragged_by(PointerButton::Middle) {
            self.effects_page.pan += resp.drag_delta();
        }
        let comp = self.page_composition(&clip);
        let z = self.effects_page.zoom;
        let origin = rect.min + MARGIN * z + self.effects_page.pan;
        let grid_at = |at: Pos2| {
            let d = (at - origin) / (CELL * z);
            [rounded(f64::from(d.x)), rounded(f64::from(d.y))]
        };
        let moving = self.effects_page.moving.as_ref();
        let place = |n: &Node| -> [f64; 2] {
            let from = moving.and_then(|m| {
                let (_, at) = m.from.iter().find(|(name, _)| *name == n.name)?;
                Some((m.by, *at))
            });
            match from {
                Some((by, at)) => [
                    at[0] + f64::from(by.x / (CELL.x * z)),
                    at[1] + f64::from(by.y / (CELL.y * z)),
                ],
                None => n.place,
            }
        };
        let boxes: Vec<(String, Rect)> = comp
            .nodes
            .iter()
            .map(|n| {
                let [x, y] = place(n);
                let at = origin + vec2(x as f32 * CELL.x, y as f32 * CELL.y) * z;
                (n.name.clone(), Rect::from_min_size(at, BOX * z))
            })
            .collect();
        let box_of = |name: &str| boxes.iter().find(|(n, _)| n == name).map(|(_, r)| *r);
        let mut knots: Vec<(End, Pos2)> = Vec::new();
        for n in &comp.nodes {
            let Some(r) = box_of(&n.name) else {
                continue;
            };
            if !matches!(n.tool, Tool::MediaOut) {
                knots.push((End::Output(n.name.clone()), r.right_center()));
            }
            for which in [NodeInput::Input, NodeInput::EffectMask] {
                let takes = match which {
                    NodeInput::Input => n.tool.takes_input(),
                    NodeInput::EffectMask => n.tool.takes_mask(),
                };
                if takes {
                    knots.push((End::Input(n.name.clone(), which), inlet(r, which)));
                }
            }
        }
        let knot_rect = |at: Pos2| Rect::from_center_size(at, Vec2::splat(KNOT * 3.0));
        let spot_at = |at: Pos2| -> Spot {
            if let Some((end, _)) = knots.iter().find(|(_, k)| knot_rect(*k).contains(at)) {
                return Spot::Knot(end.clone());
            }
            match boxes.iter().find(|(_, r)| r.contains(at)) {
                Some((name, _)) => Spot::Tile(name.clone()),
                None => Spot::Empty,
            }
        };
        let wires = joins(&comp);
        let pulled = self
            .effects_page
            .linking
            .as_ref()
            .and_then(|l| l.pulled.clone());
        let curve = |w: &Wire| -> Option<[Pos2; 4]> {
            let from = box_of(&w.from)?.right_center();
            let to = inlet(box_of(&w.to)?, w.which);
            Some(bezier((from, to), along(w.which)))
        };
        let wire_at = |at: Pos2| -> Option<(usize, Half)> {
            if boxes.iter().any(|(_, r)| r.expand(KNOT * 2.0).contains(at)) {
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
        let busy = self.effects_page.linking.is_some()
            || self.effects_page.band.is_some()
            || self.effects_page.moving.is_some();
        let pointer = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|at| rect.contains(*at));
        let near = pointer.filter(|_| !busy).and_then(wire_at);
        let picture_line = Stroke::new(1.5, c.text_dim);
        let mask_line = Stroke::new(1.5, Color32::from_rgb(90, 140, 230));
        let lit = Stroke::new(2.5, Color32::from_rgb(110, 170, 255));
        for (k, w) in wires.iter().enumerate() {
            if pulled.as_ref() == Some(&(w.to.clone(), w.which)) {
                continue;
            }
            let Some(b) = curve(w) else {
                continue;
            };
            let line = match w.which {
                NodeInput::Input => picture_line,
                NodeInput::EffectMask => mask_line,
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
        let mut picked: Option<(String, Pick)> = None;
        let mut moved: Vec<(String, [f64; 2])> = Vec::new();
        let mut menu: Option<(MenuAct, String)> = None;
        let mut pointed: Option<String> = None;
        let one_copied = self.effects_page.copied.len() == 1;
        for n in &comp.nodes {
            let Some(r) = box_of(&n.name) else {
                continue;
            };
            let id = ui.scope_id().with(("effects-node", &clip.id, &n.name));
            let node_resp = ui.interact(r, id, Sense::click_and_drag());
            let selected = self.effects_page.nodes.contains(&n.name);
            let active = self.effects_page.picked() == Some(n.name.as_str());
            let fade = if n.pass_through { 0.45 } else { 1.0 };
            let fill = c.panel.gamma_multiply(fade);
            painter.rect_filled(r, CornerRadius::same(4), fill);
            let edge = match (active, selected) {
                (true, _) => Stroke::new(2.0, c.accent),
                (false, true) => Stroke::new(1.5, Color32::WHITE),
                _ => Stroke::new(1.0, c.text_dim.gamma_multiply(0.6)),
            };
            painter.rect_stroke(r, CornerRadius::same(4), edge, StrokeKind::Inside);
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                &n.name,
                FontId::proportional(12.0 * z.clamp(0.6, 1.4)),
                c.text.gamma_multiply(fade),
            );
            let used = comp.nodes.iter().any(|x| {
                x.input.as_deref() == Some(&n.name) || x.effect_mask.as_deref() == Some(&n.name)
            });
            if !matches!(n.tool, Tool::MediaOut) {
                let out = if used { Color32::WHITE } else { c.text_dim };
                painter.rect_filled(
                    Rect::from_center_size(r.right_center(), Vec2::splat(KNOT * 2.0)),
                    CornerRadius::ZERO,
                    out,
                );
            }
            if n.tool.takes_input() {
                knot(
                    &painter,
                    r.left_center(),
                    vec2(1.0, 0.0),
                    Color32::from_rgb(230, 200, 60),
                );
            }
            if n.tool.takes_mask() {
                knot(
                    &painter,
                    r.center_top(),
                    vec2(0.0, 1.0),
                    Color32::from_rgb(90, 140, 230),
                );
            }
            if node_resp.clicked() {
                let (add, alt) = ui.input(|i| (i.modifiers.command, i.modifiers.alt));
                let how = match (alt && selected, add) {
                    (true, _) => Pick::Active,
                    (false, true) => Pick::Toggle,
                    (false, false) => Pick::Only,
                };
                picked = Some((n.name.clone(), how));
            }
            if node_resp.secondary_clicked() && !selected {
                pointed = Some(n.name.clone());
            }
            let tool = n.tool.flows() || n.tool.is_mask();
            node_resp.context_menu(|ui| {
                let items = [
                    (MenuAct::Rename, "Rename", true),
                    (MenuAct::PassThrough, "Pass Through", tool),
                    (MenuAct::Copy, "Copy", tool),
                    (MenuAct::Paste, "Paste", one_copied),
                    (MenuAct::Delete, "Delete", true),
                ];
                for (act, text, on) in items {
                    let item = match act {
                        MenuAct::PassThrough => {
                            ui.add_enabled(on, egui::Button::selectable(n.pass_through, text))
                        }
                        _ => ui.add_enabled(on, egui::Button::new(text)),
                    };
                    if item.clicked() {
                        menu = Some((act, n.name.clone()));
                        ui.close();
                    }
                }
            });
            if node_resp.drag_started_by(PointerButton::Primary) {
                let group = match selected {
                    true => self.effects_page.nodes.clone(),
                    false => vec![n.name.clone()],
                };
                let from = comp
                    .nodes
                    .iter()
                    .filter(|g| group.contains(&g.name))
                    .map(|g| (g.name.clone(), g.place))
                    .collect();
                self.effects_page.moving = Some(Moving {
                    node: n.name.clone(),
                    from,
                    by: Vec2::ZERO,
                });
            }
            if node_resp.dragged_by(PointerButton::Primary)
                && let Some(m) = self
                    .effects_page
                    .moving
                    .as_mut()
                    .filter(|m| m.node == n.name)
            {
                m.by += node_resp.drag_delta();
            }
            if node_resp.drag_stopped()
                && let Some(m) = self.effects_page.moving.take().filter(|m| m.node == n.name)
            {
                for (name, from) in &m.from {
                    let to = [
                        rounded(from[0] + f64::from(m.by.x / (CELL.x * z))),
                        rounded(from[1] + f64::from(m.by.y / (CELL.y * z))),
                    ];
                    if to != *from {
                        moved.push((name.clone(), to));
                    }
                }
            }
        }
        let mut started: Option<Linking> = None;
        for (k, (end, at)) in knots.iter().enumerate() {
            let id = ui.scope_id().with(("effects-knot", &clip.id, k));
            let knot_resp = ui
                .interact(knot_rect(*at), id, Sense::drag())
                .on_hover_text(knot_name(end));
            if !knot_resp.drag_started_by(PointerButton::Primary) {
                continue;
            }
            let from = match end {
                End::Input(dst, which) => comp.node(dst).and_then(|n| joined(n, *which)),
                End::Output(_) => None,
            };
            started = Some(match (end, from) {
                (End::Input(dst, which), Some(src)) => Linking {
                    anchor: End::Output(src),
                    pulled: Some((dst.clone(), *which)),
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
                        pulled: Some((w.to.clone(), w.which)),
                    });
                }
                Some((w, Half::Output)) => {
                    started = Some(Linking {
                        anchor: End::Input(w.to.clone(), w.which),
                        pulled: Some((w.to.clone(), w.which)),
                    });
                }
                None => {
                    let off = ui.input(|i| i.modifiers.command);
                    self.effects_page.band = Some((press, off));
                }
            }
        }
        if started.is_some() {
            self.effects_page.linking = started;
        }
        let released = ui.input(|i| i.pointer.primary_released());
        let at = ui.input(|i| i.pointer.interact_pos());
        if let Some(l) = &self.effects_page.linking
            && let Some(to) = at
        {
            let from = match &l.anchor {
                End::Output(s) => box_of(s).map(|r| r.right_center()),
                End::Input(d, w) => box_of(d).map(|r| inlet(r, *w)),
            };
            if let Some(from) = from {
                painter.line_segment([from, to], lit);
            }
        }
        let mut commands = Vec::new();
        if released && let Some(l) = self.effects_page.linking.take() {
            let spot = at.map_or(Spot::Empty, spot_at);
            commands = linked(&comp, &clip.id, &l, spot);
        }
        if let Some((start, off)) = self.effects_page.band
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
                self.effects_page.band = None;
                let inside: Vec<String> = boxes
                    .iter()
                    .filter(|(_, r)| band.intersects(*r))
                    .map(|(n, _)| n.clone())
                    .collect();
                let mut nodes = self.effects_page.nodes.clone();
                if off {
                    nodes.retain(|n| !inside.contains(n));
                } else {
                    nodes = inside;
                }
                self.send("gui.nodes", json!({ "nodes": nodes }));
            }
        }
        if released {
            self.effects_page.band = None;
        }
        if resp.secondary_clicked()
            && let Some(at) = ui.input(|i| i.pointer.interact_pos())
        {
            self.effects_page.spot = Some((clip.id.clone(), grid_at(at)));
        }
        let can_paste = !self.effects_page.copied.is_empty();
        let mut paste_here = false;
        resp.context_menu(|ui| {
            if ui
                .add_enabled(can_paste, egui::Button::new("Paste"))
                .clicked()
            {
                paste_here = true;
                ui.close();
            }
        });
        if paste_here && let Some((_, at)) = self.effects_page.spot.clone() {
            self.paste_to(json!({ "place": at }));
        }
        self.send_all(commands, "Join Nodes");
        let moves = moved
            .into_iter()
            .map(|(name, to)| {
                let params = json!({ "clip": clip.id, "node": name, "place": to });
                command("comp.set", params)
            })
            .collect();
        self.send_all(moves, "Move Nodes");
        if let Some(name) = pointed {
            self.effects_page.nodes = vec![name];
        }
        if let Some((act, node)) = menu {
            self.menu_act(act, node);
        }
        match picked {
            Some((name, Pick::Active)) => {
                let mut nodes = self.effects_page.nodes.clone();
                nodes.retain(|n| *n != name);
                nodes.push(name);
                self.send("gui.nodes", json!({ "nodes": nodes }));
            }
            Some((name, Pick::Toggle)) => {
                let mut nodes = self.effects_page.nodes.clone();
                match nodes.iter().position(|n| *n == name) {
                    Some(i) => {
                        nodes.remove(i);
                    }
                    None => nodes.push(name),
                }
                self.send("gui.nodes", json!({ "nodes": nodes }));
            }
            Some((name, Pick::Only)) => self.send("gui.nodes", json!({ "nodes": [name] })),
            None if resp.clicked() => {
                let press = ui.input(|i| i.pointer.interact_pos());
                if let Some(at) = press {
                    self.effects_page.spot = Some((clip.id.clone(), grid_at(at)));
                }
                match press.and_then(wire_at).map(|(k, half)| (&wires[k], half)) {
                    Some((w, Half::Input)) => {
                        let off = join_command(&clip.id, (&w.to, w.which), None);
                        self.send_all(vec![off], "Join Nodes");
                    }
                    _ if !self.effects_page.nodes.is_empty() => {
                        self.send("gui.nodes", json!({ "nodes": [] }));
                    }
                    _ => {}
                }
            }
            None => {}
        }
    }

    pub(super) fn effects_keys(&mut self, fired: &Fired, input: &egui::InputState) {
        let bindings = self.snap.bindings.clone();
        let mine = Fired::of_panel(&bindings, input, Panel::Effects);
        if mine.pressed(Action::PassThrough) {
            self.pass_through();
        }
        if mine.struck(Action::RenameNode) {
            self.start_rename();
        }
        if mine.pressed(Action::PreviousNodeKeyframe) {
            self.node_key_step(-1);
        }
        if mine.pressed(Action::NextNodeKeyframe) {
            self.node_key_step(1);
        }
        if fired.struck(Action::Delete) {
            self.delete_nodes(self.effects_page.nodes.clone());
        }
        if fired.struck(Action::Copy) {
            self.copy_node(false);
        }
        if fired.struck(Action::Cut) {
            self.copy_node(true);
        }
        if fired.pressed(Action::Paste) {
            self.paste_node();
        }
        if fired.pressed(Action::SelectAll) {
            self.select_all(true);
        }
        if fired.pressed(Action::SelectNothing) {
            self.select_all(false);
        }
        if input.key_pressed(egui::Key::Escape) {
            self.stop_tracks();
        }
        let Some((_, _, f)) = self.page_frames() else {
            return;
        };
        let (first, last) = (f.first, f.last);
        let fps = self.page_rate(&f).round().max(1.0) as i64;
        let speed = self.page_speed();
        let at = self.page_at();
        let to = |t: i64| json!({ "at": t.clamp(first, last) });
        let mut asks = Vec::new();
        if fired.pressed(Action::PlayOrStop) {
            asks.push(json!({ "speed": if speed == 0.0 { 1.0 } else { 0.0 } }));
        }
        if fired.struck(Action::Stop) {
            asks.push(json!({ "speed": 0.0 }));
        }
        for jkl in self.shuttle.input(&bindings, input) {
            asks.push(match jkl {
                Jkl::Play(dir) => {
                    json!({ "speed": crate::gui::timeline::tools::faster(speed, dir) })
                }
                Jkl::Step(dir) => to(at + dir as i64),
                Jkl::Slow(s) => json!({ "speed": s }),
                Jkl::Stop => json!({ "speed": 0.0 }),
            });
        }
        let moves = [
            (fired.pressed(Action::FrameBack), at - 1),
            (fired.pressed(Action::FrameForward), at + 1),
            (
                fired.pressed(Action::SecondBack) && !mine.pressed(Action::SourceStart),
                at - fps,
            ),
            (
                fired.pressed(Action::SecondForward) && !mine.pressed(Action::SourceEnd),
                at + fps,
            ),
            (mine.pressed(Action::SourceStart), first),
            (mine.pressed(Action::SourceEnd), last),
            (mine.pressed(Action::ClipIn), f.mark_in),
            (mine.pressed(Action::ClipOut), f.mark_out),
        ];
        for (_, t) in moves.into_iter().filter(|(on, _)| *on) {
            asks.push(to(t));
        }
        for params in asks {
            self.page_ask(params);
        }
    }
}

fn slid((v0, v1): (f64, f64), by: f64) -> (f64, f64) {
    let by = by.max(-v0).min(1.0 - v1);
    (v0 + by, v1 + by)
}

fn clip_name(p: &Project, c: &Clip) -> String {
    c.name
        .clone()
        .or_else(|| p.media(&c.media).map(|m| m.name.clone()))
        .unwrap_or_else(|| c.id.clone())
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
