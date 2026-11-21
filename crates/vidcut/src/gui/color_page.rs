use std::time::{Duration, Instant};

use egui::{Color32, CursorIcon, DragValue, Label, Rect, Response, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as icon;
use serde_json::{Map, Value, json};
use vidcut_core::model::{BlendMode, Corrector, Grade, GradeNode, GradeTool, KeyOutput, Picture};
use vidcut_core::ops::grade::{GradeOf, checked, corrector_checked, printed};
use vidcut_core::ops::picture::takes_effects;
use vidcut_core::preferences::SwitchingClips;
use vidcut_core::request::{
    GradeAdd, GradeLevel, GradeMixer, GradePalette, NodeZoom, Page, PrinterColor,
};
use vidcut_core::shortcuts::{Action, Panel};

use super::node_editor::{
    self, Badge, Dropped, Edge, End, Graph, Inlet, Link, Outlet, Pick, RGB_KNOT, Side, Spot, Tile,
    Wire,
};
use super::theme;
use super::workspace::Workspace;

const WHEEL: f32 = 132.0;

const OUTER: f32 = WHEEL / 2.0 - 4.0;

const INNER: f32 = OUTER - 8.0;

const DIAL: Vec2 = Vec2::new(WHEEL, 12.0);

const TICK: f32 = 6.0;

const RING: usize = 96;

const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

const BLUE_AXIS: [f64; 3] = [0.0, -0.1873, 1.8556];

const RED_AXIS: [f64; 3] = [1.5748, -0.4681, 0.0];

const LANDING: Duration = Duration::from_secs(2);

const LOG_BADGE: [&str; 4] = ["contrast", "log_shadow", "log_midtone", "log_highlight"];

const NO_BADGE: [&str; 3] = ["pivot", "low_range", "high_range"];

pub(super) const SWITCHING: [(SwitchingClips, &str); 4] = [
    (SwitchingClips::LastAdjusted, "Last Adjusted Node"),
    (SwitchingClips::First, "First Node"),
    (SwitchingClips::Last, "Last Node"),
    (SwitchingClips::Same, "Same Node"),
];

const PRINTED: [(PrinterColor, &str); 3] = [
    (PrinterColor::Red, "Red"),
    (PrinterColor::Green, "Green"),
    (PrinterColor::Blue, "Blue"),
];

const CURRENT: Color32 = Color32::from_rgb(255, 140, 0);

const ADDS: [(Action, GradeAdd); 5] = [
    (Action::AddSerialNode, GradeAdd::Serial),
    (Action::AddNodeBefore, GradeAdd::Before),
    (Action::AddParallelNode, GradeAdd::Parallel),
    (Action::AddLayerNode, GradeAdd::Layer),
    (Action::AppendNode, GradeAdd::Append),
];

const ADD_MENU: [(GradeAdd, &str); 5] = [
    (GradeAdd::Serial, "Add Serial"),
    (GradeAdd::Before, "Add Before Current"),
    (GradeAdd::Parallel, "Add Parallel"),
    (GradeAdd::Layer, "Add Layer"),
    (GradeAdd::Append, "Append Node"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Port {
    Source,
    Node(u32),
    Output,
}

#[derive(Debug, Clone, Copy)]
enum NodeAct {
    Add(GradeAdd),
    Reset,
    Label,
    Toggle,
    Lock,
    Extract,
    Delete,
    AddInput,
    Morph(GradeMixer),
    Mode(BlendMode),
}

#[derive(Debug, Clone, Copy)]
enum FieldAct {
    Corrector,
    ResetAll,
    KeepNodes,
    Zoom(NodeZoom),
    Wide,
    Cleanup,
}

pub(super) struct Graded {
    of: GradeOf,
    node: u32,
    frame: i64,
    at: i64,
    grade: Grade,
    sent: Option<Instant>,
}

struct Wheel {
    name: &'static str,
    field: &'static str,
    values: Vec<f64>,
    neutral: f64,
    reach: f64,
    step: f64,
    own_master: bool,
}

struct Change {
    field: &'static str,
    value: Value,
}

fn changed_corrector(g: &Corrector, field: &str, value: Value) -> Corrector {
    let mut v = serde_json::to_value(g).unwrap_or_default();
    v[field] = value;
    serde_json::from_value(v)
        .ok()
        .filter(|n| corrector_checked(n).is_ok())
        .unwrap_or_else(|| g.clone())
}

fn with_corrector(g: &Grade, n: u32, frame: i64, c: Corrector) -> Grade {
    let mut out = g.clone();
    if let Some(d) = out.node_mut(n) {
        d.change_at(frame, |_| c);
    }
    out
}

fn held(g: &Grade, n: u32, frame: i64) -> Option<&Corrector> {
    g.node(n)?.changed_at(frame)
}

fn key_at(g: &Grade, n: u32, frame: i64) -> Option<usize> {
    g.node(n)?.changed_key(frame)
}

fn key_output_held(g: &Grade, n: u32, frame: i64) -> Option<KeyOutput> {
    g.node(n)?.key_output_changed_at(frame)
}

fn correctors(g: &Grade) -> impl DoubleEndedIterator<Item = u32> + '_ {
    (g.nodes.iter().enumerate())
        .filter(|(_, d)| !d.tool.is_mixer())
        .map(|(i, _)| i as u32 + 1)
}

fn stepped(g: &Grade, from: Option<u32>, forward: bool) -> Option<u32> {
    match (from, forward) {
        (Some(at), true) => correctors(g).find(|n| *n > at).or(correctors(g).next()),
        (Some(at), false) => (correctors(g).rev())
            .find(|n| *n < at)
            .or(correctors(g).next_back()),
        (None, true) => correctors(g).next(),
        (None, false) => correctors(g).next_back(),
    }
}

fn switched_to(g: &Grade, how: SwitchingClips, kept: Option<u32>, was: Option<u32>) -> u32 {
    let kept = kept.filter(|n| g.node(*n).is_some());
    match how {
        SwitchingClips::LastAdjusted => kept,
        SwitchingClips::First => correctors(g).next(),
        SwitchingClips::Last => correctors(g).next_back(),
        SwitchingClips::Same => was.filter(|n| g.node(*n).is_some()).or(kept),
    }
    .unwrap_or(1)
}

fn badges(d: &GradeNode) -> Vec<Badge> {
    let lock = (d.locked).then_some(Badge {
        icon: icon::LOCK_SIMPLE,
        tip: "Locked",
    });
    let GradeTool::Corrector(c) = &d.tool else {
        return lock.into_iter().collect();
    };
    let usual = Corrector::default();
    let changed: Vec<String> = std::iter::once(c.as_ref())
        .chain(d.keys.iter().map(|k| &k.values))
        .flat_map(|v| differences(&usual, v).into_iter().map(|(k, _)| k))
        .collect();
    let log = |k: &String| LOG_BADGE.contains(&k.as_str());
    let primary = |k: &String| !log(k) && !NO_BADGE.contains(&k.as_str());
    [
        (
            changed.iter().any(primary),
            icon::SLIDERS_HORIZONTAL,
            "Primaries",
        ),
        (changed.iter().any(log), icon::CIRCLE_HALF, "Log"),
        (d.is_animated(), icon::DIAMOND, "Keyframes"),
        (
            std::iter::once(&d.key)
                .chain(d.keys.iter().map(|k| &k.key))
                .any(|k| !k.is_usual()),
            icon::SQUARE_HALF,
            "Key",
        ),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, icon, tip)| Badge { icon, tip })
    .chain(lock)
    .collect()
}

fn last_corrector(g: &Grade, kept: impl Fn(u32) -> bool) -> Option<u32> {
    let correctors = (g.nodes.iter().zip(1u32..))
        .filter(|(d, k)| !d.tool.is_mixer() && kept(*k))
        .map(|(_, k)| k);
    correctors
        .max()
        .or_else(|| (1..=g.last()).rev().find(|k| kept(*k)))
}

fn differences(from: &Corrector, to: &Corrector) -> Map<String, Value> {
    let (Ok(Value::Object(a)), Ok(Value::Object(b))) =
        (serde_json::to_value(from), serde_json::to_value(to))
    else {
        return Map::new();
    };
    b.into_iter().filter(|(k, v)| a.get(k) != Some(v)).collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Wheels,
    Bars,
    Log,
}

#[derive(Clone, Copy, PartialEq)]
enum Palette {
    Primaries,
    Key,
}

const KEY_SWATCH: Vec2 = Vec2::new(96.0, 54.0);

fn key_output(ui: &mut Ui, changes: &mut Vec<Change>, key: KeyOutput) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(KEY_SWATCH, Sense::hover());
        let level = (key.of(1.0) * 255.0).round() as u8;
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_gray(level));
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.strong("Key Output");
                let mut invert = key.invert;
                if ui.toggle_value(&mut invert, "Invert").changed() {
                    changes.push(Change {
                        field: "key_invert",
                        value: json!(invert),
                    });
                }
            });
            ui.horizontal(|ui| {
                slider(
                    ui,
                    changes,
                    ("Gain", "key_gain"),
                    key.gain,
                    0.005,
                    (0.0, 2.0),
                );
                let range = (-1.0, 1.0);
                slider(
                    ui,
                    changes,
                    ("Offset", "key_offset"),
                    key.offset,
                    0.005,
                    range,
                );
            });
        });
    });
}

