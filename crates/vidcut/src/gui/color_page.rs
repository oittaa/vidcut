use std::time::{Duration, Instant};

use egui::{Color32, CursorIcon, DragValue, Label, Rect, Response, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as icon;
use serde_json::{Map, Value, json};
use vidcut_av::screen::{ShotsOf, ShotsWanted};
use vidcut_core::model::{
    BlendMode, Corrector, Grade, GradeLink, GradeNode, GradeTool, GradeWindow, KeyInput, KeyMix,
    KeyOutput, Picture, Project, Qualifier, Settings, SharedNode, WindowKind,
};
use vidcut_core::ops::grade::{GradeOf, checked, corrector_checked, printed, taken_out};
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

const AUTO_BADGE: &str = "auto_color";

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

const ADDS: [(Action, GradeAdd); 7] = [
    (Action::AddSerialNode, GradeAdd::Serial),
    (Action::AddNodeBefore, GradeAdd::Before),
    (Action::AddParallelNode, GradeAdd::Parallel),
    (Action::AddLayerNode, GradeAdd::Layer),
    (Action::AppendNode, GradeAdd::Append),
    (Action::AddOutsideNode, GradeAdd::Outside),
    (Action::AddSplitterCombiner, GradeAdd::SplitterCombiner),
];

const WINDOW_ADDS: [(Action, WindowKind); 4] = [
    (Action::AddCircleNode, WindowKind::Circle),
    (Action::AddLinearNode, WindowKind::Linear),
    (Action::AddPolygonNode, WindowKind::Polygon),
    (Action::AddCurveNode, WindowKind::Curve),
];

const KEY_PORT: usize = 1;

const SPLIT_NAMES: [&str; 3] = ["Red", "Green", "Blue"];

fn outlet_slot(output: u8) -> usize {
    match output {
        0 => 0,
        j => KEY_PORT + usize::from(j),
    }
}

fn output_of(slot: usize) -> u8 {
    slot.checked_sub(KEY_PORT)
        .filter(|j| *j > 0)
        .and_then(|j| u8::try_from(j).ok())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
enum NodesItem {
    Key(Action),
    Act(NodeAct),
}

const NODES_MENU: [&[(&str, NodesItem)]; 6] = [
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
        (
            "Add Splitter Combiner Node",
            NodesItem::Key(Action::AddSplitterCombiner),
        ),
    ],
    &[
        (
            "Add Serial Node + CPW",
            NodesItem::Key(Action::AddCircleNode),
        ),
        (
            "Add Serial Node + LPW",
            NodesItem::Key(Action::AddLinearNode),
        ),
        (
            "Add Serial Node + PPW",
            NodesItem::Key(Action::AddPolygonNode),
        ),
        (
            "Add Serial Node + PCW",
            NodesItem::Key(Action::AddCurveNode),
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
    RemoveInput,
    Morph(GradeMixer),
    Mode(BlendMode),
    Share,
    Unshare,
    AddShared(u64),
    DeleteShared(u64),
}

#[derive(Debug, Clone, Copy)]
enum FieldAct {
    Corrector,
    KeyMixer,
    Splitter,
    Combiner,
    ResetAll,
    ResetLayers,
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

fn key_input_held(g: &Grade, n: u32, frame: i64) -> Option<KeyInput> {
    g.node(n)?.key_input_changed_at(frame)
}

fn qualifier_held(g: &Grade, n: u32, frame: i64) -> Option<Qualifier> {
    g.node(n)?.qualifier_changed_at(frame)
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
    let auto = |k: &String| k == AUTO_BADGE;
    let primary = |k: &String| !log(k) && !auto(k) && !NO_BADGE.contains(&k.as_str());
    [
        (
            changed.iter().any(primary),
            icon::SLIDERS_HORIZONTAL,
            "Primaries",
        ),
        (changed.iter().any(log), icon::CIRCLE_HALF, "Log"),
        (changed.iter().any(auto), icon::MAGIC_WAND, "Auto Color"),
        (
            std::iter::once(&d.qualifier)
                .chain(d.keys.iter().map(|k| &k.qualifier))
                .any(|q| !q.hsl_usual()),
            icon::EYEDROPPER,
            "Qualifier",
        ),
        (d.is_animated(), icon::DIAMOND, "Keyframes"),
        (
            std::iter::once((&d.key, &d.key_input, &d.qualifier))
                .chain(d.keys.iter().map(|k| (&k.key, &k.key_input, &k.qualifier)))
                .any(|(k, i, q)| !k.is_usual() || !i.is_usual() || !q.row_usual()),
            icon::SQUARE_HALF,
            "Key",
        ),
        (d.shared.is_some(), icon::SHARE_NETWORK, "Shared Node"),
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

fn qualifier_key(ui: &mut Ui, changes: &mut Vec<Change>, q: Qualifier) {
    ui.horizontal(|ui| {
        ui.strong("Qualifier");
        let mut invert = q.key_invert;
        if ui.toggle_value(&mut invert, "Invert").changed() {
            changes.push(Change {
                field: "qualifier_key_invert",
                value: json!(invert),
            });
        }
        let mut matte = !q.mask;
        if ui.toggle_value(&mut matte, "Matte").changed() {
            changes.push(Change {
                field: "qualifier_mask",
                value: json!(!matte),
            });
        }
        let gain = ("Gain", "qualifier_gain");
        slider(ui, changes, gain, q.gain, 0.005, (0.0, 2.0));
        let offset = ("Offset", "qualifier_offset");
        slider(ui, changes, offset, q.offset, 0.005, (-1.0, 1.0));
    });
}

fn qualifier_part(
    ui: &mut Ui,
    changes: &mut Vec<Change>,
    (text, field): (&str, &'static str),
    on: bool,
) {
    let mut on = on;
    if ui.checkbox(&mut on, text).changed() {
        changes.push(Change {
            field,
            value: json!(on),
        });
    }
}

fn qualifier_body(ui: &mut Ui, changes: &mut Vec<Change>, q: Qualifier) {
    let percent = (0.0, 100.0);
    let step = 0.25;
    ui.horizontal(|ui| {
        ui.strong("HSL");
        let mut invert = q.invert;
        if ui.toggle_value(&mut invert, "Invert").changed() {
            changes.push(Change {
                field: "qualifier_invert",
                value: json!(invert),
            });
        }
    });
    ui.horizontal(|ui| {
        qualifier_part(ui, changes, ("Hue", "qualifier_hue"), q.hue);
        let rows = [
            ("Center", "qualifier_hue_center", q.hue_center),
            ("Width", "qualifier_hue_width", q.hue_width),
            ("Soft", "qualifier_hue_soft", q.hue_soft),
            ("Sym", "qualifier_hue_sym", q.hue_sym),
        ];
        for (text, field, v) in rows {
            slider(ui, changes, (text, field), v, step, percent);
        }
    });
    ui.horizontal(|ui| {
        qualifier_part(ui, changes, ("Sat", "qualifier_sat"), q.sat);
        let rows = [
            ("Low", "qualifier_sat_low", q.sat_low),
            ("High", "qualifier_sat_high", q.sat_high),
            ("L.Soft", "qualifier_sat_low_soft", q.sat_low_soft),
            ("H.Soft", "qualifier_sat_high_soft", q.sat_high_soft),
        ];
        for (text, field, v) in rows {
            slider(ui, changes, (text, field), v, step, percent);
        }
    });
    ui.horizontal(|ui| {
        qualifier_part(ui, changes, ("Lum", "qualifier_lum"), q.lum);
        let rows = [
            ("Low", "qualifier_lum_low", q.lum_low),
            ("High", "qualifier_lum_high", q.lum_high),
            ("L.Soft", "qualifier_lum_low_soft", q.lum_low_soft),
            ("H.Soft", "qualifier_lum_high_soft", q.lum_high_soft),
        ];
        for (text, field, v) in rows {
            slider(ui, changes, (text, field), v, step, percent);
        }
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

fn qualified(q: Qualifier, c: &Change) -> Qualifier {
    let Some(field) = c.field.strip_prefix("qualifier_") else {
        return q;
    };
    match (field, c.value.as_f64()) {
        ("hue_width", Some(v)) => return q.with_hue_width(v),
        ("hue_soft", Some(v)) => return q.with_hue_soft(v),
        _ => {}
    }
    let Ok(Value::Object(mut values)) = serde_json::to_value(q) else {
        return q;
    };
    values.insert(field.to_string(), c.value.clone());
    serde_json::from_value(Value::Object(values)).unwrap_or(q)
}

fn qualifier_changes(from: Qualifier, to: Qualifier) -> Map<String, Value> {
    let (Ok(Value::Object(a)), Ok(Value::Object(b))) =
        (serde_json::to_value(from), serde_json::to_value(to))
    else {
        return Map::new();
    };
    b.into_iter()
        .filter(|(k, v)| a.get(k) != Some(v))
        .map(|(k, v)| (format!("qualifier_{k}"), v))
        .collect()
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

fn mixer_sets((from, to): (&Grade, &Grade), n: u32, frame: i64) -> Vec<Map<String, Value>> {
    let (Some(a), Some(b)) = (from.node(n), to.node(n)) else {
        return Vec::new();
    };
    let (was, now) = (a.mix_changed_at(frame), b.mix_changed_at(frame));
    let mut out: Vec<Map<String, Value>> = (was.iter().zip(now).enumerate())
        .filter_map(|(i, (x, y))| {
            let mut set = mix_changes(*x, *y);
            (!set.is_empty()).then(|| {
                set.insert("input".into(), json!(i));
                set
            })
        })
        .collect();
    let keys = (
        a.key_output_changed_at(frame),
        b.key_output_changed_at(frame),
    );
    let keys = keys.0.zip(keys.1).map(|(x, y)| key_changes(x, y));
    out.extend(keys.filter(|k| !k.is_empty()));
    out
}

fn window_sets(
    (from, to): (&Grade, &Grade),
    n: u32,
    frame: i64,
) -> Vec<(usize, Map<String, Value>)> {
    let (Some(a), Some(b)) = (from.node(n), to.node(n)) else {
        return Vec::new();
    };
    let (a, b) = (a.windows_changed_at(frame), b.windows_changed_at(frame));
    if a.len() != b.len() {
        return Vec::new();
    }
    let pairs = a.iter().zip(b).enumerate();
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

fn named(of: &GradeOf) -> Map<String, Value> {
    of.fields("clips")
}

fn window_target(of: &GradeOf) -> Map<String, Value> {
    of.fields("clip")
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
    let mut v = of.fields(field);
    if let Value::Object(from) = more {
        v.extend(from);
    }
    Value::Object(v)
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
        GradeTool::Splitter => "Splitter",
        GradeTool::Combiner => "Combiner",
    }
}

#[derive(Clone, Copy)]
struct ShotPlace {
    start: u32,
    stand_in: Option<u32>,
}

fn shot_of(shots: &NodeShots, n: u32, at: ShotPlace) -> Shot {
    let n = at.stand_in.unwrap_or(at.start + n);
    let cell = shots.cells.iter().find(|(k, ..)| *k == n);
    Shot {
        picture: cell.and_then(|&(_, uv, size)| Some((shots.texture?, uv, size))),
    }
}

fn tiles(g: &Grade, shots: Option<(&NodeShots, ShotPlace)>) -> Vec<Tile<Port>> {
    let input = |slot: usize, name: String| Inlet {
        slot,
        side: Side::Left,
        knot: RGB_KNOT,
        name,
    };
    let key_input = |slot: usize, name: String| Inlet {
        slot,
        side: Side::Left,
        knot: MASK_KNOT,
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
        views: None,
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
                shots.map(|(s, at)| shot_of(s, n, at)),
            ),
            GradeTool::KeyMixer { .. } => (
                kind(&d.tool).to_string(),
                (0..d.inputs.len())
                    .map(|i| key_input(i, format!("Input Link {}", i + 1)))
                    .collect(),
                vec![outlet(KEY_PORT, "Key Output")],
                None,
            ),
            GradeTool::Splitter => (
                kind(&d.tool).to_string(),
                vec![input(0, "RGB Input".into())],
                (0u8..)
                    .zip(SPLIT_NAMES)
                    .map(|(j, name)| outlet(outlet_slot(j), name))
                    .collect(),
                None,
            ),
            GradeTool::Combiner => (
                kind(&d.tool).to_string(),
                (SPLIT_NAMES.iter().enumerate())
                    .map(|(i, name)| input(i, (*name).into()))
                    .collect(),
                vec![outlet(0, "RGB Output")],
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
            views: None,
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
        views: None,
    });
    out
}

fn wires(g: &Grade) -> Vec<Wire<Port>> {
    let dim = Stroke::new(1.5, theme::colors().text_dim);
    let wire = |l: GradeLink, (to, slot): (Port, usize)| Wire {
        from: port(l.node),
        out: if l.key {
            KEY_PORT
        } else {
            outlet_slot(l.output)
        },
        to,
        slot,
        drops: !l.key && inlet(g, to, slot) == Some(false),
        line: match l.key {
            true => Stroke::new(1.5, MASK_KNOT),
            false => dim,
        },
    };
    let mut out = Vec::new();
    for (d, n) in g.nodes.iter().zip(1u32..) {
        for (slot, l) in d.inputs.iter().enumerate() {
            if let Some(l) = l {
                out.push(wire(*l, (Port::Node(n), slot)));
            }
        }
        if let Some(l) = d.key_from {
            out.push(wire(l, (Port::Node(n), KEY_PORT)));
        }
    }
    if let Some(l) = g.output {
        out.push(wire(l, (Port::Output, 0)));
    }
    out
}

fn inlet(g: &Grade, to: Port, slot: usize) -> Option<bool> {
    match to {
        Port::Node(n) => Some(match &g.node(n)?.tool {
            GradeTool::KeyMixer { .. } => true,
            GradeTool::Corrector(_) => slot == KEY_PORT,
            _ => false,
        }),
        Port::Output => Some(false),
        Port::Source => None,
    }
}

fn link_of((from, out): (Port, usize)) -> Option<GradeLink> {
    let key = out == KEY_PORT;
    let output = output_of(out);
    from_number(from).map(|node| GradeLink {
        output,
        ..GradeLink::new(node, key)
    })
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

fn linked_now(g: &Grade, to: Port, slot: usize) -> Option<GradeLink> {
    match to {
        Port::Node(n) => {
            let d = g.node(n)?;
            match (&d.tool, slot) {
                (GradeTool::Corrector(_), KEY_PORT) => d.key_from,
                _ => d.inputs.get(slot).copied().flatten(),
            }
        }
        Port::Output => g.output,
        Port::Source => None,
    }
}

fn connect(
    of: &GradeOf,
    (to, slot): (Port, Option<usize>),
    from: Option<GradeLink>,
) -> Option<Value> {
    let mut more = json!({ "node": to_number(to)? });
    if let Some(s) = slot {
        more["input"] = json!(s);
    }
    if let Some(f) = from {
        more["from"] = json!(f.node);
        if f.key {
            more["key"] = json!(true);
        }
        if f.output > 0 {
            more["output"] = json!(f.output);
        }
    }
    Some(command("grade.connect", params(of, "clip", more)))
}

fn grade_links(g: &Grade, of: &GradeOf, l: &Link<Port>) -> Vec<Value> {
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
            inlet(g, *dst, *s).map(|_| (*dst, Some(*s), (*src, *out)))
        }
        (End::Output(src, out), Spot::Tile(dst)) => match l.pulled {
            Some((pd, ps)) if pd == *dst => Some((*dst, Some(ps), (*src, *out))),
            _ => onto(*dst, *out == KEY_PORT).map(|s| (*dst, s, (*src, *out))),
        },
        (End::Input(dst, s), Spot::Knot(End::Output(src, out))) => {
            inlet(g, *dst, *s).map(|_| (*dst, Some(*s), (*src, *out)))
        }
        (End::Input(dst, s), Spot::Tile(src)) => inlet(g, *dst, *s)
            .filter(|key| gives(g, *src, *key))
            .map(|key| (*dst, Some(*s), (*src, if key { KEY_PORT } else { 0 }))),
        _ => None,
    };
    let new = new.filter(|(d, _, (s, _))| d != s);
    let mut out = Vec::new();
    if let Some((pd, ps)) = l.pulled
        && new.is_none_or(|(d, s, _)| d != pd || s != Some(ps))
    {
        out.extend(connect(of, (pd, Some(ps)), None));
    }
    if let Some((d, s, src)) = new
        && let Some(from) = link_of(src)
        && s.is_none_or(|s| linked_now(g, d, s) != Some(from))
    {
        out.extend(connect(of, (d, s), Some(from)));
    }
    out
}

fn leads(g: &Grade, n: u32) -> bool {
    g.node(n)
        .is_some_and(|d| !matches!(d.tool, GradeTool::Splitter | GradeTool::Combiner))
}

fn grade_pick(mut picked: Vec<u32>, current: Option<u32>) -> (Vec<u32>, bool) {
    let Some(c) = current else {
        return (picked, false);
    };
    let held = picked.contains(&c);
    picked.retain(|n| *n != c);
    let loose = !held && !picked.is_empty();
    picked.push(c);
    (picked, loose)
}

fn shared_name(s: &SharedNode) -> String {
    match s.node.label.trim() {
        "" => format!("Shared Node {}", s.number),
        label => label.to_string(),
    }
}

fn node_menu(
    ui: &mut Ui,
    (d, shared): (&GradeNode, Option<&[SharedNode]>),
    keys: &Bindings,
    act: &mut Option<NodeAct>,
) {
    let key = |a: Action| keys.first(a).unwrap_or_default();
    let mut pick = |ui: &mut Ui, on: bool, button: egui::Button, a: NodeAct| {
        if ui.add_enabled(on, button).clicked() {
            *act = Some(a);
            ui.close();
        }
    };
    let adds: &[(GradeAdd, &str)] = match &d.tool {
        GradeTool::Corrector(_) => &ADD_MENU,
        GradeTool::Splitter => &MIXER_ADD_MENU[1..],
        _ => &MIXER_ADD_MENU,
    };
    let keyed = d.takes_keys();
    if keyed {
        let reset = egui::Button::new("Reset Node Grade");
        pick(ui, true, reset, NodeAct::Reset);
    }
    ui.menu_button("Add Node", |ui| {
        for &(add, text) in adds {
            let action = ADDS.iter().find(|(_, a)| *a == add && !keyed);
            let shown = action.map(|(a, _)| key(*a)).unwrap_or_default();
            let button = egui::Button::new(text).shortcut_text(shown);
            pick(ui, true, button, NodeAct::Add(add));
        }
        let serial = adds.iter().any(|(a, _)| *a == GradeAdd::Serial);
        if let Some(list) = shared.filter(|l| serial && !l.is_empty()) {
            ui.separator();
            for s in list {
                let item = egui::Button::new(shared_name(s));
                pick(ui, true, item, NodeAct::AddShared(s.number));
            }
        }
    });
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
            if let Some(list) = shared {
                shared_menu(ui, (d, list), &mut pick);
            }
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
            let lock = egui::Button::selectable(d.locked, "Lock Node");
            pick(ui, true, lock, NodeAct::Lock);
            pick(ui, true, delete(), NodeAct::Delete);
            let more = egui::Button::new("Add One Input");
            pick(ui, true, more, NodeAct::AddInput);
            let less = egui::Button::new("Remove One Input");
            pick(ui, true, less, NodeAct::RemoveInput);
            return;
        }
        GradeTool::Splitter | GradeTool::Combiner => {
            let lock = egui::Button::selectable(d.locked, "Lock Node");
            pick(ui, true, lock, NodeAct::Lock);
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
    pick(
        ui,
        true,
        egui::Button::new("Remove One Input"),
        NodeAct::RemoveInput,
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

fn shared_menu(
    ui: &mut Ui,
    (d, list): (&GradeNode, &[SharedNode]),
    pick: &mut impl FnMut(&mut Ui, bool, egui::Button, NodeAct),
) {
    if d.shared.is_none() {
        let save = egui::Button::new("Save as Shared Node");
        pick(ui, true, save, NodeAct::Share);
        return;
    }
    let convert = egui::Button::new("Convert to Corrector");
    pick(ui, true, convert, NodeAct::Unshare);
    ui.menu_button("Delete Shared Node", |ui| {
        for s in list {
            let item = egui::Button::new(shared_name(s));
            pick(ui, true, item, NodeAct::DeleteShared(s.number));
        }
    });
}

fn field_menu(ui: &mut Ui, act: &mut Option<FieldAct>, clip: bool) {
    let mut pick = |ui: &mut Ui, text: &str, a: FieldAct| {
        if ui.button(text).clicked() {
            *act = Some(a);
            ui.close();
        }
    };
    pick(ui, "Reset All Grades and Nodes", FieldAct::ResetAll);
    ui.add_enabled_ui(clip, |ui| {
        pick(ui, "Reset All Node Stack Layers", FieldAct::ResetLayers);
    });
    pick(ui, "Reset Grades and Keep Nodes", FieldAct::KeepNodes);
    ui.menu_button("Add Node", |ui| {
        pick(ui, "Corrector", FieldAct::Corrector);
        pick(ui, "Key Mixer", FieldAct::KeyMixer);
        pick(ui, "Splitter", FieldAct::Splitter);
        pick(ui, "Combiner", FieldAct::Combiner);
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

pub(super) struct GradeChoice {
    level: GradeLevel,
    layer: usize,
    names: Vec<String>,
}

pub(super) fn layer_label(name: &str) -> String {
    format!("Clip - {name}")
}

fn layer_menu(ui: &mut Ui, choice: &GradeChoice) -> Option<(GradeLevel, usize)> {
    let mut picked = None;
    ui.menu_button("Switch to Node Layer", |ui| {
        let mut item = |ui: &mut Ui, (on, open): (bool, bool), text: String, to| {
            if ui
                .add_enabled(open, egui::Button::selectable(on, text))
                .clicked()
                && !on
            {
                picked = to;
                ui.close();
            }
        };
        item(ui, (false, false), "Group Pre Clip".into(), None);
        for n in 1..=vidcut_core::ops::grade::NODE_STACK_LAYERS {
            let name =
                (choice.names.get(n - 1).cloned()).unwrap_or_else(|| Settings::layer_name(n));
            let on = choice.level == GradeLevel::Clip && choice.layer == n;
            let open = n <= choice.names.len();
            item(
                ui,
                (on, open),
                layer_label(&name),
                Some((GradeLevel::Clip, n)),
            );
        }
        item(ui, (false, false), "Group Post Clip".into(), None);
        let on = choice.level == GradeLevel::Timeline;
        let to = Some((GradeLevel::Timeline, choice.layer));
        item(ui, (on, true), "Timeline".into(), to);
    });
    picked
}

fn layer_dots(ui: &mut Ui, room: Rect, choice: &GradeChoice) -> Option<(GradeLevel, usize)> {
    let count = choice.names.len() + 1;
    let gap = 14.0;
    let left = room.center().x - gap * (count - 1) as f32 / 2.0;
    let painter = ui.painter_at(room);
    let c = theme::colors();
    let mut picked = None;
    for i in 0..count {
        let at = egui::pos2(left + gap * i as f32, room.bottom() - 10.0);
        let (to, text) = match choice.names.get(i) {
            Some(name) => ((GradeLevel::Clip, i + 1), layer_label(name)),
            None => ((GradeLevel::Timeline, choice.layer), "Timeline".to_string()),
        };
        let on = match to.0 {
            GradeLevel::Clip => choice.level == GradeLevel::Clip && choice.layer == to.1,
            GradeLevel::Timeline => choice.level == GradeLevel::Timeline,
        };
        let spot = Rect::from_center_size(at, Vec2::splat(12.0));
        let r = ui.interact(spot, ui.scope_id().with(("layer-dot", i)), Sense::click());
        let color = if on { Color32::WHITE } else { c.text_dim };
        painter.circle_filled(at, 3.5, color);
        if r.on_hover_text(text).clicked() && !on {
            picked = Some(to);
        }
    }
    picked
}

fn grade_switch(ui: &mut Ui, choice: &GradeChoice) -> Option<(GradeLevel, usize)> {
    let GradeChoice {
        level,
        layer,
        names,
    } = choice;
    let (level, layer) = (*level, *layer);
    let mut picked = None;
    let right = egui::Layout::right_to_left(egui::Align::Center);
    ui.with_layout(right, |ui| {
        if names.len() < 2 {
            super::dialog::switch(ui, |ui| {
                for (l, text) in [
                    (GradeLevel::Clip, "Clip"),
                    (GradeLevel::Timeline, "Timeline"),
                ] {
                    if ui.selectable_label(level == l, text).clicked() && level != l {
                        picked = Some((l, layer));
                    }
                }
            });
            return;
        }
        let shown = match level {
            GradeLevel::Clip => layer_label(&names[layer - 1]),
            GradeLevel::Timeline => "Timeline".to_string(),
        };
        egui::ComboBox::from_id_salt("grade-level")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    let on = level == GradeLevel::Clip && layer == i + 1;
                    if ui.selectable_label(on, layer_label(name)).clicked() && !on {
                        picked = Some((GradeLevel::Clip, i + 1));
                    }
                }
                ui.separator();
                let on = level == GradeLevel::Timeline;
                if ui.selectable_label(on, "Timeline").clicked() && !on {
                    picked = Some((GradeLevel::Timeline, layer));
                }
            });
    });
    picked
}

fn usual(field: &str) -> f64 {
    let parts = [
        ("key_input_", serde_json::to_value(KeyInput::default())),
        ("qualifier_", serde_json::to_value(Qualifier::default())),
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
        let GradeOf::Clip(id, layer) = &h.of else {
            return None;
        };
        let p = self.project()?;
        let c = p.clip(id)?;
        let mut picture = c.picture.as_deref().cloned().unwrap_or_default();
        checked(&h.grade).ok()?;
        let frame = p.frame_size();
        let was = picture.grade.take().unwrap_or_else(|| Grade::usual(frame));
        let grade = was.with_layer(*layer, h.grade.clone(), frame);
        picture.grade = (!grade.is_usual(frame)).then_some(grade);
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

    pub(super) fn set_layer(&mut self, layer: usize) -> Result<(), String> {
        let count = self.layer_names().len();
        if !(1..=count).contains(&layer) {
            return Err(format!("The project has {count} node stack layers"));
        }
        if layer != self.effects_page.layer {
            self.effects_page.graded = None;
            self.effects_page.layer = layer;
        }
        Ok(())
    }

    pub(super) fn layer_names(&self) -> Vec<String> {
        (self.project()).map_or_else(Settings::one_layer, |p| {
            p.settings.node_stack_layers.clone()
        })
    }

    pub(super) fn shown_layer(&self) -> usize {
        self.effects_page.layer.clamp(1, self.layer_names().len())
    }

    fn shot_place(&self, of: &GradeOf) -> ShotPlace {
        let first = ShotPlace {
            start: 0,
            stand_in: None,
        };
        let (GradeOf::Clip(id, layer), Some(p)) = (of, self.project()) else {
            return first;
        };
        let frame = p.frame_size();
        let whole = (p.clip(id))
            .and_then(|c| c.picture.as_ref()?.grade.clone())
            .unwrap_or_else(|| Grade::usual(frame));
        match *layer > 1 && whole.layers.len() + 1 < *layer {
            true => ShotPlace {
                start: 0,
                stand_in: whole.stacked().output.map(|l| l.node).filter(|n| *n > 0),
            },
            false => ShotPlace {
                start: whole.layer_start(*layer),
                stand_in: None,
            },
        }
    }

    pub(super) fn grade_choice(&self) -> GradeChoice {
        GradeChoice {
            level: self.effects_page.grading,
            layer: self.shown_layer(),
            names: self.layer_names(),
        }
    }

    pub(super) fn switch_grade(&mut self, (level, layer): (GradeLevel, usize)) {
        let to = json!({ "page": Page::Color, "grade": level, "layer": layer });
        self.send("gui.page", to);
    }

    pub(super) fn grade_shown(&self) -> Option<(GradeOf, String, Grade)> {
        let p = self.project()?;
        match self.effects_page.grading {
            GradeLevel::Clip => {
                let id = self.color_clip()?;
                let c = p.clip(&id)?;
                let layer = self.shown_layer();
                let frame = p.frame_size();
                let whole = c.picture.as_ref().and_then(|pic| pic.grade.as_ref());
                let stored = whole.map_or_else(|| Grade::usual(frame), |g| g.layer(layer, frame));
                let name = p.clip_name(c).unwrap_or_else(|| id.clone());
                Some((GradeOf::Clip(id, layer), name, stored))
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
            GradeOf::Clip(id, _) => (self.project())
                .and_then(|p| p.clip(id))
                .map_or(at, |c| c.content_at(at)),
            GradeOf::Timeline => at,
        }
    }

    pub(super) fn color_clip(&self) -> Option<String> {
        let p = self.project()?;
        vidcut_core::ops::gallery::color_clip(p, self.snap.timeline.position)
    }

    fn palette(&mut self, ui: &mut Ui) -> bool {
        let level = self.effects_page.grading;
        let choice = self.grade_choice();
        let mut picked = None;
        let Some((of, name, stored)) = self.grade_shown() else {
            self.effects_page.graded = None;
            ui.horizontal(|ui| {
                ui.label(match level {
                    GradeLevel::Clip => "No Clip at the Playhead",
                    GradeLevel::Timeline => "No Timeline",
                });
                picked = grade_switch(ui, &choice);
            });
            if let Some(l) = picked {
                self.switch_grade(l);
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
                picked = grade_switch(ui, &choice);
            });
            if let Some(l) = picked {
                self.switch_grade(l);
            }
            return false;
        };
        let (palette, mode) = (self.effects_page.palette, self.effects_page.primaries);
        let (mut palette_picked, mut mode_picked) = (None, None);
        let mut changes: Vec<Change> = Vec::new();
        let mut reset = false;
        let mut auto = false;
        ui.horizontal(|ui| {
            ui.strong(name);
            ui.label(format!("Node {n:02}"));
            ui.separator();
            for (p, text) in [
                (GradePalette::Primaries, "Primaries"),
                (GradePalette::Qualifier, "Qualifier"),
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
                picked = grade_switch(ui, &choice);
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
            if let GradeOf::Clip(..) = &of {
                auto = ui.button("A").on_hover_text("Auto Color").clicked();
            }
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
            picked = grade_switch(ui, &choice);
        });
        let mut view = Map::new();
        if let Some(p) = palette_picked {
            view.insert("palette".into(), json!(p));
        }
        if let Some(m) = mode_picked {
            view.insert("primaries".into(), json!(m));
        }
        if let Some((l, layer)) = picked {
            view.insert("grade".into(), json!(l));
            view.insert("layer".into(), json!(layer));
        }
        if !view.is_empty() {
            view.insert("page".into(), json!(Page::Color));
            self.send("gui.page", Value::Object(view));
        }
        let key_now = key_output_held(&grade, n, frame).unwrap_or_default();
        let qualifier_now = qualifier_held(&grade, n, frame).unwrap_or_default();
        let mut window_acts = Vec::new();
        let point = match palette {
            GradePalette::Key => {
                let input = key_input_held(&grade, n, frame).unwrap_or_default();
                key_input(ui, &mut changes, input);
                ui.separator();
                key_output(ui, &mut changes, ("Key Output", key_now));
                ui.separator();
                qualifier_key(ui, &mut changes, qualifier_now);
                None
            }
            GradePalette::Qualifier => {
                qualifier_body(ui, &mut changes, qualifier_now);
                None
            }
            GradePalette::Window => {
                window_acts = self.window_body(ui, &grade, (&of, n, frame));
                None
            }
            GradePalette::Primaries => primaries_body(ui, (&g, mode), &mut changes),
        };
        let target = named(&of);
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
            let mut params = target;
            params.extend([node, at, part]);
            self.send("grade.clear", Value::Object(params));
            return palette == GradePalette::Primaries;
        }
        if auto && !locked {
            self.effects_page.graded = None;
            self.auto_color(&of, n);
            return true;
        }
        if !changes.is_empty() {
            let (quals, changes): (Vec<Change>, Vec<Change>) = changes
                .into_iter()
                .partition(|c| c.field.starts_with("qualifier_"));
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
                d.change_key_input_at(frame, |i| inputs.iter().fold(i, key_inputted));
                d.change_qualifier_at(frame, |q| quals.iter().fold(q, qualified));
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
                let inputs = (
                    key_input_held(&stored, n, frame),
                    key_input_held(&h.grade, n, frame),
                );
                if let (Some(a), Some(b)) = inputs {
                    params.extend(key_input_changes(a, b));
                }
                let quals = (
                    qualifier_held(&stored, n, frame),
                    qualifier_held(&h.grade, n, frame),
                );
                if let (Some(a), Some(b)) = quals {
                    params.extend(qualifier_changes(a, b));
                }
                Some((params, window_sets((&stored, &h.grade), n, frame)))
            }
            _ => None,
        };
        match ready {
            Some((params, sets)) if params.is_empty() && sets.is_empty() => {
                self.effects_page.graded = None;
            }
            Some((mut params, sets)) => {
                let clip = window_target(&of);
                for (i, mut set) in sets {
                    set.extend(clip.clone());
                    set.insert("node".into(), json!(n));
                    set.insert("window".into(), json!(i));
                    set.insert("at".into(), json!(at));
                    self.send("grade.window.set", Value::Object(set));
                }
                if !params.is_empty() {
                    params.extend(target);
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
        let choice = GradeChoice {
            level,
            ..self.grade_choice()
        };
        let Some(d) = grade.node(n) else {
            return;
        };
        let mix = d.mix_changed_at(frame);
        let key = d.key_output_changed_at(frame).unwrap_or_default();
        let (mut picked, mut reset) = (None, false);
        ui.horizontal(|ui| {
            ui.strong(name);
            ui.separator();
            ui.label(kind(&d.tool));
            ui.separator();
            reset = ui.button("Reset").clicked();
            picked = grade_switch(ui, &choice);
        });
        if let Some(l) = picked {
            self.switch_grade(l);
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
        key_output(ui, &mut keys, ("Output", key));
        if d.locked {
            return;
        }
        if reset {
            self.effects_page.graded = None;
            let mut params = named(of);
            let node = ("node".to_string(), json!(n));
            let at = ("at".to_string(), json!(self.snap.timeline.position));
            let part = ("palette".to_string(), json!(GradePalette::Key));
            params.extend([node, at, part]);
            self.send("grade.clear", Value::Object(params));
            return;
        }
        if !mixes.is_empty() || !keys.is_empty() {
            let mut next = grade.clone();
            if let Some(d) = next.node_mut(n) {
                d.change_key_output_at(frame, |k| keys.iter().fold(k, keyed));
                for (i, c) in &mixes {
                    d.change_mix_at(frame, *i, |m| mixed(m, c));
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
                Some(mixer_sets((stored, &h.grade), n, frame))
            }
            _ => None,
        };
        match ready {
            Some(sets) if sets.is_empty() => self.effects_page.graded = None,
            Some(sets) => {
                let target = named(of);
                let commands = (sets.into_iter())
                    .map(|mut set| {
                        set.extend(target.clone());
                        set.insert("node".into(), json!(n));
                        set.insert("at".into(), json!(at));
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
        (of, n, frame): (&GradeOf, u32, i64),
    ) -> Vec<WindowAct> {
        let node = grade.node(n);
        let windows = node.map_or(&[][..], |d| d.windows_changed_at(frame));
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
        let windows = grade
            .node(n)
            .map_or(&[][..], |d| d.windows_changed_at(frame));
        let current = self
            .effects_page
            .window
            .min(windows.len().saturating_sub(1));
        let target = window_target(of);
        let send = |this: &mut Self, method: &str, mut params: Map<String, Value>| {
            this.effects_page.graded = None;
            params.extend(target.clone());
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
                let mut params = target.clone();
                params.insert("node".into(), json!(n));
                params.insert("window".into(), json!(current));
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
                if let Some(d) = next.node_mut(n) {
                    d.change_window_at(frame, i, |w| grade_windows::with_value(w, f, v));
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
        let mut params = named(&of);
        params.insert("node".into(), json!(n));
        params.insert("at".into(), json!(self.snap.timeline.position));
        params.insert("color".into(), json!(point.0));
        params.insert("points".into(), json!(point.1));
        self.send("grade.printer", Value::Object(params));
    }

    pub(super) fn color_menu(&mut self, ui: &mut Ui) {
        let page = self.effects_page.page == Page::Color && self.project().is_some();
        let key = self
            .snap
            .bindings
            .first(Action::AutoColor)
            .unwrap_or_default();
        let auto = egui::Button::new("Auto Color").shortcut_text(key);
        if ui.add_enabled(page, auto).clicked() {
            self.grade_keys_struck(&[Action::AutoColor]);
            ui.close();
        }
        let key = (self.snap.bindings.first(Action::GrabStill)).unwrap_or_default();
        let grab = egui::Button::new("Grab Still").shortcut_text(key);
        if ui.add_enabled(page, grab).clicked() {
            self.grab_still();
            ui.close();
        }
        ui.separator();
        for (action, text) in [
            (
                Action::ApplyGradeOneClipPrior,
                "Apply Grade From One Clip Prior",
            ),
            (
                Action::ApplyGradeTwoClipsPrior,
                "Apply Grade From Two Clips Prior",
            ),
        ] {
            let key = self.snap.bindings.first(action).unwrap_or_default();
            let button = egui::Button::new(text).shortcut_text(key);
            if ui.add_enabled(page, button).clicked() {
                self.grade_keys_struck(&[action]);
                ui.close();
            }
        }
        ui.separator();
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
        ui.separator();
        let ripple = self.ripple_params().filter(|_| page);
        let button = egui::Button::new("Ripple Node Changes to Selected Clips");
        if ui.add_enabled(ripple.is_some(), button).clicked() {
            if let Some(params) = ripple {
                self.send("grade.ripple", params);
                self.effects_page.ripple_base = None;
            }
            ui.close();
        }
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
            NodesItem::Key(
                Action::AddSerialNode
                | Action::AddNodeBefore
                | Action::AddCircleNode
                | Action::AddLinearNode
                | Action::AddPolygonNode
                | Action::AddCurveNode,
            ) => current.is_some_and(|d| !d.takes_keys()),
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
        let mut layer_to = None;
        for (i, group) in NODES_MENU.iter().enumerate() {
            if i > 0 {
                ui.separator();
            } else {
                layer_to = layer_menu(ui, &self.grade_choice());
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
        if let Some(to) = layer_to {
            self.switch_grade(to);
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
            page.pick_grade(vec![switched_to(g, switching, kept, was)], false);
            page.grade_seen = Some((open, of.clone(), clip));
            page.grade_coming = None;
            page.grade_view.stop();
            page.grade_spot = None;
            page.grade_keys.forget();
        } else if seen.is_some_and(|(_, _, c)| *c != clip) {
            let n = switched_to(g, switching, was, was);
            if Some(n) != was {
                page.pick_grade(vec![n], false);
            }
            page.grade_seen = Some((open, of.clone(), clip));
        }
        if (page.grade_coming).is_some_and(|(n, t)| g.node(n).is_some() || t.elapsed() > LANDING) {
            page.grade_coming = None;
        }
        let coming = page.grade_coming.map(|(n, _)| n);
        let before = page.grade_nodes.len();
        let last = page.grade_nodes.last().copied();
        page.grade_nodes
            .retain(|n| g.node(*n).is_some() || Some(*n) == coming);
        if page.grade_nodes.last().copied() != last || page.grade_nodes.len() < 2 {
            page.grade_loose = false;
        }
        if page.grade_nodes.is_empty() && before > 0 {
            page.pick_grade(last_corrector(g, |_| true).into_iter().collect(), false);
        }
        self.keep_ripple_base(of, g);
    }

    fn keep_ripple_base(&mut self, of: &GradeOf, g: &Grade) {
        let n = self
            .current_grade_node(g)
            .filter(|n| g.corrector(*n).is_some());
        let key = n
            .filter(|_| matches!(of, GradeOf::Clip(..)))
            .map(|n| (of.clone(), n));
        if self.effects_page.ripple_base.as_ref().map(|(k, _)| k) == key.as_ref() {
            return;
        }
        let base = key.and_then(|(of, n)| {
            let copied = vidcut_core::ops::grade::copy(self.project()?, &of, n).ok()?;
            Some(((of, n), copied))
        });
        self.effects_page.ripple_base = base;
    }

    fn ripple_params(&self) -> Option<Value> {
        let (of, _, g) = self.grade_shown()?;
        let GradeOf::Clip(clip, layer) = &of else {
            return None;
        };
        let n = self.current_grade_node(&g)?;
        let ((at, node), before) = self.effects_page.ripple_base.as_ref()?;
        if *at != of || *node != n {
            return None;
        }
        let clips: Vec<&String> = (self.snap.selection.thumbnails.iter())
            .filter(|c| *c != clip)
            .collect();
        (!clips.is_empty()).then(
            || json!({ "clip": clip, "layer": layer, "node": n, "before": before, "clips": clips }),
        )
    }

    fn select_added(&mut self, g: &Grade, add: GradeAdd, node: Option<u32>) {
        let frame = self.project().map_or([0.0; 2], Project::frame_size);
        let Some(n) = vidcut_core::ops::grade::added_number(g, add, node, frame) else {
            return;
        };
        self.effects_page.pick_grade(vec![n], false);
        self.effects_page.grade_coming = Some((n, Instant::now()));
    }

    pub(super) fn current_grade_node(&self, g: &Grade) -> Option<u32> {
        let n = *self.effects_page.grade_nodes.last()?;
        g.node(n).map(|_| n)
    }

    pub(super) fn pick_grade_nodes(
        &mut self,
        nodes: Vec<u32>,
        current: Option<u32>,
    ) -> Result<(), String> {
        let (of, _, g) = self.grade_shown().ok_or(match self.effects_page.grading {
            GradeLevel::Clip => "There is no clip at the playhead",
            GradeLevel::Timeline => "There is no timeline",
        })?;
        if let Some(missing) = (nodes.iter().chain(&current)).find(|n| g.node(**n).is_none()) {
            return Err(format!("the grade has no node {missing}"));
        }
        let mut kept: Vec<u32> = Vec::new();
        for n in nodes {
            kept.retain(|k| *k != n);
            kept.push(n);
        }
        if let Some(c) = current.filter(|c| !leads(&g, *c)) {
            return Err(format!(
                "node {c}: a splitter or a combiner is never current"
            ));
        }
        self.settle_grade(&of, &g);
        let was = self.current_grade_node(&g);
        let current = (current.or(kept.iter().rev().copied().find(|n| leads(&g, *n)))).or(was);
        let (kept, loose) = grade_pick(kept, current);
        self.effects_page.pick_grade(kept, loose);
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
                self.add_grade_node(add, None);
            }
        }
        for (action, kind) in WINDOW_ADDS {
            if struck(action) {
                self.add_grade_node(GradeAdd::Serial, Some(kind));
            }
        }
        if struck(Action::AddStaticKeyframe) {
            self.add_grade_key(vidcut_core::model::GradeKeyKind::Static);
        }
        if struck(Action::GrabStill) {
            self.grab_still();
        }
        for (action, prior) in [
            (Action::ApplyGradeOneClipPrior, 1),
            (Action::ApplyGradeTwoClipsPrior, 2),
        ] {
            if struck(action) {
                self.send("grade.apply", json!({ "prior": prior }));
            }
        }
        let whole = self.strip_grades();
        if let Some((clip, clips)) = &whole {
            if struck(Action::Copy) {
                self.send("grade.copy", json!({ "clip": clip }));
            }
            if struck(Action::Paste) && self.snap.grade_clipboard {
                self.send("grade.apply", json!({ "clips": clips }));
            }
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
        if let Some(n) = corrector
            && struck(Action::AutoColor)
        {
            self.auto_color(&of, n);
        }
        if let Some(n) = corrector.filter(|_| whole.is_none()) {
            if struck(Action::Copy) {
                self.send("grade.copy", params(&of, "clip", json!({ "node": n })));
            }
            if struck(Action::Paste) && self.snap.node_clipboard {
                self.send("grade.paste", params(&of, "clips", json!({ "node": n })));
            }
        }
        if struck(Action::DeleteNode) && !self.delete_window_points() {
            let picked: Vec<u32> = (self.effects_page.picked_grade().iter())
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

    fn auto_color(&mut self, of: &GradeOf, n: u32) {
        if let GradeOf::Timeline = of {
            return;
        }
        let mut params = window_target(of);
        params.insert("node".into(), json!(n));
        params.insert("at".into(), json!(self.snap.timeline.position));
        self.send("grade.auto_color", Value::Object(params));
    }

    fn switch_grade_nodes(&mut self, of: &GradeOf, g: &Grade, picked: Option<&[u32]>, label: &str) {
        let nodes: Vec<(u32, bool)> = (g.nodes.iter().zip(1u32..))
            .filter(|(_, n)| picked.is_none_or(|p| p.contains(n)))
            .map(|(d, n)| (n, d.on))
            .collect();
        let to = match picked {
            Some(p) => match p.last().and_then(|n| g.node(*n)) {
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
        self.effects_page
            .pick_grade(last.into_iter().collect(), false);
        self.effects_page.graded = None;
        self.send_all(commands, "Delete Node");
    }

    fn add_grade_node(&mut self, add: GradeAdd, window: Option<WindowKind>) {
        let Some((of, _, g)) = self.grade_shown() else {
            return;
        };
        self.settle_grade(&of, &g);
        let node = self.current_grade_node(&g);
        let d = node.and_then(|n| g.node(n));
        let mixer = d.is_some_and(|d| d.tool.is_mixer());
        let serial = matches!(add, GradeAdd::Serial | GradeAdd::SplitterCombiner);
        if mixer && !serial && !matches!(add, GradeAdd::Before | GradeAdd::Append) {
            return;
        }
        if d.is_some_and(GradeNode::takes_keys) && add != GradeAdd::Append {
            return;
        }
        self.grade_add_sent(&of, &g, (add, node), window);
    }

    fn grade_add_sent(
        &mut self,
        of: &GradeOf,
        g: &Grade,
        (add, node): (GradeAdd, Option<u32>),
        window: Option<WindowKind>,
    ) {
        let mut more = match (add, node) {
            (GradeAdd::Append, _) => json!({ "add": add }),
            (_, Some(n)) => json!({ "add": add, "node": n }),
            (_, None) => return,
        };
        if let Some(w) = window {
            more["window"] = json!(w);
        }
        self.select_added(g, add, node);
        self.send("grade.add", params(of, "clip", more));
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
                self.grade_add_sent(of, g, (add, Some(n)), None);
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
            NodeAct::DeleteShared(s) => {
                self.send("grade.shared.delete", json!({ "shared": s }));
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
            NodeAct::Add(_) | NodeAct::Delete | NodeAct::Label | NodeAct::DeleteShared(_) => return,
            NodeAct::Share => ("grade.share", params(of, "clip", json!({ "node": n }))),
            NodeAct::Unshare => ("grade.unshare", params(of, "clip", json!({ "node": n }))),
            NodeAct::AddShared(s) => {
                let more = json!({ "node": n, "shared": s });
                ("grade.shared.add", params(of, "clip", more))
            }
            NodeAct::Reset => {
                let more = json!({ "node": n, "at": self.snap.timeline.position });
                ("grade.clear", params(of, "clips", more))
            }
            NodeAct::Toggle => set(json!({ "on": !d.on })),
            NodeAct::Lock => set(json!({ "locked": !d.locked })),
            NodeAct::Morph(to) => set(json!({ "mixer": to })),
            NodeAct::Mode(mode) => set(json!({ "mode": mode })),
            NodeAct::AddInput => {
                let more = json!({ "node": n });
                ("grade.connect", params(of, "clip", more))
            }
            NodeAct::RemoveInput => {
                let more = json!({ "node": n });
                ("grade.input.remove", params(of, "clip", more))
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
            FieldAct::Corrector | FieldAct::KeyMixer | FieldAct::Splitter | FieldAct::Combiner => {
                let at = self.effects_page.grade_spot.unwrap_or_default();
                let add = match act {
                    FieldAct::KeyMixer => GradeAdd::KeyMixer,
                    FieldAct::Splitter => GradeAdd::Splitter,
                    FieldAct::Combiner => GradeAdd::Combiner,
                    _ => GradeAdd::Corrector,
                };
                if matches!(add, GradeAdd::Corrector | GradeAdd::KeyMixer) {
                    self.select_added(g, add, None);
                }
                ("grade.add", json!({ "add": add, "place": at }))
            }
            FieldAct::KeepNodes => {
                let at = self.snap.timeline.position;
                ("grade.clear", json!({ "keep_nodes": true, "at": at }))
            }
            FieldAct::ResetAll => {
                self.effects_page.pick_grade(vec![1], false);
                ("grade.clear", json!({}))
            }
            FieldAct::ResetLayers => {
                let GradeOf::Clip(id, _) = of else {
                    return;
                };
                self.effects_page.pick_grade(vec![1], false);
                self.effects_page.graded = None;
                let params = json!({ "clips": id, "all_layers": true });
                self.send("grade.clear", params);
                return;
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
                    GradeOf::Clip(id, _) => ShotsOf::Clip(id.clone()),
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
            GradeOf::Clip(id, layer) => format!("grade-node-{id}-{layer}"),
            GradeOf::Timeline => "grade-node-timeline".to_string(),
        };
        let selected: Vec<Port> = (self.effects_page.picked_grade().iter())
            .map(|n| Port::Node(*n))
            .collect();
        let current = self.effects_page.grade_nodes.last().map(|n| Port::Node(*n));
        let graph = Graph {
            salt,
            tiles: tiles(
                &stored,
                (self.snap.show_thumbnails).then(|| (&self.node_shots, self.shot_place(&of))),
            ),
            wires: wires(&stored),
            selected: &selected,
            current: current.as_ref(),
            active: CURRENT,
            viewers: [None, None],
        };
        let mut node_act: Option<(NodeAct, u32)> = None;
        let mut field_act: Option<FieldAct> = None;
        let shared_nodes = (self.project_shared()).filter(|_| matches!(of, GradeOf::Clip(..)));
        let menu = |ui: &mut Ui, p: &Port| {
            let Port::Node(n) = *p else {
                return;
            };
            let Some(d) = stored.node(n) else {
                return;
            };
            let mut act = None;
            let shared = shared_nodes.as_ref().map(|p| p.shared_nodes.as_slice());
            node_menu(ui, (d, shared), &self.snap.bindings, &mut act);
            if let Some(a) = act {
                node_act = Some((a, n));
            }
        };
        let room = ui.available_rect_before_wrap();
        let clip = matches!(of, GradeOf::Clip(..));
        let done = match listed {
            true => node_editor::list(ui, &mut self.effects_page.grade_view, &graph, menu),
            false => node_editor::show(ui, &mut self.effects_page.grade_view, &graph, menu, |ui| {
                field_menu(ui, &mut field_act, clip)
            }),
        };
        if let Some(to) = layer_dots(ui, room, &self.grade_choice()) {
            self.switch_grade(to);
        }
        if let Some((inside, off)) = done.band {
            let inside: Vec<u32> = (inside.into_iter())
                .filter_map(|p| match p {
                    Port::Node(n) => Some(n),
                    _ => None,
                })
                .collect();
            let current = self.effects_page.grade_nodes.last().copied();
            let mut picked = self.effects_page.picked_grade().to_vec();
            match off {
                true => picked.retain(|n| !inside.contains(n) || Some(*n) == current),
                false => picked = inside,
            }
            self.send_grade_pick(picked, current);
        }
        if let Some(at) = done.spot {
            self.effects_page.grade_spot = Some(at);
        }
        let mut commands = done
            .link
            .map(|l| grade_links(&stored, &of, &l))
            .unwrap_or_default();
        if let Some((to, slot)) = done.unlink {
            commands.extend(connect(&of, (to, Some(slot)), None));
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
            self.effects_page.pick_grade(vec![n], false);
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
        let mut current = self.effects_page.grade_nodes.last().copied();
        let mut picked = self.effects_page.picked_grade().to_vec();
        match done.picked {
            Some((Port::Node(n), Pick::Active)) => {
                picked.push(n);
                if leads(&stored, n) {
                    current = Some(n);
                }
            }
            Some((Port::Node(n), Pick::Toggle)) if Some(n) == current && picked.contains(&n) => {
                return;
            }
            Some((Port::Node(n), Pick::Toggle)) if picked.contains(&n) => {
                picked.retain(|k| *k != n);
            }
            Some((Port::Node(n), Pick::Toggle)) => picked.push(n),
            Some((Port::Node(n), Pick::Only)) => {
                picked = vec![n];
                if leads(&stored, n) {
                    current = Some(n);
                }
            }
            None if done.clicked && done.unlink.is_none() => picked = current.into_iter().collect(),
            _ => return,
        }
        self.send_grade_pick(picked, current);
    }

    fn send_grade_pick(&mut self, picked: Vec<u32>, current: Option<u32>) {
        let (nodes, loose) = grade_pick(picked, current);
        let page = &self.effects_page;
        if nodes == page.grade_nodes && loose == page.grade_loose {
            return;
        }
        let p = match nodes.split_last() {
            Some((c, held)) if loose => json!({ "nodes": held, "current": c }),
            _ => json!({ "nodes": nodes }),
        };
        self.send("gui.grade_nodes", p);
    }
}
