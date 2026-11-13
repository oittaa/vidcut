use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, PointerButton, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2, pos2, vec2,
};
use serde_json::{Value, json};
use vidcut_core::effect_keys::Keyed;
use vidcut_core::model::{Clip, Composition, EffectValue, Mask, Node, Project, Tool, ToolType};
use vidcut_core::place::{Outline, Shaping};
use vidcut_core::render::RenderPlan;
use vidcut_core::request::{NodeInput, Page};
use vidcut_core::shortcuts::{Action, Panel};

use super::shortcuts::Fired;
use super::shuttle::Jkl;
use super::theme::{self, Deco};
use super::workspace::{Workspace, dim, fit};
use crate::live::{Answer, PlayerId};

const CELL: Vec2 = vec2(132.0, 64.0);
const BOX: Vec2 = vec2(104.0, 30.0);
const MARGIN: Vec2 = vec2(40.0, 40.0);
const KNOT: f32 = 5.0;
const GRIP: f32 = 9.0;
const HANDLE: f32 = 4.0;
const RULER: f32 = 24.0;
const TOOLS: f32 = 30.0;
const TRANSPORT: f32 = 36.0;
const SETTLE: f64 = 2.0;
const SCREEN: usize = 0;
const ZOOMS: (f32, f32) = (0.3, 3.0);
const ROUND: f64 = 100.0;
const WIRE_HIT: f32 = 6.0;
const STEPS: usize = 24;
const LIBRARY: [(&str, &[ToolType]); 3] = [
    ("Tools", &[ToolType::Blur]),
    (
        "Masks",
        &[ToolType::Rectangle, ToolType::Ellipse, ToolType::Polygon],
    ),
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
    pan: Vec2,
    zoom: f32,
    moving: Option<Moving>,
    asked: Vec<(Answer, Then)>,
    linking: Option<Linking>,
    band: Option<(Pos2, bool)>,
    renaming: Option<Renaming>,
    copied: Option<Node>,
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
            pan: Vec2::ZERO,
            zoom: 1.0,
            moving: None,
            asked: Vec::new(),
            linking: None,
            band: None,
            renaming: None,
            copied: None,
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
        self.moving = None;
        self.linking = None;
        self.band = None;
        self.renaming = None;
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

struct Held {
    clip: String,
    node: String,
    grip: Grip,
    composition: Composition,
    sent: Option<(u64, Instant)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Center,
    Width,
    Height,
    Point(usize),
}

struct Moving {
    node: String,
    from: [f64; 2],
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

fn turned([x, y]: [f64; 2], turn: f64) -> [f64; 2] {
    let (s, c) = turn.sin_cos();
    [x * c + y * s, -x * s + y * c]
}

fn plus(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn rounded(x: f64) -> f64 {
    (x * ROUND).round() / ROUND
}

fn outline_points(s: &Shaping) -> Vec<[f64; 2]> {
    match &s.outline {
        Outline::Ellipse { half, turn } => (0..=72)
            .map(|i| {
                let a = f64::from(i) / 72.0 * std::f64::consts::TAU;
                plus(
                    s.middle,
                    turned([half[0] * a.cos(), half[1] * a.sin()], *turn),
                )
            })
            .collect(),
        Outline::Rectangle { half, turn, .. } => [
            [-1.0, -1.0],
            [1.0, -1.0],
            [1.0, 1.0],
            [-1.0, 1.0],
            [-1.0, -1.0],
        ]
        .iter()
        .map(|[x, y]| plus(s.middle, turned([x * half[0], y * half[1]], *turn)))
        .collect(),
        Outline::Polygon { points, .. } => {
            let mut out = points.clone();
            if let Some(first) = points.first() {
                out.push(*first);
            }
            out
        }
    }
}

fn grips(s: &Shaping) -> Vec<(Grip, [f64; 2])> {
    let mut out = vec![(Grip::Center, s.middle)];
    match &s.outline {
        Outline::Ellipse { half, turn } | Outline::Rectangle { half, turn, .. } => {
            out.push((Grip::Width, plus(s.middle, turned([half[0], 0.0], *turn))));
            out.push((Grip::Height, plus(s.middle, turned([0.0, -half[1]], *turn))));
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

fn gripped(m: &mut Mask, tool: &Tool, grip: Grip, at: [f64; 2], (w, h): (f64, f64)) {
    let s = Shaping::of(m, tool, (w, h));
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
                    let side = if ellipse { w } else { h };
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
        Grip::Point(_) => ("points", json!(m.points)),
    }
}

fn grip_value(grip: Grip) -> Option<EffectValue> {
    match grip {
        Grip::Center => Some(EffectValue::Center),
        Grip::Width => Some(EffectValue::Width),
        Grip::Height => Some(EffectValue::Height),
        Grip::Point(_) => None,
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
        let source = clip.source_at(self.snap.timeline.position) as f64;
        self.node_preview(&mut c, source);
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
            self.leave_inspector();
        }
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
            let Some(node) = r
                .ok()
                .and_then(|r| r.result["node"].as_str().map(str::to_owned))
            else {
                continue;
            };
            self.effects_page.nodes = match then {
                Then::Select => vec![node],
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

    fn delete_nodes(&mut self, only_active: bool) {
        let Some((clip, comp)) = self.page_clip() else {
            return;
        };
        let gone: Vec<String> = match only_active {
            true => self
                .effects_page
                .picked()
                .map(str::to_owned)
                .into_iter()
                .collect(),
            false => self.effects_page.nodes.clone(),
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
            .filter(|n| n.tool.works() || n.tool.is_mask())
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
        let Some(n) = self.effects_page.picked().and_then(|n| comp.node(n)) else {
            return;
        };
        if !(n.tool.works() || n.tool.is_mask()) {
            return;
        }
        self.effects_page.copied = Some(n.clone());
        if cut {
            self.delete_nodes(true);
        }
    }

    fn paste_node(&mut self) {
        let (Some(clip), Some(copy)) = (self.effects_clip(), self.effects_page.copied.clone())
        else {
            return;
        };
        let mut params = json!({ "clip": clip, "copy": copy });
        if let Some(after) = self.effects_page.picked() {
            params["after"] = json!(after);
        }
        let answer = self.live.request_answered("comp.paste", params);
        self.effects_page.asked.push((answer, Then::Select));
    }

    fn node_key_step(&mut self, dir: i64) {
        let Some((clip, comp)) = self.page_clip() else {
            return;
        };
        let Some(n) = self.effects_page.picked().and_then(|n| comp.node(n)) else {
            return;
        };
        let times = n
            .keys
            .iter()
            .flat_map(|k| k.keys.iter())
            .map(|k| clip.key_time(k.frame).round() as i64);
        if let Some(t) = super::inspector::next_key(times, self.snap.timeline.position, dir) {
            self.seek(PlayerId::Timeline, t, false);
        }
    }

    fn select_all(&mut self, on: bool) {
        self.effects_page.nodes = match (on, self.page_clip()) {
            (true, Some((_, comp))) => comp.nodes.iter().map(|n| n.name.clone()).collect(),
            _ => Vec::new(),
        };
    }

    fn menu_act(&mut self, act: MenuAct) {
        match act {
            MenuAct::Rename => self.start_rename(),
            MenuAct::PassThrough => self.pass_through(),
            MenuAct::Copy => self.copy_node(false),
            MenuAct::Paste => self.paste_node(),
            MenuAct::Delete => self.delete_nodes(false),
        }
    }

    pub(super) fn effects_page_ui(&mut self, ui: &mut egui::Ui) {
        self.settle_page(ui.ctx());
        self.rename_window(ui.ctx());
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
        ui.horizontal(|ui| {
            ui.label(RichText::new(vidcut_core::composition::MEDIA_OUT).strong());
            if let (Some(p), Some(c)) = (&p, &clip) {
                ui.label(dim(clip_name(p, c)));
            }
        });
        let avail = ui.available_rect_before_wrap();
        let below = RULER + TRANSPORT + TOOLS + 12.0;
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
        self.time_ruler(ui, clip.as_ref());
        self.transport(ui, PlayerId::Timeline, None);
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
        let player = self.snap.player(PlayerId::Timeline).clone();
        let speed = if player.playing { player.speed } else { 0.0 };
        let frame = (player.position - c.start).clamp(0, (c.end() - c.start - 1).max(0));
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
        self.mask_handles(ui, area_resp, picture, c, &comp, size);
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
        let Some(name) = self.effects_page.picked().map(str::to_owned) else {
            return;
        };
        let Some(node) = comp.node(&name) else {
            return;
        };
        let at = self.snap.timeline.position;
        let node = node.sampled(clip.source_at(at) as f64);
        let Some(m) = node.tool.mask() else {
            return;
        };
        let (w, h) = (f64::from(w), f64::from(h));
        let scale = f64::from(picture.width()) / w;
        let screen = |[x, y]: [f64; 2]| picture.min + vec2((x * scale) as f32, (y * scale) as f32);
        let pixel = |p: Pos2| {
            let d = p - picture.min;
            [f64::from(d.x) / scale, f64::from(d.y) / scale]
        };
        let shaping = Shaping::of(m, &node.tool, (w, h));
        let line: Vec<Pos2> = outline_points(&shaping).into_iter().map(screen).collect();
        let painter = ui.painter_at(resp.rect);
        let accent = theme::colors().accent;
        painter.add(egui::Shape::line(line, Stroke::new(1.5, accent)));
        let spots = grips(&shaping);
        for (grip, spot) in &spots {
            let s = screen(*spot);
            match grip {
                Grip::Center => {
                    painter.line_segment(
                        [s - vec2(6.0, 0.0), s + vec2(6.0, 0.0)],
                        Stroke::new(1.5, accent),
                    );
                    painter.line_segment(
                        [s - vec2(0.0, 6.0), s + vec2(0.0, 6.0)],
                        Stroke::new(1.5, accent),
                    );
                }
                _ => {
                    painter.rect_filled(
                        Rect::from_center_size(s, Vec2::splat(HANDLE * 2.0)),
                        CornerRadius::ZERO,
                        accent,
                    );
                }
            }
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let near = spots
                .iter()
                .map(|(g, s)| (*g, screen(*s).distance(press)))
                .filter(|(_, d)| *d <= GRIP)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((grip, _)) = near {
                self.effects_page.held = Some(Held {
                    clip: clip.id.clone(),
                    node: name.clone(),
                    grip,
                    composition: comp.clone(),
                    sent: None,
                });
            }
        }
        let source = clip.source_at(at) as f64;
        let rev = self.project().map_or(0, |p| p.rev);
        let held = self
            .effects_page
            .held
            .as_mut()
            .filter(|x| x.sent.is_none() && x.clip == clip.id && x.node == name);
        let Some(held) = held else {
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
            if let (Some(m), Some(now)) = (n.tool.mask_mut(), tool.mask()) {
                *m = now.clone();
                gripped(m, &tool, held.grip, pixel(to), (w, h));
            }
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
            let mut params = json!({ "clip": clip.id, "node": name, "at": at });
            params[key] = value;
            self.send("comp.set", params);
        }
    }

    fn time_ruler(&mut self, ui: &mut egui::Ui, clip: Option<&Clip>) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), RULER), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let c = theme::colors();
        painter.rect_filled(rect, CornerRadius::same(2), c.panel_dark);
        let Some(clip) = clip else {
            return;
        };
        let (a, b) = (clip.start, clip.end().max(clip.start + 1));
        let span = (b - a) as f32;
        let x_of = |t: i64| rect.left() + (t - a) as f32 / span * rect.width();
        let fps = self.fps().as_f64().round().max(1.0) as i64;
        let step = [1, 5, 10, fps, 5 * fps, 10 * fps, 30 * fps, 60 * fps]
            .into_iter()
            .find(|s| span / (*s as f32) * 8.0 <= rect.width())
            .unwrap_or(60 * fps)
            .max(1);
        let mut t = a + (step - (a % step)) % step;
        while t < b {
            let x = x_of(t);
            painter.line_segment(
                [pos2(x, rect.bottom() - 6.0), pos2(x, rect.bottom())],
                Stroke::new(1.0, c.text_dim),
            );
            t += step;
        }
        if let Some(n) = self
            .effects_page
            .picked()
            .and_then(|name| stored_composition(clip).node(name).cloned())
        {
            for k in n.keys.iter().flat_map(|k| k.keys.iter()) {
                let x = x_of(clip.key_time(k.frame).round() as i64);
                painter.line_segment(
                    [pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)],
                    Stroke::new(2.0, Color32::WHITE),
                );
            }
        }
        let at = self.snap.timeline.position;
        let x = x_of(at);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, Color32::from_rgb(230, 60, 60)),
        );
        if (resp.clicked() || resp.dragged())
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let f = a + ((pos.x - rect.left()) / rect.width() * span).round() as i64;
            self.seek(PlayerId::Timeline, f.clamp(a, b - 1), resp.dragged());
        }
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
        let moving = self.effects_page.moving.as_ref();
        let place = |n: &Node| -> [f64; 2] {
            match moving.filter(|m| m.node == n.name) {
                Some(m) => [
                    m.from[0] + f64::from(m.by.x / (CELL.x * z)),
                    m.from[1] + f64::from(m.by.y / (CELL.y * z)),
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
        let mut picked: Option<(String, bool)> = None;
        let mut moved: Option<(String, [f64; 2])> = None;
        let mut menu: Option<MenuAct> = None;
        let mut pointed: Option<String> = None;
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
                let add = ui.input(|i| i.modifiers.command);
                picked = Some((n.name.clone(), add));
            }
            if node_resp.secondary_clicked() && !selected {
                pointed = Some(n.name.clone());
            }
            let tool = n.tool.works() || n.tool.is_mask();
            node_resp.context_menu(|ui| {
                let items = [
                    (MenuAct::Rename, "Rename", true),
                    (MenuAct::PassThrough, "Pass Through", tool),
                    (MenuAct::Copy, "Copy", tool),
                    (MenuAct::Paste, "Paste", true),
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
                        menu = Some(act);
                        ui.close();
                    }
                }
            });
            if node_resp.drag_started_by(PointerButton::Primary) {
                self.effects_page.moving = Some(Moving {
                    node: n.name.clone(),
                    from: n.place,
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
                let to = [
                    rounded(m.from[0] + f64::from(m.by.x / (CELL.x * z))),
                    rounded(m.from[1] + f64::from(m.by.y / (CELL.y * z))),
                ];
                if to != m.from {
                    moved = Some((n.name.clone(), to));
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
        self.send_all(commands, "Join Nodes");
        if let Some((name, to)) = moved {
            self.send(
                "comp.set",
                json!({ "clip": clip.id, "node": name, "place": to }),
            );
        }
        if let Some(name) = pointed {
            self.effects_page.nodes = vec![name];
        }
        if let Some(act) = menu {
            self.menu_act(act);
        }
        match picked {
            Some((name, true)) => {
                let mut nodes = self.effects_page.nodes.clone();
                match nodes.iter().position(|n| *n == name) {
                    Some(i) => {
                        nodes.remove(i);
                    }
                    None => nodes.push(name),
                }
                self.send("gui.nodes", json!({ "nodes": nodes }));
            }
            Some((name, false)) => self.send("gui.nodes", json!({ "nodes": [name] })),
            None if resp.clicked() => {
                let press = ui.input(|i| i.pointer.interact_pos());
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
        let player = self.snap.player(PlayerId::Timeline).clone();
        let fps = self.fps().as_f64().round().max(1.0) as i64;
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
            self.delete_nodes(false);
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
        if fired.pressed(Action::PlayOrStop) {
            self.send("player.toggle", json!({}));
        }
        if fired.struck(Action::Stop) {
            self.send("player.pause", json!({}));
        }
        for jkl in self.shuttle.input(&bindings, input) {
            match jkl {
                Jkl::Play(dir) => {
                    let speed = crate::gui::timeline::tools::faster(player.speed, dir);
                    self.send("player.play", json!({ "speed": speed }));
                }
                Jkl::Step(dir) => self.send("player.step", json!({ "frames": dir as i64 })),
                Jkl::Slow(speed) => self.send("player.play", json!({ "speed": speed })),
                Jkl::Stop => self.send("player.pause", json!({})),
            }
        }
        for (action, frames) in [
            (Action::FrameBack, -1),
            (Action::FrameForward, 1),
            (Action::SecondBack, -fps),
            (Action::SecondForward, fps),
        ] {
            if fired.pressed(action) {
                self.send("player.step", json!({ "frames": frames }));
            }
        }
    }
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