fn primaries_body(
    ui: &mut Ui,
    (g, mode): (&Corrector, Mode),
    changes: &mut Vec<Change>,
) -> Option<(PrinterColor, f64)> {
    let wheels = match mode {
        Mode::Log => logs(g),
        Mode::Wheels | Mode::Bars => primaries(g),
    };
    let mut point = None;
    ui.horizontal(|ui| {
        for w in &wheels {
            let made = match mode {
                Mode::Bars => {
                    let (made, pressed) = bars(ui, w);
                    point = point.or(pressed);
                    made
                }
                Mode::Wheels | Mode::Log => wheel(ui, w),
            };
            if let Some(values) = made {
                changes.push(Change {
                    field: w.field,
                    value: json!(values),
                });
            }
            ui.add_space(12.0);
        }
    });
    let c = changes;
    ui.horizontal(|ui| {
        let percent = (0.0, 100.0);
        slider(
            ui,
            c,
            ("Color Boost", "color_boost"),
            g.color_boost,
            0.5,
            (-100.0, 100.0),
        );
        slider(
            ui,
            c,
            ("Shadows", "shadows"),
            g.shadows,
            0.5,
            (-100.0, 100.0),
        );
        slider(
            ui,
            c,
            ("Highlights", "highlights"),
            g.highlights,
            0.5,
            (-100.0, 100.0),
        );
        slider(
            ui,
            c,
            ("Saturation", "saturation"),
            g.saturation,
            0.5,
            percent,
        );
        slider(ui, c, ("Hue", "hue"), g.hue, 0.5, percent);
        slider(ui, c, ("Lum Mix", "lum_mix"), g.lum_mix, 0.5, percent);
    });
    point
}

fn keyed(key: KeyOutput, c: &Change) -> KeyOutput {
    let mut key = key;
    match (c.field, &c.value) {
        ("key_gain", Value::Number(v)) => key.gain = v.as_f64().unwrap_or(key.gain),
        ("key_offset", Value::Number(v)) => key.offset = v.as_f64().unwrap_or(key.offset),
        ("key_invert", Value::Bool(v)) => key.invert = *v,
        _ => {}
    }
    key
}

fn key_changes(from: KeyOutput, to: KeyOutput) -> Map<String, Value> {
    let mut out = Map::new();
    if from.gain != to.gain {
        out.insert("key_gain".into(), json!(to.gain));
    }
    if from.offset != to.offset {
        out.insert("key_offset".into(), json!(to.offset));
    }
    if from.invert != to.invert {
        out.insert("key_invert".into(), json!(to.invert));
    }
    out
}

const BAR: Vec2 = Vec2::new(16.0, 110.0);

fn named(of: &GradeOf) -> (String, Value) {
    match of {
        GradeOf::Clip(id) => ("clips".into(), json!(id)),
        GradeOf::Timeline => ("timeline".into(), json!(true)),
    }
}

const PRINTER_COLORS: [PrinterColor; 7] = [
    PrinterColor::Master,
    PrinterColor::Red,
    PrinterColor::Green,
    PrinterColor::Blue,
    PrinterColor::Cyan,
    PrinterColor::Magenta,
    PrinterColor::Yellow,
];

fn printer_label(color: PrinterColor, points: f64) -> String {
    let name = match color {
        PrinterColor::Master => "Master",
        PrinterColor::Red => "Red",
        PrinterColor::Green => "Green",
        PrinterColor::Blue => "Blue",
        PrinterColor::Cyan => "Cyan",
        PrinterColor::Magenta => "Magenta",
        PrinterColor::Yellow => "Yellow",
    };
    format!("{name} {}", if points > 0.0 { "+" } else { "-" })
}

fn params(of: &GradeOf, field: &str, more: Value) -> Value {
    let mut v = match of {
        GradeOf::Clip(id) => json!({ field: id }),
        GradeOf::Timeline => json!({ "timeline": true }),
    };
    if let (Value::Object(to), Value::Object(from)) = (&mut v, more) {
        to.extend(from);
    }
    v
}

fn command(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn port(v: u32) -> Port {
    match v {
        0 => Port::Source,
        n => Port::Node(n),
    }
}

fn from_number(p: Port) -> Option<u32> {
    match p {
        Port::Source => Some(0),
        Port::Node(n) => Some(n),
        Port::Output => None,
    }
}

fn to_number(p: Port) -> Option<u32> {
    match p {
        Port::Node(n) => Some(n),
        Port::Output => Some(0),
        Port::Source => None,
    }
}

fn kind(tool: &GradeTool) -> &'static str {
    match tool {
        GradeTool::Corrector(_) => "Corrector",
        GradeTool::ParallelMixer => "Parallel Mixer",
        GradeTool::LayerMixer { .. } => "Layer Mixer",
    }
}

fn tiles(g: &Grade) -> Vec<Tile<Port>> {
    let line = Stroke::new(1.5, theme::colors().text_dim);
    let input = |slot: usize, name: String| Inlet {
        slot,
        side: Side::Left,
        knot: RGB_KNOT,
        wire: line,
        name,
    };
    let mut out = vec![Tile {
        key: Port::Source,
        label: "Source".into(),
        place: [0.0; 2],
        faded: false,
        switch: false,
        drops: false,
        note: String::new(),
        badges: Vec::new(),
        outlet: Some(Outlet {
            name: "Source".into(),
            color: RGB_KNOT,
        }),
        inlets: Vec::new(),
        edge: Some(Edge::Left),
    }];
    for (d, n) in g.nodes.iter().zip(1u32..) {
        let (label, inlets) = match &d.tool {
            GradeTool::Corrector(_) => (format!("{n:02}"), vec![input(0, "RGB Input".into())]),
            tool => (
                kind(tool).to_string(),
                (0..d.inputs.len())
                    .map(|i| input(i, format!("Input {}", i + 1)))
                    .collect(),
            ),
        };
        out.push(Tile {
            key: Port::Node(n),
            label,
            place: d.place,
            faded: !d.on,
            switch: !d.tool.is_mixer(),
            drops: !d.tool.is_mixer(),
            note: d.label.clone(),
            badges: badges(d),
            outlet: Some(Outlet {
                name: "RGB Output".into(),
                color: RGB_KNOT,
            }),
            inlets,
            edge: None,
        });
    }
    out.push(Tile {
        key: Port::Output,
        label: "Output".into(),
        place: [0.0; 2],
        faded: false,
        switch: false,
        drops: false,
        note: String::new(),
        badges: Vec::new(),
        outlet: None,
        inlets: vec![input(0, "Output".into())],
        edge: Some(Edge::Right),
    });
    out
}

fn wires(g: &Grade) -> Vec<Wire<Port>> {
    let mut out: Vec<Wire<Port>> = g
        .nodes
        .iter()
        .zip(1u32..)
        .flat_map(|(d, n)| {
            d.inputs.iter().enumerate().filter_map(move |(slot, v)| {
                v.map(|v| Wire {
                    from: port(v),
                    to: Port::Node(n),
                    slot,
                })
            })
        })
        .collect();
    if let Some(o) = g.output {
        out.push(Wire {
            from: port(o),
            to: Port::Output,
            slot: 0,
        });
    }
    out
}

