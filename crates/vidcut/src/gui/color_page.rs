use std::time::{Duration, Instant};

use egui::{Color32, CursorIcon, DragValue, Label, Rect, Response, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as icon;
use serde_json::{Map, Value, json};
use vidcut_av::screen::{ShotsOf, ShotsWanted};
use vidcut_core::model::{
    BlendMode, Corrector, Grade, GradeNode, GradeTool, GradeWindow, KeyInput, KeyMix, KeyOutput,
    Picture, Project, WindowKind,
};
use vidcut_core::ops::grade::{GradeOf, checked, corrector_checked, printed, taken_out};
use vidcut_core::ops::picture::takes_effects;
use vidcut_core::preferences::SwitchingClips;
use vidcut_core::request::{
    GradeAdd, GradeLevel, GradeMixer, GradePalette, NodeZoom, Page, PrimariesView, PrinterColor,
};
use vidcut_core::shortcuts::{Action, Bindings, Panel};

use super::grade_windows::{self, WindowAct};
use super::node_editor::{
    self, Badge, Dropped, Edge, End, Graph, Inlet, Link, MASK_KNOT, Outlet, Pick, RGB_KNOT,
    SHOT_HEIGHT, Shot, Side, Spot, Tile, Wire,
};
use super::theme;
use super::workspace::{NodeShots, Workspace};

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

const SHOT_STEP: u32 = 16;

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

const ADDS: [(Action, GradeAdd); 6] = [
    (Action::AddSerialNode, GradeAdd::Serial),
    (Action::AddNodeBefore, GradeAdd::Before),
    (Action::AddParallelNode, GradeAdd::Parallel),
    (Action::AddLayerNode, GradeAdd::Layer),
    (Action::AppendNode, GradeAdd::Append),
    (Action::AddOutsideNode, GradeAdd::Outside),
];

const KEY_PORT: usize = 1;

#[derive(Debug, Clone, Copy)]
enum NodesItem {
    Key(Action),
    Act(NodeAct),
}

const NODES_MENU: [&[(&str, NodesItem)]; 5] = [
    &[
        ("Previous Node", NodesItem::Key(Action::PreviousNode)),
        ("Next Node", NodesItem::Key(Action::NextNode)),
    ],
    &[
        ("Add Serial Node", NodesItem::Key(Action::AddSerialNode)),
        (
            "Add Serial Before Current",
            NodesItem::Key(Action::AddNodeBefore),
        ),
        ("Add Parallel Node", NodesItem::Key(Action::AddParallelNode)),
        ("Add Layer Node", NodesItem::Key(Action::AddLayerNode)),
        ("Append a Node", NodesItem::Key(Action::AppendNode)),
        (
            "Add Outside Node",
            NodesItem::Act(NodeAct::Add(GradeAdd::Outside)),
        ),
    ],
    &[
        (
            "Enable/Disable Selected Nodes",
            NodesItem::Key(Action::SwitchSelectedNodes),
        ),
        (
            "Enable/Disable All Nodes",
            NodesItem::Key(Action::SwitchAllNodes),
        ),
    ],
    &[
        ("Extract Current Node", NodesItem::Key(Action::ExtractNode)),
        ("Delete Current Node", NodesItem::Act(NodeAct::Delete)),
    ],
    &[("Label Selected Node", NodesItem::Act(NodeAct::Label))],
];

const MIXER_ADD_MENU: [(GradeAdd, &str); 2] = [
    (GradeAdd::Serial, "Add Serial"),
    (GradeAdd::Before, "Add Serial Before"),
];

const ADD_MENU: [(GradeAdd, &str); 6] = [
    (GradeAdd::Serial, "Add Serial"),
    (GradeAdd::Before, "Add Before Current"),
    (GradeAdd::Parallel, "Add Parallel"),
    (GradeAdd::Layer, "Add Layer"),
    (GradeAdd::Append, "Append Node"),
    (GradeAdd::Outside, "Add Outside"),
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
    KeyMixer,
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

pub(super) struct WindowName {
    of: GradeOf,
    node: u32,
    index: usize,
    text: String,
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
    let GradeTool::Corrector(c) = &d.tool else {
        return Vec::new();
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
            !d.key_input.is_usual()
                || std::iter::once(&d.key)
                    .chain(d.keys.iter().map(|k| &k.key))
                    .any(|k| !k.is_usual()),
            icon::SQUARE_HALF,
            "Key",
        ),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, icon, tip)| Badge { icon, tip })
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

const KEY_SWATCH: Vec2 = Vec2::new(96.0, 54.0);

fn key_output(ui: &mut Ui, changes: &mut Vec<Change>, (title, key): (&str, KeyOutput)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(KEY_SWATCH, Sense::hover());
        let level = (key.of(1.0) * 255.0).round() as u8;
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_gray(level));
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.strong(title);
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

fn key_input(ui: &mut Ui, changes: &mut Vec<Change>, input: KeyInput) {
    ui.horizontal(|ui| {
        ui.strong("Key Input");
        let mut invert = input.invert;
        if ui.toggle_value(&mut invert, "Invert").changed() {
            changes.push(Change {
                field: "key_input_invert",
                value: json!(invert),
            });
        }
        let mut matte = !input.mask;
        if ui.toggle_value(&mut matte, "Matte").changed() {
            changes.push(Change {
                field: "key_input_mask",
                value: json!(!matte),
            });
        }
    });
    ui.horizontal(|ui| {
        let (gain, range) = ((0.0, 2.0), (-1.0, 1.0));
        slider(
            ui,
            changes,
            ("Gain", "key_input_gain"),
            input.gain,
            0.005,
            gain,
        );
        let offset = ("Offset", "key_input_offset");
        slider(ui, changes, offset, input.offset, 0.005, range);
        slider(
            ui,
            changes,
            ("Blur R.", "key_input_blur"),
            input.blur,
            0.005,
            range,
        );
        let ratio = ("Blur H/V", "key_input_blur_ratio");
        slider(ui, changes, ratio, input.blur_ratio, 0.005, range);
    });
}

fn key_mix_row(ui: &mut Ui, changes: &mut Vec<Change>, (i, m): (usize, KeyMix)) {
    ui.horizontal(|ui| {
        ui.strong(format!("Input Link {}", i + 1));
        let mut invert = m.invert;
        if ui.toggle_value(&mut invert, "Invert").changed() {
            changes.push(Change {
                field: "mix_invert",
                value: json!(invert),
            });
        }
        let mut matte = m.matte;
        if ui.toggle_value(&mut matte, "Matte").changed() {
            changes.push(Change {
                field: "mix_matte",
                value: json!(matte),
            });
        }
        slider(ui, changes, ("Gain", "mix_gain"), m.gain, 0.005, (0.0, 2.0));
        let offset = ("Offset", "mix_offset");
        slider(ui, changes, offset, m.offset, 0.005, (-1.0, 1.0));
    });
}

fn primaries_body(
    ui: &mut Ui,
    (g, mode): (&Corrector, PrimariesView),
    changes: &mut Vec<Change>,
) -> Option<(PrinterColor, f64)> {
    let wheels = match mode {
        PrimariesView::Log => logs(g),
        PrimariesView::Wheels | PrimariesView::Bars => primaries(g),
    };
    let mut point = None;
    ui.horizontal(|ui| {
        for w in &wheels {
            let made = match mode {
                PrimariesView::Bars => {
                    let (made, pressed) = bars(ui, w);
                    point = point.or(pressed);
                    made
                }
                PrimariesView::Wheels | PrimariesView::Log => wheel(ui, w),
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

fn key_inputted(input: KeyInput, c: &Change) -> KeyInput {
    let mut input = input;
    let number = |v: &Value, now: f64| v.as_f64().unwrap_or(now);
    match (c.field, &c.value) {
        ("key_input_gain", v) => input.gain = number(v, input.gain),
        ("key_input_offset", v) => input.offset = number(v, input.offset),
        ("key_input_blur", v) => input.blur = number(v, input.blur),
        ("key_input_blur_ratio", v) => input.blur_ratio = number(v, input.blur_ratio),
        ("key_input_invert", Value::Bool(v)) => input.invert = *v,
        ("key_input_mask", Value::Bool(v)) => input.mask = *v,
        _ => {}
    }
    input
}

fn mixed(m: KeyMix, c: &Change) -> KeyMix {
    let mut m = m;
    match (c.field, &c.value) {
        ("mix_gain", Value::Number(v)) => m.gain = v.as_f64().unwrap_or(m.gain),
        ("mix_offset", Value::Number(v)) => m.offset = v.as_f64().unwrap_or(m.offset),
        ("mix_invert", Value::Bool(v)) => m.invert = *v,
        ("mix_matte", Value::Bool(v)) => m.matte = *v,
        _ => {}
    }
    m
}

fn key_input_changes(from: KeyInput, to: KeyInput) -> Map<String, Value> {
    let pairs = [
        ("key_input_gain", from.gain, to.gain),
        ("key_input_offset", from.offset, to.offset),
        ("key_input_blur", from.blur, to.blur),
        ("key_input_blur_ratio", from.blur_ratio, to.blur_ratio),
    ];
    let mut out: Map<String, Value> = pairs
        .into_iter()
        .filter(|(_, a, b)| a != b)
        .map(|(k, _, b)| (k.to_string(), json!(b)))
        .collect();
    if from.invert != to.invert {
        out.insert("key_input_invert".into(), json!(to.invert));
    }
    if from.mask != to.mask {
        out.insert("key_input_mask".into(), json!(to.mask));
    }
    out
}

fn mix_changes(from: KeyMix, to: KeyMix) -> Map<String, Value> {
    let mut out = Map::new();
    if from.invert != to.invert {
        out.insert("mix_invert".into(), json!(to.invert));
    }
    if from.matte != to.matte {
        out.insert("mix_matte".into(), json!(to.matte));
    }
    if from.gain != to.gain {
        out.insert("mix_gain".into(), json!(to.gain));
    }
    if from.offset != to.offset {
        out.insert("mix_offset".into(), json!(to.offset));
    }
    out
}

fn mixer_sets(from: &Grade, to: &Grade, n: u32) -> Vec<Map<String, Value>> {
    let (Some(a), Some(b)) = (from.node(n), to.node(n)) else {
        return Vec::new();
    };
    let (GradeTool::KeyMixer { mix: was }, GradeTool::KeyMixer { mix: now }) = (&a.tool, &b.tool)
    else {
        return Vec::new();
    };
    let mut out: Vec<Map<String, Value>> = (was.iter().zip(now).enumerate())
        .filter_map(|(i, (x, y))| {
            let mut set = mix_changes(*x, *y);
            (!set.is_empty()).then(|| {
                set.insert("input".into(), json!(i));
                set
            })
        })
        .collect();
    let keys = key_changes(a.key, b.key);
    if !keys.is_empty() {
        out.push(keys);
    }
    out
}

fn window_sets(from: &Grade, to: &Grade, n: u32) -> Vec<(usize, Map<String, Value>)> {
    let (Some(a), Some(b)) = (from.node(n), to.node(n)) else {
        return Vec::new();
    };
    if a.windows.len() != b.windows.len() {
        return Vec::new();
    }
    let pairs = a.windows.iter().zip(&b.windows).enumerate();
    pairs
        .map(|(i, (a, b))| (i, grade_windows::window_changes(a, b)))
        .filter(|(_, m)| !m.is_empty())
        .collect()
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

fn window_target(of: &GradeOf) -> (String, Value) {
    match of {
        GradeOf::Clip(id) => ("clip".into(), json!(id)),
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
        GradeTool::KeyMixer { .. } => "Key Mixer",
    }
}

fn shot_of(shots: &NodeShots, n: u32) -> Shot {
    let cell = shots.cells.iter().find(|(k, ..)| *k == n);
    Shot {
        picture: cell.and_then(|&(_, uv, size)| Some((shots.texture?, uv, size))),
    }
}

fn tiles(g: &Grade, shots: Option<&NodeShots>) -> Vec<Tile<Port>> {
    let line = Stroke::new(1.5, theme::colors().text_dim);
    let input = |slot: usize, name: String| Inlet {
        slot,
        side: Side::Left,
        knot: RGB_KNOT,
        wire: line,
        name,
    };
    let key_input = |slot: usize, name: String| Inlet {
        slot,
        side: Side::Left,
        knot: MASK_KNOT,
        wire: Stroke::new(1.5, MASK_KNOT),
        name,
    };
    let outlet = |slot: usize, name: &str| Outlet {
        slot,
        name: name.into(),
        color: if slot == KEY_PORT {
            MASK_KNOT
        } else {
            RGB_KNOT
        },
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
        locked: None,
        outlets: vec![outlet(0, "Source")],
        inlets: Vec::new(),
        edge: Some(Edge::Left),
        shot: None,
    }];
    for (d, n) in g.nodes.iter().zip(1u32..) {
        let (label, inlets, outlets, shot) = match &d.tool {
            GradeTool::Corrector(_) => (
                format!("{n:02}"),
                vec![
                    input(0, "RGB Input".into()),
                    key_input(KEY_PORT, "Key Input".into()),
                ],
                vec![outlet(0, "RGB Output"), outlet(KEY_PORT, "Key Output")],
                shots.map(|s| shot_of(s, n)),
            ),
            GradeTool::KeyMixer { .. } => (
                kind(&d.tool).to_string(),
                (0..d.inputs.len())
                    .map(|i| key_input(i, format!("Input Link {}", i + 1)))
                    .collect(),
                vec![outlet(KEY_PORT, "Key Output")],
                None,
            ),
            tool => (
                kind(tool).to_string(),
                (0..d.inputs.len())
                    .map(|i| input(i, format!("Input {}", i + 1)))
                    .collect(),
                vec![outlet(0, "RGB Output")],
                None,
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
            locked: Some(d.locked),
            outlets,
            inlets,
            edge: None,
            shot,
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
        locked: None,
        outlets: Vec::new(),
        inlets: vec![input(0, "Output".into())],
        edge: Some(Edge::Right),
        shot: None,
    });
    out
}

fn wires(g: &Grade) -> Vec<Wire<Port>> {
    let mut out = Vec::new();
    for (d, n) in g.nodes.iter().zip(1u32..) {
        let keys = d.takes_keys();
        for (slot, v) in d.inputs.iter().enumerate() {
            if let Some(v) = v {
                out.push(Wire {
                    from: port(*v),
                    out: if keys { KEY_PORT } else { 0 },
                    to: Port::Node(n),
                    slot,
                    drops: !keys,
                });
            }
        }
        if let Some(k) = d.key_from {
            out.push(Wire {
                from: port(k),
                out: KEY_PORT,
                to: Port::Node(n),
                slot: KEY_PORT,
                drops: false,
            });
        }
    }
    if let Some(o) = g.output {
        out.push(Wire {
            from: port(o),
            out: 0,
            to: Port::Output,
            slot: 0,
            drops: true,
        });
    }
    out
}

fn inlet(g: &Grade, to: Port, slot: Option<usize>) -> Option<(Option<usize>, bool)> {
    match to {
        Port::Node(n) => Some(match (&g.node(n)?.tool, slot) {
            (GradeTool::KeyMixer { .. }, s) => (s, true),
            (GradeTool::Corrector(_), Some(KEY_PORT)) => (None, true),
            (_, s) => (s, false),
        }),
        Port::Output => Some((Some(0), false)),
        Port::Source => None,
    }
}

fn gives(g: &Grade, from: Port, key: bool) -> bool {
    match (from, key) {
        (Port::Source, false) => true,
        (Port::Node(n), key) => g.node(n).is_some_and(|d| match &d.tool {
            GradeTool::Corrector(_) => true,
            GradeTool::KeyMixer { .. } => key,
            _ => !key,
        }),
        _ => false,
    }
}

fn linked_now(g: &Grade, to: Port, slot: usize) -> Option<Port> {
    match to {
        Port::Node(n) => {
            let d = g.node(n)?;
            let v = match inlet(g, to, Some(slot))? {
                (None, true) => d.key_from,
                _ => d.inputs.get(slot).copied().flatten(),
            };
            v.map(port)
        }
        Port::Output => g.output.map(port),
        Port::Source => None,
    }
}

fn connect(
    g: &Grade,
    of: &GradeOf,
    (to, slot): (Port, Option<usize>),
    from: Option<Port>,
) -> Option<Value> {
    let (input, key) = inlet(g, to, slot)?;
    let mut more = json!({ "node": to_number(to)? });
    if let Some(s) = input {
        more["input"] = json!(s);
    }
    if key {
        more["key"] = json!(true);
    }
    if let Some(f) = from.and_then(from_number) {
        more["from"] = json!(f);
    }
    Some(command("grade.connect", params(of, "clip", more)))
}

fn grade_links(g: &Grade, of: &GradeOf, l: &Link<Port>) -> Vec<Value> {
    let keyed = |dst: Port, s: usize| inlet(g, dst, Some(s)).map(|(_, key)| key);
    let onto = |dst: Port, key: bool| -> Option<Option<usize>> {
        let tool = match dst {
            Port::Node(n) => &g.node(n)?.tool,
            Port::Output => return (!key).then_some(Some(0)),
            Port::Source => return None,
        };
        match (tool, key) {
            (GradeTool::Corrector(_), true) => Some(Some(KEY_PORT)),
            (GradeTool::Corrector(_), false) => Some(Some(0)),
            (GradeTool::KeyMixer { .. }, true) => Some(None),
            (GradeTool::KeyMixer { .. }, false) | (_, true) => None,
            (_, false) => Some(None),
        }
    };
    let new = match (&l.anchor, &l.spot) {
        (End::Output(src, out), Spot::Knot(End::Input(dst, s))) => {
            let key = *out == KEY_PORT;
            (keyed(*dst, *s) == Some(key)).then_some((*dst, Some(*s), *src))
        }
        (End::Output(src, out), Spot::Tile(dst)) => match l.pulled {
            Some((pd, ps)) if pd == *dst => Some((*dst, Some(ps), *src)),
            _ => onto(*dst, *out == KEY_PORT).map(|s| (*dst, s, *src)),
        },
        (End::Input(dst, s), Spot::Knot(End::Output(src, out))) => {
            let key = *out == KEY_PORT;
            (keyed(*dst, *s) == Some(key)).then_some((*dst, Some(*s), *src))
        }
        (End::Input(dst, s), Spot::Tile(src)) => keyed(*dst, *s)
            .filter(|key| gives(g, *src, *key))
            .map(|_| (*dst, Some(*s), *src)),
        _ => None,
    };
    let new = new.filter(|(d, _, s)| d != s);
    let mut out = Vec::new();
    if let Some((pd, ps)) = l.pulled
        && new.is_none_or(|(d, s, _)| d != pd || s != Some(ps))
    {
        out.extend(connect(g, of, (pd, Some(ps)), None));
    }
    if let Some((d, s, src)) = new
        && s.is_none_or(|s| linked_now(g, d, s) != Some(src))
    {
        out.extend(connect(g, of, (d, s), Some(src)));
    }
    out
}

fn node_menu(ui: &mut Ui, d: &GradeNode, keys: &Bindings, act: &mut Option<NodeAct>) {
    let key = |a: Action| keys.first(a).unwrap_or_default();
    let mut pick = |ui: &mut Ui, on: bool, button: egui::Button, a: NodeAct| {
        if ui.add_enabled(on, button).clicked() {
            *act = Some(a);
            ui.close();
        }
    };
    let adds: &[(GradeAdd, &str)] = match &d.tool {
        GradeTool::Corrector(_) => &ADD_MENU,
        GradeTool::KeyMixer { .. } => &[],
        _ => &MIXER_ADD_MENU,
    };
    if !adds.is_empty() {
        ui.menu_button("Add Node", |ui| {
            for &(add, text) in adds {
                let action = ADDS.iter().find(|(_, a)| *a == add);
                let shown = action.map(|(a, _)| key(*a)).unwrap_or_default();
                let button = egui::Button::new(text).shortcut_text(shown);
                pick(ui, true, button, NodeAct::Add(add));
            }
        });
    }
    let delete = || egui::Button::new("Delete Node").shortcut_text(key(Action::DeleteNode));
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
        GradeTool::KeyMixer { .. } => {
            let reset = egui::Button::new("Reset Node Grade");
            pick(ui, true, reset, NodeAct::Reset);
            let lock = egui::Button::selectable(d.locked, "Lock Node");
            pick(ui, true, lock, NodeAct::Lock);
            pick(ui, true, delete(), NodeAct::Delete);
            let more = egui::Button::new("Add One Input");
            pick(ui, true, more, NodeAct::AddInput);
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
    ui.menu_button("Add Node", |ui| {
        pick(ui, "Corrector", FieldAct::Corrector);
        pick(ui, "Key Mixer", FieldAct::KeyMixer);
    });
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
    let parts = [
        ("key_input_", serde_json::to_value(KeyInput::default())),
        ("mix_", serde_json::to_value(KeyMix::default())),
        ("key_", serde_json::to_value(KeyOutput::default())),
    ];
    let (values, field) = parts
        .into_iter()
        .find_map(|(lead, v)| Some((v, field.strip_prefix(lead)?)))
        .unwrap_or_else(|| (serde_json::to_value(Corrector::default()), field));
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
        let p = self.project()?;
        let c = p.clip(id)?;
        let mut picture = c.picture.clone().unwrap_or_default();
        checked(&h.grade).ok()?;
        picture.grade = (!h.grade.is_usual(p.frame_size())).then(|| h.grade.clone());
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
                let usual = || Grade::usual(p.frame_size());
                Some((GradeOf::Clip(id), name, stored.unwrap_or_else(usual)))
            }
            GradeLevel::Timeline => {
                if !p.compound_path.is_empty() {
                    return None;
                }
                let t = p.timeline()?;
                let stored = (t.grade.clone()).unwrap_or_else(|| Grade::usual(p.frame_size()));
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
        let cap = match (self.effects_page.palette, full) {
            (GradePalette::Primaries, _) | (_, None) => inner,
            (_, Some(h)) => h.min(inner),
        };
        egui::ScrollArea::vertical()
            .id_salt("primaries-scroll")
            .max_height(cap)
            .min_scrolled_height(cap)
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

    pub(super) fn held_grade(&self, of: &GradeOf, n: u32) -> Option<&Grade> {
        (self.effects_page.graded.as_ref())
            .filter(|h| h.of == *of && h.node == n)
            .map(|h| &h.grade)
    }

    pub(super) fn grade_unsent(&self) -> bool {
        (self.effects_page.graded.as_ref()).is_some_and(|h| h.sent.is_none())
    }

    pub(super) fn hold_grade(&mut self, of: &GradeOf, n: u32, grade: Grade) {
        self.effects_page.graded = Some(Graded {
            of: of.clone(),
            node: n,
            frame: self.grade_frame(of),
            at: self.snap.timeline.position,
            grade,
            sent: None,
        });
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
            let mixer = node.filter(|n| grade.node(*n).is_some_and(GradeNode::takes_keys));
            if let Some(n) = mixer {
                let stored_grade = (&stored, &grade);
                self.key_mixer_palette(ui, (&of, &name, level), stored_grade, (n, frame, at));
                return false;
            }
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
        let (palette, mode) = (self.effects_page.palette, self.effects_page.primaries);
        let (mut palette_picked, mut mode_picked) = (None, None);
        let mut changes: Vec<Change> = Vec::new();
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.strong(name);
            ui.label(format!("Node {n:02}"));
            ui.separator();
            for (p, text) in [
                (GradePalette::Primaries, "Primaries"),
                (GradePalette::Window, "Window"),
                (GradePalette::Key, "Key"),
            ] {
                if ui.selectable_label(palette == p, text).clicked() && palette != p {
                    palette_picked = Some(p);
                }
            }
            ui.separator();
            if palette != GradePalette::Primaries {
                reset = ui.button("Reset").clicked();
                picked = grade_switch(ui, level);
                return;
            }
            for (m, text) in [
                (PrimariesView::Wheels, "Wheels"),
                (PrimariesView::Bars, "Bars"),
                (PrimariesView::Log, "Log"),
            ] {
                if ui.selectable_label(mode == m, text).clicked() && mode != m {
                    mode_picked = Some(m);
                }
            }
            ui.separator();
            reset = ui.button("Reset").clicked();
            ui.separator();
            let log = mode == PrimariesView::Log;
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
        let mut view = Map::new();
        if let Some(p) = palette_picked {
            view.insert("palette".into(), json!(p));
        }
        if let Some(m) = mode_picked {
            view.insert("primaries".into(), json!(m));
        }
        if let Some(l) = picked {
            view.insert("grade".into(), json!(l));
        }
        if !view.is_empty() {
            view.insert("page".into(), json!(Page::Color));
            self.send("gui.page", Value::Object(view));
        }
        let key_now = key_output_held(&grade, n, frame).unwrap_or_default();
        let mut window_acts = Vec::new();
        let point = match palette {
            GradePalette::Key => {
                let input = grade.node(n).map(|d| d.key_input).unwrap_or_default();
                key_input(ui, &mut changes, input);
                ui.separator();
                key_output(ui, &mut changes, ("Key Output", key_now));
                None
            }
            GradePalette::Window => {
                window_acts = self.window_body(ui, &grade, (&of, n));
                None
            }
            GradePalette::Primaries => primaries_body(ui, (&g, mode), &mut changes),
        };
        let (field, target) = named(&of);
        let locked = grade.node(n).is_some_and(|d| d.locked);
        if locked {
            changes.clear();
            window_acts.retain(|a| matches!(a, WindowAct::Pick(_) | WindowAct::Copy));
        }
        for act in window_acts {
            self.window_act(&of, &grade, (n, frame, at), act);
        }
        if reset && !locked {
            self.effects_page.graded = None;
            self.effects_page.window_name = None;
            let node = ("node".to_string(), json!(n));
            let at = ("at".to_string(), json!(self.snap.timeline.position));
            let part = ("palette".to_string(), json!(palette));
            let params = Map::from_iter([(field, target), node, at, part]);
            self.send("grade.clear", Value::Object(params));
            return palette == GradePalette::Primaries;
        }
        if !changes.is_empty() {
            let (inputs, changes): (Vec<Change>, Vec<Change>) = changes
                .into_iter()
                .partition(|c| c.field.starts_with("key_input_"));
            let (keys, values): (Vec<Change>, Vec<Change>) = changes
                .into_iter()
                .partition(|c| c.field.starts_with("key_"));
            let c = values
                .into_iter()
                .fold(g, |g, c| changed_corrector(&g, c.field, c.value));
            let mut next = with_corrector(&grade, n, frame, c);
            if let Some(d) = next.node_mut(n) {
                d.change_key_output_at(frame, |k| keys.iter().fold(k, keyed));
                d.key_input = inputs.iter().fold(d.key_input, key_inputted);
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
                let inputs = (stored.node(n), h.grade.node(n));
                if let (Some(a), Some(b)) = inputs {
                    params.extend(key_input_changes(a.key_input, b.key_input));
                }
                Some((params, window_sets(&stored, &h.grade, n)))
            }
            _ => None,
        };
        match ready {
            Some((params, sets)) if params.is_empty() && sets.is_empty() => {
                self.effects_page.graded = None;
            }
            Some((mut params, sets)) => {
                let (one, clip) = window_target(&of);
                for (i, mut set) in sets {
                    set.insert(one.clone(), clip.clone());
                    set.insert("node".into(), json!(n));
                    set.insert("window".into(), json!(i));
                    self.send("grade.window.set", Value::Object(set));
                }
                if !params.is_empty() {
                    params.insert(field, target);
                    params.insert("node".into(), json!(n));
                    params.insert("at".into(), json!(at));
                    self.send("grade.set", Value::Object(params));
                }
            }
            None => {}
        }
        if let Some(point) = point.filter(|_| !locked) {
            self.printer_point(point);
        }
        palette == GradePalette::Primaries
    }

    fn key_mixer_palette(
        &mut self,
        ui: &mut Ui,
        (of, name, level): (&GradeOf, &str, GradeLevel),
        (stored, grade): (&Grade, &Grade),
        (n, frame, at): (u32, i64, i64),
    ) {
        let Some(d) = grade.node(n) else {
            return;
        };
        let GradeTool::KeyMixer { mix } = &d.tool else {
            return;
        };
        let (mut picked, mut reset) = (None, false);
        ui.horizontal(|ui| {
            ui.strong(name);
            ui.separator();
            ui.label(kind(&d.tool));
            ui.separator();
            reset = ui.button("Reset").clicked();
            picked = grade_switch(ui, level);
        });
        if let Some(l) = picked {
            self.send("gui.page", json!({ "page": Page::Color, "grade": l }));
        }
        let mut mixes: Vec<(usize, Change)> = Vec::new();
        let linked = (mix.iter().copied().enumerate()).filter(|(i, _)| d.inputs[*i].is_some());
        for (i, m) in linked {
            let mut changes = Vec::new();
            key_mix_row(ui, &mut changes, (i, m));
            mixes.extend(changes.into_iter().map(|c| (i, c)));
        }
        ui.separator();
        let mut keys = Vec::new();
        key_output(ui, &mut keys, ("Output", d.key));
        if d.locked {
            return;
        }
        if reset {
            self.effects_page.graded = None;
            let (field, target) = named(of);
            let node = ("node".to_string(), json!(n));
            let part = ("palette".to_string(), json!(GradePalette::Key));
            let params = Map::from_iter([(field, target), node, part]);
            self.send("grade.clear", Value::Object(params));
            return;
        }
        if !mixes.is_empty() || !keys.is_empty() {
            let mut next = grade.clone();
            if let Some(d) = next.node_mut(n) {
                d.key = keys.iter().fold(d.key, keyed);
                if let GradeTool::KeyMixer { mix } = &mut d.tool {
                    for (i, c) in &mixes {
                        if let Some(m) = mix.get_mut(*i) {
                            *m = mixed(*m, c);
                        }
                    }
                }
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
                Some(mixer_sets(stored, &h.grade, n))
            }
            _ => None,
        };
        match ready {
            Some(sets) if sets.is_empty() => self.effects_page.graded = None,
            Some(sets) => {
                let (field, target) = named(of);
                let commands = (sets.into_iter())
                    .map(|mut set| {
                        set.insert(field.clone(), target.clone());
                        set.insert("node".into(), json!(n));
                        command("grade.set", Value::Object(set))
                    })
                    .collect();
                self.send_all(commands, "Key Mixer");
            }
            None => {}
        }
    }

    fn window_body(
        &mut self,
        ui: &mut Ui,
        grade: &Grade,
        (of, n): (&GradeOf, u32),
    ) -> Vec<WindowAct> {
        let node = grade.node(n);
        let windows = node.map_or(&[][..], |d| &d.windows[..]);
        let locked = node.is_some_and(|d| d.locked);
        let current = self
            .effects_page
            .window
            .min(windows.len().saturating_sub(1));
        let size = self.project().map_or([0.0; 2], Project::frame_size);
        let usual = (windows.get(current)).map(|w| GradeWindow::new(w.shape.kind(), size));
        let shown = (windows, current, usual.as_ref());
        let mut editing = (self.effects_page.window_name.take())
            .filter(|e| e.of == *of && e.node == n && e.index < windows.len() && !locked)
            .map(|e| (e.index, e.text));
        let copied = self.snap.window_clipboard;
        let acts = grade_windows::window_palette(ui, shown, (&mut editing, locked), copied);
        self.effects_page.window_name = editing.map(|(index, text)| WindowName {
            of: of.clone(),
            node: n,
            index,
            text,
        });
        acts
    }

    fn window_act(
        &mut self,
        of: &GradeOf,
        grade: &Grade,
        (n, frame, at): (u32, i64, i64),
        act: WindowAct,
    ) {
        let windows = grade.node(n).map_or(&[][..], |d| &d.windows[..]);
        let current = self
            .effects_page
            .window
            .min(windows.len().saturating_sub(1));
        let (field, target) = window_target(of);
        let send = |this: &mut Self, method: &str, mut params: Map<String, Value>| {
            this.effects_page.graded = None;
            params.insert(field.clone(), target.clone());
            params.insert("node".into(), json!(n));
            this.send(method, Value::Object(params));
        };
        let set = |i: usize, name: &str, v: Value| {
            Map::from_iter([("window".to_string(), json!(i)), (name.to_string(), v)])
        };
        if !matches!(act, WindowAct::Pick(_) | WindowAct::Rename(..)) {
            self.effects_page.window_name = None;
        }
        match act {
            WindowAct::Add(kind) => {
                self.effects_page.window = windows.len();
                let params = Map::from_iter([("shape".to_string(), json!(kind))]);
                send(self, "grade.window.add", params);
            }
            WindowAct::Delete => match windows.get(current) {
                Some(_) if current < WindowKind::ALL.len() => {
                    self.say("This window can't be deleted. Turn it off instead.");
                }
                Some(w) => {
                    self.effects_page.window = grade_windows::first_of(w.shape.kind());
                    let params = Map::from_iter([("window".to_string(), json!(current))]);
                    send(self, "grade.window.remove", params);
                }
                None => {}
            },
            WindowAct::Pick(i) => self.effects_page.window = i,
            WindowAct::Toggle(i) => {
                self.effects_page.window = i;
                if let Some(w) = windows.get(i) {
                    send(self, "grade.window.set", set(i, "on", json!(!w.on)));
                }
            }
            WindowAct::Invert(i, v) => send(self, "grade.window.set", set(i, "invert", json!(v))),
            WindowAct::Mask(i, v) => send(self, "grade.window.set", set(i, "mask", json!(v))),
            WindowAct::Rename(i, name) => {
                send(self, "grade.window.set", set(i, "name", json!(name)));
            }
            WindowAct::ResetWindow => {
                let size = self.project().map_or([0.0; 2], Project::frame_size);
                if let Some(w) = windows.get(current) {
                    let usual = grade_windows::reset(w, size);
                    let mut params = grade_windows::window_changes(w, &usual);
                    if !params.is_empty() {
                        params.insert("window".into(), json!(current));
                        send(self, "grade.window.set", params);
                    }
                }
            }
            WindowAct::Copy => {
                let params = Map::from_iter([
                    (field.clone(), target.clone()),
                    ("node".to_string(), json!(n)),
                    ("window".to_string(), json!(current)),
                ]);
                self.send("grade.window.copy", Value::Object(params));
            }
            WindowAct::Paste(append) => {
                let copied = self.snap.window_clipboard;
                let onto = windows.get(current).map(|w| w.shape.kind());
                if append || onto != copied {
                    self.effects_page.window = windows.len();
                }
                let params = match append {
                    true => Map::from_iter([("append".to_string(), json!(true))]),
                    false => Map::from_iter([("window".to_string(), json!(current))]),
                };
                send(self, "grade.window.paste", params);
            }
            WindowAct::Convert => {
                self.effects_page.window = match current < WindowKind::ALL.len() {
                    true => windows.len(),
                    false => windows.len() - 1,
                };
                let params = Map::from_iter([("window".to_string(), json!(current))]);
                send(self, "grade.window.convert", params);
            }
            WindowAct::Value(i, f, v) => {
                let mut next = grade.clone();
                let at_window = next.node_mut(n).and_then(|d| d.windows.get_mut(i));
                if let Some(w) = at_window {
                    *w = grade_windows::with_value(w, f, v);
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
        }
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
        ui.separator();
        ui.add_enabled_ui(page, |ui| {
            ui.menu_button("Nodes", |ui| self.nodes_menu(ui));
        });
        if !page {
            super::dialog::why(ui, "open the Color page");
        }
    }

    fn nodes_menu(&mut self, ui: &mut Ui) {
        let shown = self.grade_shown().map(|(of, _, g)| (of, g));
        if let Some((of, g)) = &shown {
            self.settle_grade(of, g);
        }
        let g = shown.as_ref().map(|(_, g)| g);
        let n = g.and_then(|g| self.current_grade_node(g));
        let current = g.zip(n).and_then(|(g, n)| g.node(n));
        let corrector = current.filter(|d| !d.tool.is_mixer());
        let can = |item: NodesItem| match item {
            NodesItem::Key(Action::PreviousNode | Action::NextNode) => {
                g.is_some_and(|g| correctors(g).next().is_some())
            }
            NodesItem::Key(Action::AppendNode) => g.is_some(),
            NodesItem::Key(Action::AddSerialNode | Action::AddNodeBefore) => {
                current.is_some_and(|d| !d.takes_keys())
            }
            NodesItem::Key(Action::AddParallelNode | Action::AddLayerNode) => corrector.is_some(),
            NodesItem::Act(NodeAct::Add(_)) => corrector.is_some(),
            NodesItem::Key(Action::SwitchAllNodes) => g.is_some_and(|g| !g.nodes.is_empty()),
            NodesItem::Key(Action::ExtractNode) => {
                corrector.is_some_and(|d| d.inputs.iter().any(Option::is_some))
            }
            NodesItem::Act(NodeAct::Label) => corrector.is_some(),
            NodesItem::Key(_) | NodesItem::Act(_) => current.is_some(),
        };
        let mut picked = None;
        for (i, group) in NODES_MENU.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            for &(text, item) in *group {
                let key = match item {
                    NodesItem::Key(a) => self.snap.bindings.first(a).unwrap_or_default(),
                    NodesItem::Act(_) => String::new(),
                };
                let button = egui::Button::new(text).shortcut_text(key);
                if ui.add_enabled(can(item), button).clicked() {
                    picked = Some(item);
                    ui.close();
                }
            }
        }
        match (picked, shown, n) {
            (Some(NodesItem::Key(a)), _, _) => self.grade_keys_struck(&[a]),
            (Some(NodesItem::Act(act)), Some((of, g)), Some(n)) => {
                self.grade_node_act(&of, &g, (act, n));
            }
            _ => {}
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
        self.grade_keys_struck(&mine);
    }

    fn grade_keys_struck(&mut self, mine: &[Action]) {
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
        let mut left = g.clone();
        let mut now: Vec<Option<u32>> = (0..=g.last()).map(Some).collect();
        let mut commands = Vec::new();
        for n in order {
            let Some(at) = now.get(n as usize).copied().flatten() else {
                continue;
            };
            let Ok(out) = taken_out(&mut left, at) else {
                continue;
            };
            commands.push(command(
                "grade.remove",
                params(of, "clip", json!({ "node": at })),
            ));
            for k in &mut now {
                *k = k.filter(|c| !out.contains(c)).map(|c| {
                    let below = out.iter().filter(|o| **o < c).count();
                    c - below as u32
                });
            }
        }
        let last = last_corrector(&left, |_| true);
        self.effects_page.grade_nodes = last.into_iter().collect();
        self.effects_page.graded = None;
        self.send_all(commands, "Delete Node");
    }

    fn add_grade_node(&mut self, add: GradeAdd) {
        let Some((of, _, g)) = self.grade_shown() else {
            return;
        };
        self.settle_grade(&of, &g);
        let node = self.current_grade_node(&g);
        let d = node.and_then(|n| g.node(n));
        let mixer = d.is_some_and(|d| d.tool.is_mixer());
        if mixer && !matches!(add, GradeAdd::Serial | GradeAdd::Before | GradeAdd::Append) {
            return;
        }
        if d.is_some_and(GradeNode::takes_keys) && add != GradeAdd::Append {
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
            NodeAct::AddInput => {
                let mut more = json!({ "node": n });
                if d.takes_keys() {
                    more["key"] = json!(true);
                }
                ("grade.connect", params(of, "clip", more))
            }
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
            FieldAct::Corrector | FieldAct::KeyMixer => {
                let at = self.effects_page.grade_spot.unwrap_or_default();
                let add = match act {
                    FieldAct::KeyMixer => GradeAdd::KeyMixer,
                    _ => GradeAdd::Corrector,
                };
                self.select_coming(g.last() + 1);
                ("grade.add", json!({ "add": add, "place": at }))
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
        let mut thumbnails = self.snap.show_thumbnails;
        let mut held = self.snap.disable_thumbnail_refresh;
        let mut color_to = None;
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
                    if ui.checkbox(&mut thumbnails, "Show Thumbnails").clicked() {
                        color_to = Some(json!({ "show_thumbnails": thumbnails }));
                        ui.close();
                    }
                    let hold =
                        egui::Checkbox::new(&mut held, "Disable Thumbnail Refresh During Playback");
                    if ui.add_enabled(thumbnails, hold).clicked() {
                        color_to = Some(json!({ "disable_thumbnail_refresh": held }));
                        ui.close();
                    }
                });
            });
        });
        if let Some(value) = switch_to {
            let more = json!({ "color": { "switching_clips": value } });
            self.send("preferences.set", more);
        }
        if let Some(color) = color_to {
            self.send("preferences.set", json!({ "color": color }));
        }
        if self.snap.show_thumbnails {
            let zoom = match listed {
                true => 1.0,
                false => self.effects_page.grade_view.zoom().max(1.0),
            };
            let pixels = SHOT_HEIGHT * zoom * ui.ctx().pixels_per_point();
            let playing = self.snap.timeline.playing;
            self.shots_want = Some(ShotsWanted {
                of: match &of {
                    GradeOf::Clip(id) => ShotsOf::Clip(id.clone()),
                    GradeOf::Timeline => ShotsOf::Timeline,
                },
                height: (pixels.ceil() as u32).next_multiple_of(SHOT_STEP),
                refresh: !(self.snap.disable_thumbnail_refresh && playing),
            });
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
            tiles: tiles(
                &stored,
                self.snap.show_thumbnails.then_some(&self.node_shots),
            ),
            wires: wires(&stored),
            selected: &selected,
            active: CURRENT,
        };
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
            node_menu(ui, d, &self.snap.bindings, &mut act);
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
            commands.extend(connect(&stored, &of, (to, Some(slot)), None));
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
        if let Some(Port::Node(n)) = done.locked {
            node_act = Some((NodeAct::Lock, n));
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