fn linked_now(g: &Grade, to: Port, slot: usize) -> Option<Port> {
    match to {
        Port::Node(n) => g.node(n)?.inputs.get(slot).copied().flatten().map(port),
        Port::Output => g.output.map(port),
        Port::Source => None,
    }
}

fn connect(of: &GradeOf, to: Port, slot: Option<usize>, from: Option<Port>) -> Option<Value> {
    let mut more = json!({ "node": to_number(to)? });
    if let Some(s) = slot {
        more["input"] = json!(s);
    }
    if let Some(f) = from.and_then(from_number) {
        more["from"] = json!(f);
    }
    Some(command("grade.connect", params(of, "clip", more)))
}

fn grade_links(g: &Grade, of: &GradeOf, l: &Link<Port>) -> Vec<Value> {
    let onto = |dst: Port| -> Option<Option<usize>> {
        match dst {
            Port::Node(n) if g.node(n)?.tool.is_mixer() => Some(None),
            Port::Node(_) | Port::Output => Some(Some(0)),
            Port::Source => None,
        }
    };
    let new = match (&l.anchor, &l.spot) {
        (End::Output(src), Spot::Knot(End::Input(dst, s))) => Some((*dst, Some(*s), *src)),
        (End::Output(src), Spot::Tile(dst)) => match l.pulled {
            Some((pd, ps)) if pd == *dst => Some((*dst, Some(ps), *src)),
            _ => onto(*dst).map(|s| (*dst, s, *src)),
        },
        (End::Input(dst, s), Spot::Knot(End::Output(src)) | Spot::Tile(src))
            if *src != Port::Output =>
        {
            Some((*dst, Some(*s), *src))
        }
        _ => None,
    };
    let new = new.filter(|(d, _, s)| d != s);
    let mut out = Vec::new();
    if let Some((pd, ps)) = l.pulled
        && new.is_none_or(|(d, s, _)| d != pd || s != Some(ps))
    {
        out.extend(connect(of, pd, Some(ps), None));
    }
    if let Some((d, s, src)) = new
        && s.is_none_or(|s| linked_now(g, d, s) != Some(src))
    {
        out.extend(connect(of, d, s, Some(src)));
    }
    out
}

fn node_menu(ui: &mut Ui, d: &GradeNode, delete_key: &str, act: &mut Option<NodeAct>) {
    let mut pick = |ui: &mut Ui, on: bool, button: egui::Button, a: NodeAct| {
        if ui.add_enabled(on, button).clicked() {
            *act = Some(a);
            ui.close();
        }
    };
    let mixer = d.tool.is_mixer();
    ui.menu_button("Add Node", |ui| {
        for (add, text) in ADD_MENU {
            let can = !mixer || matches!(add, GradeAdd::Serial | GradeAdd::Append);
            pick(ui, can, egui::Button::new(text), NodeAct::Add(add));
        }
    });
    let delete = || egui::Button::new("Delete Node").shortcut_text(delete_key);
    let (morph, to) = match &d.tool {
        GradeTool::Corrector(_) => {
            pick(
                ui,
                true,
                egui::Button::new("Reset Node Grade"),
                NodeAct::Reset,
            );
            pick(ui, true, egui::Button::new("Node Label"), NodeAct::Label);
            let lock = egui::Button::selectable(d.locked, "Lock Node");
            pick(ui, true, lock, NodeAct::Lock);
            let off = egui::Button::selectable(!d.on, "Disable Node");
            pick(ui, true, off, NodeAct::Toggle);
            let free = d.inputs.iter().all(Option::is_none);
            pick(
                ui,
                !free,
                egui::Button::new("Extract Node"),
                NodeAct::Extract,
            );
            pick(ui, true, delete(), NodeAct::Delete);
            return;
        }
        GradeTool::ParallelMixer => ("Morph Into Layer Mixer Node", GradeMixer::LayerMixer),
        GradeTool::LayerMixer { .. } => ("Morph Into Parallel Node", GradeMixer::ParallelMixer),
    };
    let lock = egui::Button::selectable(d.locked, "Lock Node");
    pick(ui, true, lock, NodeAct::Lock);
    pick(ui, true, delete(), NodeAct::Delete);
    pick(
        ui,
        true,
        egui::Button::new("Add One Input"),
        NodeAct::AddInput,
    );
    pick(ui, true, egui::Button::new(morph), NodeAct::Morph(to));
    if let GradeTool::LayerMixer { mode } = &d.tool {
        ui.menu_button("Composite Mode", |ui| {
            for m in BlendMode::ALL {
                let text = super::placing::mode_label(m.into());
                let item = egui::Button::selectable(m == *mode, text);
                pick(ui, true, item, NodeAct::Mode(m));
            }
        });
    }
}

fn field_menu(ui: &mut Ui, act: &mut Option<FieldAct>) {
    let mut pick = |ui: &mut Ui, text: &str, a: FieldAct| {
        if ui.button(text).clicked() {
            *act = Some(a);
            ui.close();
        }
    };
    pick(ui, "Reset All Grades and Nodes", FieldAct::ResetAll);
    pick(ui, "Reset Grades and Keep Nodes", FieldAct::KeepNodes);
    ui.menu_button("Add Node", |ui| pick(ui, "Corrector", FieldAct::Corrector));
    ui.separator();
    for (text, zoom) in [
        ("Zoom In", NodeZoom::In),
        ("Zoom Out", NodeZoom::Out),
        ("Zoom to Window", NodeZoom::Fit),
        ("Original Size", NodeZoom::Original),
    ] {
        pick(ui, text, FieldAct::Zoom(zoom));
    }
    pick(ui, "Toggle Display Mode", FieldAct::Wide);
    pick(ui, "Cleanup Node Graph", FieldAct::Cleanup);
}

fn grade_switch(ui: &mut Ui, level: GradeLevel) -> Option<GradeLevel> {
    let mut picked = None;
    let right = egui::Layout::right_to_left(egui::Align::Center);
    ui.with_layout(right, |ui| {
        super::dialog::switch(ui, |ui| {
            for (l, text) in [
                (GradeLevel::Clip, "Clip"),
                (GradeLevel::Timeline, "Timeline"),
            ] {
                if ui.selectable_label(level == l, text).clicked() && level != l {
                    picked = Some(l);
                }
            }
        });
    });
    picked
}

fn usual(field: &str) -> f64 {
    let (values, field) = match field.strip_prefix("key_") {
        Some(f) => (serde_json::to_value(KeyOutput::default()), f),
        None => (serde_json::to_value(Corrector::default()), field),
    };
    values
        .ok()
        .and_then(|v| v.get(field).and_then(Value::as_f64))
        .unwrap_or_default()
}

fn name(ui: &mut Ui, text: &str) -> Response {
    ui.add(Label::new(text).sense(Sense::click_and_drag()))
        .on_hover_cursor(CursorIcon::ResizeHorizontal)
}

fn number(ui: &mut Ui, (text, v): (&str, &mut f64), w: &Wheel) -> bool {
    let mut out = false;
    ui.vertical(|ui| {
        let n = name(ui, text);
        let dx = n.drag_delta().x;
        if n.double_clicked() {
            *v = w.neutral;
            out = true;
        } else if n.dragged() && dx != 0.0 {
            *v += f64::from(dx) * w.step;
            out = true;
        }
        let r = ui.add(
            DragValue::new(v)
                .speed(w.step)
                .fixed_decimals(2)
                .update_while_editing(false),
        );
        out |= r.changed();
    });
    out
}

fn dial(ui: &mut Ui, values: &mut [f64], w: &Wheel) -> bool {
    let (rect, resp) = ui.allocate_exact_size(DIAL, Sense::drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, rect.height() / 2.0, Color32::from_gray(40));
    let mean = values.iter().take(3).sum::<f64>() / 3.0;
    let turned = ((mean - w.neutral) / w.reach) as f32 * INNER;
    let mut x = rect.left() + turned.rem_euclid(TICK);
    while x < rect.right() {
        painter.vline(
            x,
            rect.y_range().shrink(3.0),
            Stroke::new(1.0, Color32::from_gray(110)),
        );
        x += TICK;
    }
    let dx = resp
        .on_hover_cursor(CursorIcon::ResizeHorizontal)
        .drag_delta()
        .x;
    if dx == 0.0 {
        return false;
    }
    let by = f64::from(dx) * w.reach / f64::from(INNER);
    values.iter_mut().for_each(|v| *v += by);
    true
}

fn heading(ui: &mut Ui, w: &Wheel, values: &mut [f64]) -> bool {
    let mut reset = false;
    ui.horizontal(|ui| {
        ui.label(w.name);
        if ui.small_button("Reset").clicked() {
            values.iter_mut().for_each(|v| *v = w.neutral);
            reset = true;
        }
    });
    reset
}

fn strip(ui: &mut Ui, v: &mut f64, w: &Wheel, color: Color32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(BAR, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_gray(28));
    let mid = rect.center().y;
    let half = rect.height() / 2.0;
    let to = (mid - ((*v - w.neutral) / w.reach) as f32 * half).clamp(rect.top(), rect.bottom());
    painter.rect_filled(
        Rect::from_x_y_ranges(rect.x_range(), mid.min(to)..=mid.max(to)),
        0.0,
        color,
    );
    painter.hline(
        rect.x_range(),
        mid,
        Stroke::new(1.0, Color32::from_gray(90)),
    );
    if resp.double_clicked() {
        *v = w.neutral;
        return true;
    }
    let dy = resp.drag_delta().y;
    if resp.dragged() && dy != 0.0 {
        *v -= f64::from(dy) * w.reach / f64::from(half);
        return true;
    }
    false
}

fn arrow(ui: &mut Ui, (glyph, sign): (&str, &str), i: usize) -> bool {
    let (_, color) = PRINTED[i];
    ui.small_button(glyph)
        .on_hover_text(format!("{sign} {color}"))
        .clicked()
}

fn bars(ui: &mut Ui, w: &Wheel) -> (Option<Vec<f64>>, Option<(PrinterColor, f64)>) {
    let mut values = w.values.clone();
    let mut out = false;
    let mut point = None;
    let arrows = w.field == "offset";
    let order = [
        (3, "Y", Color32::from_gray(200)),
        (0, "R", Color32::from_rgb(220, 70, 70)),
        (1, "G", Color32::from_rgb(70, 200, 90)),
        (2, "B", Color32::from_rgb(80, 120, 230)),
    ];
    ui.vertical(|ui| {
        if heading(ui, w, &mut values) {
            out = true;
        }
        ui.horizontal(|ui| {
            let n = values.len();
            for (i, text, color) in order.into_iter().filter(|o| o.0 < n) {
                ui.vertical(|ui| {
                    if name(ui, text).double_clicked() {
                        values[i] = w.neutral;
                        out = true;
                    }
                    if arrows && arrow(ui, (icon::CARET_UP, "Plus"), i) {
                        point = Some((PRINTED[i].0, 1.0));
                    }
                    if strip(ui, &mut values[i], w, color) {
                        out = true;
                    }
                    if arrows && arrow(ui, (icon::CARET_DOWN, "Minus"), i) {
                        point = Some((PRINTED[i].0, -1.0));
                    }
                    let r = ui.add(
                        DragValue::new(&mut values[i])
                            .speed(w.step)
                            .fixed_decimals(2)
                            .update_while_editing(false),
                    );
                    if r.changed() {
                        out = true;
                    }
                });
            }
        });
    });
    (out.then_some(values), point)
}

fn chroma(d: [f64; 3]) -> (f64, f64, f64) {
    let y = (0..3).map(|i| d[i] * LUMA[i]).sum::<f64>();
    ((d[2] - y) / BLUE_AXIS[2], (d[0] - y) / RED_AXIS[0], y)
}

fn ring_color(angle: f32) -> Color32 {
    let (cb, cr) = (f64::from(angle.cos()), f64::from(angle.sin()));
    let c = |i: usize| (0.5 + 0.3 * (cb * BLUE_AXIS[i] + cr * RED_AXIS[i])).clamp(0.0, 1.0);
    Color32::from_rgb(
        (c(0) * 255.0) as u8,
        (c(1) * 255.0) as u8,
        (c(2) * 255.0) as u8,
    )
}

fn slider(
    ui: &mut Ui,
    changes: &mut Vec<Change>,
    (label, field): (&str, &'static str),
    v: f64,
    step: f64,
    range: (f64, f64),
) {
    let mut v = v;
    ui.horizontal(|ui| {
        let n = name(ui, label);
        let dx = n.drag_delta().x;
        let mut changed = false;
        if n.double_clicked() {
            v = usual(field);
            changed = true;
        } else if n.dragged() && dx != 0.0 {
            v = (v + f64::from(dx) * step).clamp(range.0, range.1);
            changed = true;
        }
        let r = ui.add(
            DragValue::new(&mut v)
                .speed(step)
                .range(range.0..=range.1)
                .fixed_decimals(3)
                .update_while_editing(false),
        );
        if changed || r.changed() {
            changes.push(Change {
                field,
                value: json!(v),
            });
        }
    });
}

fn wheel(ui: &mut Ui, w: &Wheel) -> Option<Vec<f64>> {
    let mut values = w.values.clone();
    let mut out = false;
    ui.vertical(|ui| {
        if heading(ui, w, &mut values) {
            out = true;
        }
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(WHEEL), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let center = rect.center();
        let (outer, inner) = (OUTER, INNER);
        for i in 0..RING {
            let a0 = i as f32 / RING as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / RING as f32 * std::f32::consts::TAU;
            let at = |a: f32, r: f32| center + Vec2::new(a.cos(), -a.sin()) * r;
            let mid = (outer + inner) / 2.0;
            painter.line_segment(
                [at(a0, mid), at(a1, mid)],
                Stroke::new(outer - inner, ring_color((a0 + a1) / 2.0)),
            );
        }
        painter.circle_filled(center, inner - 1.0, Color32::from_gray(28));
        painter.line_segment(
            [center - Vec2::X * inner, center + Vec2::X * inner],
            Stroke::new(1.0, Color32::from_gray(60)),
        );
        painter.line_segment(
            [center - Vec2::Y * inner, center + Vec2::Y * inner],
            Stroke::new(1.0, Color32::from_gray(60)),
        );
        let d = [0, 1, 2].map(|i| values[i] - w.neutral);
        let (cb, cr, y) = chroma(d);
        let puck = center + Vec2::new((cb / w.reach) as f32, -(cr / w.reach) as f32) * inner;
        painter.circle_stroke(puck, 5.0, Stroke::new(2.0, Color32::WHITE));
        let (shift, command) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
        let delta = resp.drag_delta();
        if resp.double_clicked() {
            for v in values.iter_mut().take(3) {
                *v = w.neutral + y;
            }
            out = true;
        } else if resp.dragged() && command {
            if delta.x != 0.0 {
                let by = f64::from(delta.x) * w.reach / f64::from(inner);
                values.iter_mut().for_each(|v| *v += by);
                out = true;
            }
        } else if resp.dragged() && (shift || delta != Vec2::ZERO) {
            let at = match resp.interact_pointer_pos().filter(|_| shift) {
                Some(pos) => (pos - center) / inner,
                None => (puck - center + delta) / inner,
            };
            let at = if at.length() > 1.0 {
                at / at.length()
            } else {
                at
            };
            let (cb, cr) = (f64::from(at.x) * w.reach, -f64::from(at.y) * w.reach);
            for (i, v) in values.iter_mut().take(3).enumerate() {
                *v = w.neutral + y + cb * BLUE_AXIS[i] + cr * RED_AXIS[i];
            }
            out = true;
        }
        out |= dial(ui, &mut values, w);
        ui.horizontal(|ui| {
            let names = ["R", "G", "B", "Y"];
            for (i, text) in names.iter().enumerate().take(values.len()) {
                out |= number(ui, (text, &mut values[i]), w);
            }
        });
        if !w.own_master {
            let mean = values.iter().take(3).sum::<f64>() / 3.0;
            let mut master = mean;
            ui.horizontal(|ui| {
                ui.label("Master");
                let r = ui.add(
                    DragValue::new(&mut master)
                        .speed(w.step)
                        .fixed_decimals(2)
                        .update_while_editing(false),
                );
                if r.changed() {
                    for v in values.iter_mut().take(3) {
                        *v += master - mean;
                    }
                    out = true;
                }
            });
        }
    });
    out.then_some(values)
}

fn primaries(g: &Corrector) -> [Wheel; 4] {
    [
        Wheel {
            name: "Lift",
            field: "lift",
            values: g.lift.to_vec(),
            neutral: 0.0,
            reach: 0.25,
            step: 0.002,
            own_master: true,
        },
        Wheel {
            name: "Gamma",
            field: "gamma",
            values: g.gamma.to_vec(),
            neutral: 0.0,
            reach: 0.25,
            step: 0.002,
            own_master: true,
        },
        Wheel {
            name: "Gain",
            field: "gain",
            values: g.gain.to_vec(),
            neutral: 1.0,
            reach: 0.5,
            step: 0.005,
            own_master: true,
        },
        Wheel {
            name: "Offset",
            field: "offset",
            values: g.offset.to_vec(),
            neutral: 25.0,
            reach: 10.0,
            step: 0.1,
            own_master: false,
        },
    ]
}

fn logs(g: &Corrector) -> [Wheel; 4] {
    let log = |name, field, values: [f64; 3]| Wheel {
        name,
        field,
        values: values.to_vec(),
        neutral: 0.0,
        reach: 0.25,
        step: 0.002,
        own_master: false,
    };
    [
        log("Shadow", "log_shadow", g.log_shadow),
        log("Midtone", "log_midtone", g.log_midtone),
        log("Highlight", "log_highlight", g.log_highlight),
        Wheel {
            name: "Offset",
            field: "offset",
            values: g.offset.to_vec(),
            neutral: 25.0,
            reach: 10.0,
            step: 0.1,
            own_master: false,
        },
    ]
}

impl Workspace {
    pub(super) fn graded_wanted(&self) -> Option<(String, Picture)> {
        if self.effects_page.page != Page::Color {
            return None;
        }
        let h = self.effects_page.graded.as_ref()?;
        let GradeOf::Clip(id) = &h.of else {
            return None;
        };
        let c = self.project()?.clip(id)?;
        let mut picture = c.picture.clone().unwrap_or_default();
        picture.grade = checked(&h.grade).ok()?;
        Some((id.clone(), picture))
    }

    pub(super) fn timeline_grade_wanted(&self) -> Option<Grade> {
        let h = self.effects_page.graded.as_ref()?;
        let on = self.effects_page.page == Page::Color && h.of == GradeOf::Timeline;
        on.then(|| h.grade.clone())
    }

    pub(super) fn set_grading(&mut self, level: GradeLevel) {
        if level != self.effects_page.grading {
            self.effects_page.graded = None;
            self.effects_page.grading = level;
            self.sync_grade_preview();
        }
    }

    pub(super) fn grade_shown(&self) -> Option<(GradeOf, String, Grade)> {
        let p = self.project()?;
        match self.effects_page.grading {
            GradeLevel::Clip => {
                let id = self.color_clip()?;
                let c = p.clip(&id)?;
                let stored = c.picture.as_ref().and_then(|pic| pic.grade.clone());
                let name = c.name.clone().unwrap_or_else(|| id.clone());
                Some((GradeOf::Clip(id), name, stored.unwrap_or_default()))
            }
            GradeLevel::Timeline => {
                if !p.compound_path.is_empty() {
                    return None;
                }
                let t = p.timeline()?;
                let stored = t.grade.clone().unwrap_or_default();
                Some((GradeOf::Timeline, t.name.clone(), stored))
            }
        }
    }

    pub(super) fn primaries_panel(&mut self, ui: &mut Ui, inner: f32) {
        let id = ui.scope_id().with("palette-height");
        let room = ui.available_rect_before_wrap();
        let tall = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(room.height());
        let full_id = ui.scope_id().with("primaries-height");
        let full = ui.data(|d| d.get_temp::<f32>(full_id));
        let mut side = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("color-keys-side")
                .max_rect(Rect::from_min_size(room.min, Vec2::new(room.width(), tall))),
        );
        let keys = egui::Panel::right("color-keys")
            .resizable(true)
            .default_size(380.0)
            .min_size(220.0)
            .frame(egui::Frame::NONE)
            .show(&mut side, |ui| self.grade_keys_panel(ui))
            .response
            .rect;
        let mut left = ui.new_child(egui::UiBuilder::new().id_salt("color-palette").max_rect(
            Rect::from_min_max(room.min, egui::pos2(keys.left(), room.bottom())),
        ));
        egui::ScrollArea::vertical()
            .id_salt("primaries-scroll")
            .max_height(inner)
            .min_scrolled_height(inner)
            .auto_shrink([false, true])
            .show(&mut left, |ui| {
                let primaries = self.palette(ui);
                let drawn = ui.min_rect();
                match (primaries, full) {
                    (true, _) => ui.data_mut(|d| {
                        d.insert_temp(full_id, drawn.height());
                    }),
                    (false, Some(h)) => {
                        ui.expand_to_include_rect(Rect::from_min_size(drawn.min, Vec2::new(0.0, h)))
                    }
                    (false, None) => {}
                }
            });
        let palette = left.min_rect();
        if (palette.height() - tall).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(id, palette.height()));
            ui.ctx().request_repaint();
        }
        ui.advance_cursor_after_rect(palette);
        self.sync_picture_preview();
    }

    pub(super) fn grade_frame(&self, of: &GradeOf) -> i64 {
        let at = self.snap.timeline.position;
        match of {
            GradeOf::Clip(id) => (self.project())
                .and_then(|p| p.clip(id))
                .map_or(at, |c| c.content_at(at)),
            GradeOf::Timeline => at,
        }
    }

    pub(super) fn color_clip(&self) -> Option<String> {
        let p = self.project()?;
        super::placing::topmost_by(p, self.snap.timeline.position, takes_effects)
    }

    fn palette(&mut self, ui: &mut Ui) -> bool {
        let level = self.effects_page.grading;
        let mut picked = None;
        let Some((of, name, stored)) = self.grade_shown() else {
            self.effects_page.graded = None;
            ui.horizontal(|ui| {
                ui.label(match level {
                    GradeLevel::Clip => "No Clip at the Playhead",
                    GradeLevel::Timeline => "No Timeline",
                });
                picked = grade_switch(ui, level);
            });
            if let Some(l) = picked {
                self.send("gui.page", json!({ "page": Page::Color, "grade": l }));
            }
            return false;
        };
        self.settle_grade(&of, &stored);
        let node = self.current_grade_node(&stored);
        let frame = self.grade_frame(&of);
        if let Some(h) = &self.effects_page.graded
            && (h.of != of
                || Some(h.node) != node
                || key_at(&stored, h.node, h.frame) != key_at(&stored, h.node, frame)
                || h.sent
                    .is_some_and(|t| h.grade == stored || t.elapsed() > LANDING))
        {
            self.effects_page.graded = None;
        }
        let (grade, frame, at) = match &self.effects_page.graded {
            Some(h) => (h.grade.clone(), h.frame, h.at),
            None => (stored.clone(), frame, self.snap.timeline.position),
        };
        let Some((n, g)) = node.and_then(|n| Some((n, held(&grade, n, frame)?.clone()))) else {
            let what = node
                .and_then(|n| stored.node(n))
                .map_or("No Node Selected", |d| kind(&d.tool));
            ui.horizontal(|ui| {
                ui.strong(&name);
                ui.separator();
                ui.label(what);
                picked = grade_switch(ui, level);
            });
            if let Some(l) = picked {
                self.send("gui.page", json!({ "page": Page::Color, "grade": l }));
            }
            return false;
        };
        let key = egui::Id::unique("color-wheels-mode");
        let mut mode = ui.data(|d| d.get_temp::<Mode>(key)).unwrap_or(Mode::Wheels);
        let shown = egui::Id::unique("color-palette");
        let mut palette = (ui.data(|d| d.get_temp::<Palette>(shown))).unwrap_or(Palette::Primaries);
        let mut changes: Vec<Change> = Vec::new();
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.strong(name);
            ui.label(format!("Node {n:02}"));
            ui.separator();
            for (p, text) in [(Palette::Primaries, "Primaries"), (Palette::Key, "Key")] {
                if ui.selectable_label(palette == p, text).clicked() {
                    palette = p;
                }
            }
            ui.separator();
            if palette == Palette::Key {
                reset = ui.button("Reset").clicked();
                picked = grade_switch(ui, level);
                return;
            }
            for (m, text) in [
                (Mode::Wheels, "Wheels"),
                (Mode::Bars, "Bars"),
                (Mode::Log, "Log"),
            ] {
                if ui.selectable_label(mode == m, text).clicked() {
                    mode = m;
                }
            }
            ui.separator();
            reset = ui.button("Reset").clicked();
            ui.separator();
            let log = mode == Mode::Log;
            let c = &mut changes;
            slider(ui, c, ("Temp", "temp"), g.temp, 10.0, (-4000.0, 4000.0));
            slider(ui, c, ("Tint", "tint"), g.tint, 0.5, (-100.0, 100.0));
            slider(
                ui,
                c,
                ("Contrast", "contrast"),
                g.contrast,
                0.005,
                (0.0, 2.0),
            );
            slider(ui, c, ("Pivot", "pivot"), g.pivot, 0.002, (0.0, 1.0));
            slider(
                ui,
                c,
                ("Midtone Detail", "midtone_detail"),
                g.midtone_detail,
                0.5,
                (-100.0, 100.0),
            );
            if log {
                let (low, high) = (g.low_range, g.high_range);
                slider(
                    ui,
                    c,
                    ("Low Range", "low_range"),
                    low,
                    0.002,
                    (0.001, high - 0.001),
                );
                slider(
                    ui,
                    c,
                    ("High Range", "high_range"),
                    high,
                    0.002,
                    (low + 0.001, 0.999),
                );
            }
            picked = grade_switch(ui, level);
        });
        ui.data_mut(|d| {
            d.insert_temp(key, mode);
            d.insert_temp(shown, palette);
        });
        if let Some(l) = picked {
            self.send("gui.page", json!({ "page": Page::Color, "grade": l }));
        }
        let key_now = key_output_held(&grade, n, frame).unwrap_or_default();
        let point = match palette {
            Palette::Key => {
                key_output(ui, &mut changes, key_now);
                None
            }
            Palette::Primaries => primaries_body(ui, (&g, mode), &mut changes),
        };
        let (field, target) = named(&of);
        let locked = grade.node(n).is_some_and(|d| d.locked);
        if locked {
            changes.clear();
        }
        if reset && !locked {
            self.effects_page.graded = None;
            let node = ("node".to_string(), json!(n));
            let at = ("at".to_string(), json!(self.snap.timeline.position));
            let part = match palette {
                Palette::Key => GradePalette::Key,
                Palette::Primaries => GradePalette::Primaries,
            };
            let part = ("palette".to_string(), json!(part));
            let params = Map::from_iter([(field, target), node, at, part]);
            self.send("grade.clear", Value::Object(params));
            return palette == Palette::Primaries;
        }
        if !changes.is_empty() {
            let (keys, values): (Vec<Change>, Vec<Change>) = changes
                .into_iter()
                .partition(|c| c.field.starts_with("key_"));
            let c = values
                .into_iter()
                .fold(g, |g, c| changed_corrector(&g, c.field, c.value));
            let mut next = with_corrector(&grade, n, frame, c);
            if let Some(d) = next.node_mut(n) {
                d.change_key_output_at(frame, |k| keys.iter().fold(k, keyed));
            }
            self.effects_page.graded = Some(Graded {
                of: of.clone(),
                node: n,
                frame,
                at,
                grade: next,
                sent: None,
            });
        }
        let down = ui.input(|i| i.pointer.any_down());
        let ready = match &mut self.effects_page.graded {
            Some(h) if h.sent.is_none() && !down => {
                h.sent = Some(Instant::now());
                let mut params = match (held(&stored, n, frame), held(&h.grade, n, frame)) {
                    (Some(a), Some(b)) => differences(a, b),
                    _ => Map::new(),
                };
                let keys = (
                    key_output_held(&stored, n, frame),
                    key_output_held(&h.grade, n, frame),
                );
                if let (Some(a), Some(b)) = keys {
                    params.extend(key_changes(a, b));
                }
                Some(params)
            }
            _ => None,
        };
        match ready {
            Some(params) if params.is_empty() => self.effects_page.graded = None,
            Some(mut params) => {
                params.insert(field, target);
                params.insert("node".into(), json!(n));
                params.insert("at".into(), json!(at));
                self.send("grade.set", Value::Object(params));
            }
            None => {}
        }
        if let Some(point) = point.filter(|_| !locked) {
            self.printer_point(point);
        }
        palette == Palette::Primaries
    }

    fn printer_point(&mut self, point: (PrinterColor, f64)) {
        let Some((of, _, g)) = self.grade_shown() else {
            return;
        };
        self.settle_grade(&of, &g);
        let Some(n) = self
            .current_grade_node(&g)
            .filter(|n| g.corrector(*n).is_some())
        else {
            return;
        };
        let frame = self.grade_frame(&of);
        if let Some(h) = &mut self.effects_page.graded
            && h.of == of
            && h.node == n
            && key_at(&g, n, h.frame) == key_at(&g, n, frame)
            && let Some(c) = held(&h.grade, n, h.frame)
        {
            let lights = crate::preferences::load()
                .map(|p| p.color)
                .unwrap_or_default();
            let mut c = c.clone();
            c.offset = printed(c.offset, point, &lights);
            h.grade = with_corrector(&h.grade, n, h.frame, c);
        }
        let (field, target) = named(&of);
        let mut params = Map::from_iter([(field, target)]);
        params.insert("node".into(), json!(n));
        params.insert("at".into(), json!(self.snap.timeline.position));
        params.insert("color".into(), json!(point.0));
        params.insert("points".into(), json!(point.1));
        self.send("grade.printer", Value::Object(params));
    }

    pub(super) fn color_menu(&mut self, ui: &mut Ui) {
        let page = self.effects_page.page == Page::Color && self.project().is_some();
        for (menu, size) in [
            ("Full Printer Light", 1.0),
            ("Half Printer Light", 0.5),
            ("Quarter Printer Light", 0.25),
        ] {
            ui.add_enabled_ui(page, |ui| {
                ui.menu_button(menu, |ui| {
                    for color in PRINTER_COLORS {
                        for points in [size, -size] {
                            if ui.button(printer_label(color, points)).clicked() {
                                self.printer_point((color, points));
                                ui.close();
                            }
                        }
                    }
                });
            });
        }
        if !page {
            super::dialog::why(ui, "open the Color page");
        }
    }

    pub(super) fn settle_grade(&mut self, of: &GradeOf, g: &Grade) {
        let open = (self.project().and_then(|p| p.sheet_key())).unwrap_or_default();
        let clip = matches!(of, GradeOf::Timeline)
            .then(|| self.color_clip())
            .flatten();
        let switching = self.snap.switching_clips;
        let page = &mut self.effects_page;
        let was = page.grade_nodes.last().copied();
        let seen = page.grade_seen.as_ref();
        if seen.is_none_or(|(t, o, _)| *t != open || o != of) {
            if let (Some((t, o, _)), Some(n)) = (page.grade_seen.take(), was) {
                page.grade_left.insert((t, o), n);
            }
            let kept = page.grade_left.get(&(open.clone(), of.clone())).copied();
            page.grade_nodes = vec![switched_to(g, switching, kept, was)];
            page.grade_seen = Some((open, of.clone(), clip));
            page.grade_coming = None;
            page.grade_view.stop();
            page.grade_spot = None;
            page.grade_keys.forget();
        } else if seen.is_some_and(|(_, _, c)| *c != clip) {
            let n = switched_to(g, switching, was, was);
            if Some(n) != was {
                page.grade_nodes = vec![n];
            }
            page.grade_seen = Some((open, of.clone(), clip));
        }
        if (page.grade_coming).is_some_and(|(n, t)| g.node(n).is_some() || t.elapsed() > LANDING) {
            page.grade_coming = None;
        }
        let coming = page.grade_coming.map(|(n, _)| n);
        let before = page.grade_nodes.len();
        page.grade_nodes
            .retain(|n| g.node(*n).is_some() || Some(*n) == coming);
        if page.grade_nodes.is_empty() && before > 0 {
            page.grade_nodes = last_corrector(g, |_| true).into_iter().collect();
        }
    }

    fn select_coming(&mut self, n: u32) {
        self.effects_page.grade_nodes = vec![n];
        self.effects_page.grade_coming = Some((n, Instant::now()));
    }

    pub(super) fn current_grade_node(&self, g: &Grade) -> Option<u32> {
        let n = *self.effects_page.grade_nodes.last()?;
        g.node(n).map(|_| n)
    }

    pub(super) fn pick_grade_nodes(&mut self, nodes: Vec<u32>) -> Result<(), String> {
        let (of, _, g) = self.grade_shown().ok_or(match self.effects_page.grading {
            GradeLevel::Clip => "There is no clip at the playhead",
            GradeLevel::Timeline => "There is no timeline",
        })?;
        if let Some(missing) = nodes.iter().find(|n| g.node(**n).is_none()) {
            return Err(format!("the grade has no node {missing}"));
        }
        let mut kept: Vec<u32> = Vec::new();
        for n in nodes {
            kept.retain(|k| *k != n);
            kept.push(n);
        }
        self.settle_grade(&of, &g);
        self.effects_page.grade_nodes = kept;
        Ok(())
    }

    pub(super) fn color_keys(&mut self, ctx: &egui::Context) {
        if self.effects_page.page != Page::Color || self.project().is_none() {
            return;
        }
        let bindings = self.snap.bindings.clone();
        let mut mine: Vec<Action> = Vec::new();
        let mut keep = |found: Option<(Action, bool)>| match found {
            Some((a, repeat)) => {
                if !repeat {
                    mine.push(a);
                }
                false
            }
            _ => true,
        };
        let clipboard = |a: &(Action, bool)| matches!(a.0, Action::Copy | Action::Paste);
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                let own = super::shortcuts::owner(&bindings, Panel::Color, e);
                let app = || super::shortcuts::owner(&bindings, Panel::App, e).filter(clipboard);
                keep(own.or_else(app))
            })
        });
        let struck = |a: Action| mine.contains(&a);
        for (action, add) in ADDS {
            if struck(action) {
                self.add_grade_node(add);
            }
        }
        if struck(Action::AddStaticKeyframe) {
            self.add_grade_key(vidcut_core::model::GradeKeyKind::Static);
        }
        let Some((of, _, g)) = self.grade_shown() else {
            return;
        };
        self.settle_grade(&of, &g);
        let corrector = self
            .current_grade_node(&g)
            .filter(|n| g.corrector(*n).is_some());
        for (action, act) in [
            (Action::ExtractNode, NodeAct::Extract),
            (Action::ResetNodeGrade, NodeAct::Reset),
        ] {
            if let Some(n) = corrector
                && struck(action)
            {
                self.grade_node_act(&of, &g, (act, n));
            }
        }
        if let Some(n) = corrector {
            if struck(Action::Copy) {
                self.send("grade.copy", params(&of, "clip", json!({ "node": n })));
            }
            if struck(Action::Paste) && self.snap.node_clipboard {
                self.send("grade.paste", params(&of, "clips", json!({ "node": n })));
            }
        }
        if struck(Action::DeleteNode) {
            let picked: Vec<u32> = (self.effects_page.grade_nodes.iter())
                .copied()
                .filter(|n| g.node(*n).is_some())
                .collect();
            self.delete_grade_nodes(&of, &g, &picked);
        }
        if struck(Action::SwitchSelectedNodes) {
            let picked = self.effects_page.grade_nodes.clone();
            let label = "Enable/Disable Selected Nodes";
            self.switch_grade_nodes(&of, &g, Some(&picked), label);
        }
        if struck(Action::SwitchAllNodes) {
            self.switch_grade_nodes(&of, &g, None, "Enable/Disable All Nodes");
        }
        for (action, forward) in [(Action::PreviousNode, false), (Action::NextNode, true)] {
            if struck(action)
                && let Some(n) = stepped(&g, self.current_grade_node(&g), forward)
            {
                self.send("gui.grade_nodes", json!({ "nodes": [n] }));
            }
        }
        for (action, act) in [
            (Action::ResetGradesKeepNodes, FieldAct::KeepNodes),
            (Action::ResetAllGrades, FieldAct::ResetAll),
        ] {
            if struck(action) {
                self.grade_field_act(&of, &g, act);
            }
        }
    }

    fn switch_grade_nodes(&mut self, of: &GradeOf, g: &Grade, picked: Option<&[u32]>, label: &str) {
        let nodes: Vec<(u32, bool)> = (g.nodes.iter().zip(1u32..))
            .filter(|(_, n)| picked.is_none_or(|p| p.contains(n)))
            .map(|(d, n)| (n, d.on))
            .collect();
        let to = match picked {
            Some(_) => match self.current_grade_node(g).and_then(|n| g.node(n)) {
                Some(d) => !d.on,
                None => return,
            },
            None => !nodes.iter().any(|(_, on)| *on),
        };
        let commands = (nodes.iter())
            .filter(|(_, on)| *on != to)
            .map(|(n, _)| {
                let more = json!({ "node": n, "on": to });
                command("grade.set", params(of, "clips", more))
            })
            .collect();
        self.effects_page.graded = None;
        self.send_all(commands, label);
    }

    fn delete_grade_nodes(&mut self, of: &GradeOf, g: &Grade, gone: &[u32]) {
        if gone.is_empty() {
            return;
        }
        let mut order = gone.to_vec();
        order.sort_unstable_by(|a, b| b.cmp(a));
        order.dedup();
        let commands = (order.iter())
            .map(|n| command("grade.remove", params(of, "clip", json!({ "node": n }))))
            .collect();
        let renumbered = |k: u32| k - order.iter().filter(|n| **n < k).count() as u32;
        let last = last_corrector(g, |k| !order.contains(&k));
        self.effects_page.grade_nodes = last.map(renumbered).into_iter().collect();
        self.effects_page.graded = None;
        self.send_all(commands, "Delete Node");
    }

    fn add_grade_node(&mut self, add: GradeAdd) {
        let Some((of, _, g)) = self.grade_shown() else {
            return;
        };
        self.settle_grade(&of, &g);
        let node = self.current_grade_node(&g);
        let mixer = node
            .and_then(|n| g.node(n))
            .is_some_and(|d| d.tool.is_mixer());
        if mixer && !matches!(add, GradeAdd::Serial | GradeAdd::Append) {
            return;
        }
        let more = match (add, node) {
            (GradeAdd::Append, _) => json!({ "add": add }),
            (_, Some(n)) => json!({ "add": add, "node": n }),
            (_, None) => return,
        };
        self.select_coming(g.last() + 1);
        self.send("grade.add", params(&of, "clip", more));
    }

    fn grade_dropped(&mut self, of: &GradeOf, dropped: Dropped<Port>) {
        match dropped {
            Dropped::Swap(Port::Node(a), Port::Node(b)) => {
                let more = json!({ "node": a, "with": b });
                self.send("grade.swap", params(of, "clip", more));
            }
            Dropped::Copy(Port::Node(a), Port::Node(b)) => {
                let copied = self
                    .project()
                    .map(|p| vidcut_core::ops::grade::copy(p, of, a));
                if let Some(Ok(settings)) = copied {
                    let more = json!({ "node": b, "settings": settings });
                    self.send("grade.paste", params(of, "clips", more));
                }
            }
            Dropped::Insert {
                node: Port::Node(n),
                wire: (from, to, slot),
                place,
            } => {
                let (Some(from), Some(to)) = (from_number(from), to_number(to)) else {
                    return;
                };
                let set = json!({ "node": n, "place": place });
                let take = json!({ "node": n, "from": from });
                let give = json!({ "node": to, "input": slot, "from": n });
                let commands = vec![
                    command("grade.set", params(of, "clips", set)),
                    command("grade.connect", params(of, "clip", take)),
                    command("grade.connect", params(of, "clip", give)),
                ];
                self.send_all(commands, "Insert Node");
            }
            _ => {}
        }
    }

    fn grade_node_act(&mut self, of: &GradeOf, g: &Grade, (act, n): (NodeAct, u32)) {
        let Some(d) = g.node(n) else {
            return;
        };
        match act {
            NodeAct::Add(add) => {
                self.effects_page.grade_nodes = vec![n];
                self.add_grade_node(add);
                return;
            }
            NodeAct::Delete => {
                self.delete_grade_nodes(of, g, &[n]);
                return;
            }
            NodeAct::Label => {
                let page = &mut self.effects_page;
                page.grade_view.name(Port::Node(n), d.label.clone());
                return;
            }
            _ => {}
        }
        let set = |more: Value| {
            let mut more = more;
            more["node"] = json!(n);
            ("grade.set", params(of, "clips", more))
        };
        let (method, p) = match act {
            NodeAct::Add(_) | NodeAct::Delete | NodeAct::Label => return,
            NodeAct::Reset => {
                let more = json!({ "node": n, "at": self.snap.timeline.position });
                ("grade.clear", params(of, "clips", more))
            }
            NodeAct::Toggle => set(json!({ "on": !d.on })),
            NodeAct::Lock => set(json!({ "locked": !d.locked })),
            NodeAct::Morph(to) => set(json!({ "mixer": to })),
            NodeAct::Mode(mode) => set(json!({ "mode": mode })),
            NodeAct::AddInput => ("grade.connect", params(of, "clip", json!({ "node": n }))),
            NodeAct::Extract => {
                let more = json!({ "node": n, "extract": true });
                ("grade.remove", params(of, "clip", more))
            }
        };
        self.effects_page.graded = None;
        self.send(method, p);
    }

    fn grade_field_act(&mut self, of: &GradeOf, g: &Grade, act: FieldAct) {
        let (method, more) = match act {
            FieldAct::Zoom(zoom) => {
                self.send("gui.node_view", json!({ "zoom": zoom }));
                return;
            }
            FieldAct::Wide => {
                let wide = !self.effects_page.grade_wide;
                self.send("gui.node_view", json!({ "wide": wide }));
                return;
            }
            FieldAct::Cleanup => {
                let area = self.effects_page.grade_view.area();
                ("grade.cleanup", json!({ "area": area }))
            }
            FieldAct::Corrector => {
                let at = self.effects_page.grade_spot.unwrap_or_default();
                self.select_coming(g.last() + 1);
                (
                    "grade.add",
                    json!({ "add": GradeAdd::Corrector, "place": at }),
                )
            }
            FieldAct::KeepNodes => {
                let at = self.snap.timeline.position;
                ("grade.clear", json!({ "keep_nodes": true, "at": at }))
            }
            FieldAct::ResetAll => {
                self.effects_page.grade_nodes = vec![1];
                ("grade.clear", json!({}))
            }
        };
        let field = match method {
            "grade.add" | "grade.cleanup" => "clip",
            _ => "clips",
        };
        self.effects_page.graded = None;
        self.send(method, params(of, field, more));
    }

    pub(super) fn grade_nodes_panel(&mut self, ui: &mut Ui) {
        let Some((of, _, stored)) = self.grade_shown() else {
            node_editor::empty(
                ui,
                match self.effects_page.grading {
                    GradeLevel::Clip => "No Clip at the Playhead",
                    GradeLevel::Timeline => "No Timeline",
                },
            );
            return;
        };
        self.settle_grade(&of, &stored);
        let switching = self.snap.switching_clips;
        let mut switch_to = None;
        let listed = self.effects_page.grade_list;
        let mut list_to = None;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(icon::LIST_BULLETS, |ui| {
                    for (list, name) in [(false, "Graph View"), (true, "List View")] {
                        if ui.radio(list == listed, name).clicked() {
                            list_to = Some(list);
                            ui.close();
                        }
                    }
                });
                ui.menu_button(icon::DOTS_THREE, |ui| {
                    ui.menu_button("Switching Clips Selects", |ui| {
                        for (value, name) in SWITCHING {
                            if ui.radio(value == switching, name).clicked() {
                                switch_to = Some(value);
                                ui.close();
                            }
                        }
                    });
                });
            });
        });
        if let Some(value) = switch_to {
            let more = json!({ "color": { "switching_clips": value } });
            self.send("preferences.set", more);
        }
        if let Some(list) = list_to {
            self.send("gui.node_view", json!({ "list": list }));
        }
        let salt = match &of {
            GradeOf::Clip(id) => format!("grade-node-{id}"),
            GradeOf::Timeline => "grade-node-timeline".to_string(),
        };
        let selected: Vec<Port> = (self.effects_page.grade_nodes.iter())
            .map(|n| Port::Node(*n))
            .collect();
        let graph = Graph {
            salt,
            tiles: tiles(&stored),
            wires: wires(&stored),
            selected: &selected,
            active: CURRENT,
        };
        let delete_key = self
            .snap
            .bindings
            .first(Action::DeleteNode)
            .unwrap_or_default();
        let mut node_act: Option<(NodeAct, u32)> = None;
        let mut field_act: Option<FieldAct> = None;
        let menu = |ui: &mut Ui, p: &Port| {
            let Port::Node(n) = *p else {
                return;
            };
            let Some(d) = stored.node(n) else {
                return;
            };
            let mut act = None;
            node_menu(ui, d, &delete_key, &mut act);
            if let Some(a) = act {
                node_act = Some((a, n));
            }
        };
        let done = match listed {
            true => node_editor::list(ui, &mut self.effects_page.grade_view, &graph, menu),
            false => node_editor::show(ui, &mut self.effects_page.grade_view, &graph, menu, |ui| {
                field_menu(ui, &mut field_act)
            }),
        };
        if let Some((inside, off)) = done.band {
            let inside: Vec<u32> = (inside.into_iter())
                .filter_map(|p| match p {
                    Port::Node(n) => Some(n),
                    _ => None,
                })
                .collect();
            let mut nodes = self.effects_page.grade_nodes.clone();
            let current = nodes.last().copied();
            if off {
                nodes.retain(|n| !inside.contains(n) || Some(*n) == current);
            } else {
                let others = inside.into_iter().filter(|n| Some(*n) != current);
                nodes = others.chain(current).collect();
            }
            self.send("gui.grade_nodes", json!({ "nodes": nodes }));
        }
        if let Some(at) = done.spot {
            self.effects_page.grade_spot = Some(at);
        }
        let mut commands = done
            .link
            .map(|l| grade_links(&stored, &of, &l))
            .unwrap_or_default();
        if let Some((to, slot)) = done.unlink {
            commands.extend(connect(&of, to, Some(slot), None));
        }
        self.send_all(commands, "Link Nodes");
        let moves = (done.moved.into_iter())
            .filter_map(|(p, to)| match p {
                Port::Node(n) => {
                    let more = json!({ "node": n, "place": to });
                    Some(command("grade.set", params(&of, "clips", more)))
                }
                _ => None,
            })
            .collect();
        self.send_all(moves, "Move Nodes");
        if let Some(d) = done.dropped {
            self.grade_dropped(&of, d);
        }
        if let Some(Port::Node(n)) = done.pointed {
            self.effects_page.grade_nodes = vec![n];
        }
        if let Some(Port::Node(n)) = done.switched {
            node_act = Some((NodeAct::Toggle, n));
        }
        if let Some((Port::Node(n), label)) = done.named {
            let more = json!({ "node": n, "label": label });
            self.send("grade.set", params(&of, "clips", more));
        }
        if let Some(act) = node_act {
            self.grade_node_act(&of, &stored, act);
        }
        if let Some(act) = field_act {
            self.grade_field_act(&of, &stored, act);
        }
        let mut nodes = self.effects_page.grade_nodes.clone();
        match done.picked {
            Some((Port::Node(n), Pick::Active)) => {
                nodes.retain(|k| *k != n);
                nodes.push(n);
            }
            Some((Port::Node(n), Pick::Toggle)) if !nodes.contains(&n) => {
                nodes.insert(nodes.len().saturating_sub(1), n);
            }
            Some((Port::Node(n), Pick::Only)) => nodes = vec![n],
            None if done.clicked && done.unlink.is_none() => {
                nodes = nodes.last().copied().into_iter().collect();
            }
            _ => return,
        }
        if nodes != self.effects_page.grade_nodes {
            self.send("gui.grade_nodes", json!({ "nodes": nodes }));
        }
    }
}
