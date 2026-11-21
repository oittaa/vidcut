use egui::{CursorIcon, DragValue, Label, Response, RichText, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as icon;
use serde_json::{Map, Value, json};
use vidcut_core::model::{GradeWindow, WindowKind, WindowShape};

use super::theme;

const ADDS: [(WindowKind, &str); 4] = [
    (WindowKind::Linear, "Linear"),
    (WindowKind::Circle, "Circle"),
    (WindowKind::Polygon, "Polygon"),
    (WindowKind::Gradient, "Gradient"),
];

const SHAPE_BUTTON: Vec2 = Vec2::new(30.0, 22.0);

const NAME_WIDTH: f32 = 150.0;

const LABEL_WIDTH: f32 = 54.0;

const DIVIDER: f32 = 6.0;

const UNUSED: f64 = 50.0;

const STEP: f64 = 0.1;

const ROTATE_STEP: f64 = 0.5;

pub(super) const PERCENT: (f64, f64) = (0.0, 100.0);

pub(super) const TURN: (f64, f64) = (-180.0, 180.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Field {
    Size,
    Aspect,
    Pan,
    Tilt,
    Rotate,
    Opacity,
    Soft(usize),
    Inside,
    Outside,
}

const TRANSFORM: [(Field, &str); 6] = [
    (Field::Size, "Size"),
    (Field::Aspect, "Aspect"),
    (Field::Pan, "Pan"),
    (Field::Tilt, "Tilt"),
    (Field::Rotate, "Rotate"),
    (Field::Opacity, "Opacity"),
];

const SOFTNESS: [(Field, &str); 6] = [
    (Field::Soft(0), "Soft 1"),
    (Field::Soft(1), "Soft 2"),
    (Field::Soft(2), "Soft 3"),
    (Field::Soft(3), "Soft 4"),
    (Field::Inside, "Inside"),
    (Field::Outside, "Outside"),
];

#[derive(Debug, Clone, PartialEq)]
pub(super) enum WindowAct {
    Add(WindowKind),
    Delete,
    Pick(usize),
    Toggle(usize),
    Invert(usize, bool),
    Mask(usize, bool),
    Rename(usize, String),
    ResetWindow,
    Value(usize, Field, f64),
}

pub(super) fn first_of(kind: WindowKind) -> usize {
    WindowKind::ALL
        .iter()
        .position(|k| *k == kind)
        .unwrap_or_default()
}

fn glyph(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Linear => icon::SQUARE,
        WindowKind::Circle => icon::CIRCLE,
        WindowKind::Polygon => icon::POLYGON,
        WindowKind::Gradient => icon::GRADIENT,
    }
}

fn value(w: &GradeWindow, f: Field) -> Option<f64> {
    match (f, &w.shape) {
        (Field::Size, WindowShape::Gradient { .. })
        | (Field::Aspect, WindowShape::Gradient { .. } | WindowShape::Polygon { .. }) => None,
        (Field::Size, _) => Some(w.size),
        (Field::Aspect, _) => Some(w.aspect),
        (Field::Pan, _) => Some(w.pan),
        (Field::Tilt, _) => Some(w.tilt),
        (Field::Rotate, _) => Some(w.rotate),
        (Field::Opacity, _) => Some(w.opacity),
        (Field::Soft(i), WindowShape::Linear { soft, .. }) => soft.get(i).copied(),
        (Field::Soft(0), WindowShape::Circle { soft } | WindowShape::Gradient { soft }) => {
            Some(*soft)
        }
        (Field::Inside, WindowShape::Polygon { inside, .. }) => Some(*inside),
        (Field::Outside, WindowShape::Polygon { outside, .. }) => Some(*outside),
        _ => None,
    }
}

pub(super) fn with_value(w: &GradeWindow, f: Field, v: f64) -> GradeWindow {
    let mut w = w.clone();
    match f {
        Field::Size => w.size = v,
        Field::Aspect => w.aspect = v,
        Field::Pan => w.pan = v,
        Field::Tilt => w.tilt = v,
        Field::Rotate => w.rotate = v,
        Field::Opacity => w.opacity = v,
        Field::Soft(_) | Field::Inside | Field::Outside => match (f, &mut w.shape) {
            (Field::Soft(i), WindowShape::Linear { soft, .. }) if i < soft.len() => soft[i] = v,
            (Field::Soft(_), WindowShape::Circle { soft } | WindowShape::Gradient { soft }) => {
                *soft = v;
            }
            (Field::Inside, WindowShape::Polygon { inside, .. }) => *inside = v,
            (Field::Outside, WindowShape::Polygon { outside, .. }) => *outside = v,
            _ => {}
        },
    }
    w
}

pub(super) fn reset(w: &GradeWindow, frame: [f64; 2]) -> GradeWindow {
    GradeWindow {
        on: w.on,
        name: w.name.clone(),
        mask: w.mask,
        ..GradeWindow::new(w.shape.kind(), frame)
    }
}

pub(super) fn window_changes(from: &GradeWindow, to: &GradeWindow) -> Map<String, Value> {
    let mut m = Map::new();
    for (name, a, b) in [
        ("size", from.size, to.size),
        ("aspect", from.aspect, to.aspect),
        ("pan", from.pan, to.pan),
        ("tilt", from.tilt, to.tilt),
        ("rotate", from.rotate, to.rotate),
        ("opacity", from.opacity, to.opacity),
    ] {
        if a != b {
            m.insert(name.into(), json!(b));
        }
    }
    if from.invert != to.invert {
        m.insert("invert".into(), json!(to.invert));
    }
    match (&from.shape, &to.shape) {
        (
            WindowShape::Linear {
                soft: a,
                points: ap,
            },
            WindowShape::Linear {
                soft: b,
                points: bp,
            },
        ) => {
            if a != b {
                m.insert("soft".into(), json!(b));
            }
            if ap != bp {
                m.insert("points".into(), json!(bp));
            }
        }
        (WindowShape::Circle { soft: a }, WindowShape::Circle { soft: b })
        | (WindowShape::Gradient { soft: a }, WindowShape::Gradient { soft: b })
            if a != b =>
        {
            m.insert("soft".into(), json!([b]));
        }
        (
            WindowShape::Polygon {
                points: ap,
                inside: ai,
                outside: ao,
            },
            WindowShape::Polygon {
                points: bp,
                inside: bi,
                outside: bo,
            },
        ) => {
            if ai != bi {
                m.insert("inside".into(), json!(bi));
            }
            if ao != bo {
                m.insert("outside".into(), json!(bo));
            }
            if ap != bp {
                m.insert("points".into(), json!(bp));
            }
        }
        _ => {}
    }
    m
}

pub(super) fn window_palette(
    ui: &mut Ui,
    (windows, current, usual): (&[GradeWindow], usize, Option<&GradeWindow>),
    editing: &mut Option<(usize, String)>,
) -> Vec<WindowAct> {
    let mut acts = Vec::new();
    ui.horizontal(|ui| {
        for (kind, text) in ADDS {
            if ui.button(format!("{} {text}", icon::PLUS)).clicked() {
                acts.push(WindowAct::Add(kind));
            }
        }
        ui.separator();
        if ui.button("Delete").clicked() {
            acts.push(WindowAct::Delete);
        }
        ui.menu_button(icon::DOTS_THREE, |ui| {
            if ui.button("Reset Selected Window").clicked() {
                acts.push(WindowAct::ResetWindow);
                ui.close();
            }
        });
    });
    let shown_id = ui.scope_id().with("window-shown");
    let reveal = ui.data(|d| d.get_temp::<usize>(shown_id)) != Some(current);
    ui.data_mut(|d| d.insert_temp(shown_id, current));
    ui.horizontal_top(|ui| {
        let room = ui.available_height();
        let list = ui.vertical(|ui| {
            egui::ScrollArea::vertical()
                .id_salt("window-list")
                .max_height(room)
                .show(ui, |ui| {
                    for (i, w) in windows.iter().enumerate() {
                        let now = i == current;
                        row(ui, (i, w, now, now && reveal), editing, &mut acts);
                    }
                });
        });
        let list = list.response.rect;
        ui.add_space(DIVIDER);
        let Some(w) = windows.get(current) else {
            return;
        };
        let shown = ui.vertical(|ui| fields(ui, (current, w, usual), &mut acts));
        let shown = shown.response.rect;
        let x = (list.right() + shown.left()) / 2.0;
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let bottom = list.bottom().max(shown.bottom());
        ui.painter().vline(x, list.top()..=bottom, stroke);
    });
    acts
}

fn row(
    ui: &mut Ui,
    (i, w, current, reveal): (usize, &GradeWindow, bool, bool),
    editing: &mut Option<(usize, String)>,
    acts: &mut Vec<WindowAct>,
) {
    let fill = match current {
        true => ui.visuals().widgets.hovered.weak_bg_fill,
        false => ui.visuals().extreme_bg_color,
    };
    let framed = egui::Frame::new()
        .fill(fill)
        .inner_margin(Vec2::new(6.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let shape = ui.add(
                    egui::Button::new(RichText::new(glyph(w.shape.kind())).size(15.0))
                        .min_size(SHAPE_BUTTON)
                        .stroke(match w.on {
                            true => Stroke::new(1.5, theme::WINDOW_ON),
                            false => Stroke::NONE,
                        }),
                );
                if shape.clicked() {
                    acts.push(WindowAct::Toggle(i));
                }
                name_field(ui, (i, &w.name), editing, acts);
                ui.add_enabled_ui(w.on, |ui| {
                    let mut invert = w.invert;
                    if ui.toggle_value(&mut invert, "Invert").changed() {
                        acts.push(WindowAct::Invert(i, invert));
                    }
                    let mut mask = w.mask;
                    if ui.toggle_value(&mut mask, "Mask").changed() {
                        acts.push(WindowAct::Mask(i, mask));
                    }
                });
            });
        });
    if reveal {
        framed.response.scroll_to_me(None);
    }
}

fn name_field(
    ui: &mut Ui,
    (i, name): (usize, &str),
    editing: &mut Option<(usize, String)>,
    acts: &mut Vec<WindowAct>,
) {
    let size = Vec2::new(NAME_WIDTH, ui.spacing().interact_size.y);
    if let Some((at, text)) = editing.as_mut().filter(|(at, _)| *at == i) {
        let r = ui.add_sized(size, egui::TextEdit::singleline(text));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() {
            if ui.input(|k| k.key_pressed(egui::Key::Enter)) && *text != name {
                acts.push(WindowAct::Rename(*at, text.clone()));
            }
            *editing = None;
        }
        return;
    }
    let r = ui.add_sized(size, Label::new(name).truncate().sense(Sense::click()));
    if r.double_clicked() {
        *editing = Some((i, name.to_string()));
    } else if r.clicked() {
        acts.push(WindowAct::Pick(i));
    }
}

fn fields(
    ui: &mut Ui,
    (i, w, usual): (usize, &GradeWindow, Option<&GradeWindow>),
    acts: &mut Vec<WindowAct>,
) {
    for (title, list) in [("Transform", TRANSFORM), ("Softness", SOFTNESS)] {
        ui.strong(title);
        egui::Grid::new(("window-fields", title))
            .num_columns(4)
            .show(ui, |ui| {
                for pair in list.chunks(2) {
                    for &(f, text) in pair {
                        field(ui, (i, w, usual), (f, text), acts);
                    }
                    ui.end_row();
                }
            });
    }
}

fn field(
    ui: &mut Ui,
    (i, w, usual): (usize, &GradeWindow, Option<&GradeWindow>),
    (f, text): (Field, &str),
    acts: &mut Vec<WindowAct>,
) {
    let (step, range) = match f {
        Field::Rotate => (ROTATE_STEP, TURN),
        _ => (STEP, PERCENT),
    };
    let shown = value(w, f);
    let live = w.on && shown.is_some();
    let mut v = shown.unwrap_or(UNUSED);
    let n = ui.add_enabled(live, label(text));
    let n = n.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let dx = n.drag_delta().x;
    let mut changed = false;
    if let Some(u) = usual
        .and_then(|u| value(u, f))
        .filter(|_| n.double_clicked())
    {
        v = u;
        changed = true;
    } else if n.dragged() && dx != 0.0 {
        v = (v + f64::from(dx) * step).clamp(range.0, range.1);
        changed = true;
    }
    let r = ui.add_enabled(
        live,
        DragValue::new(&mut v)
            .speed(step)
            .range(range.0..=range.1)
            .clamp_existing_to_range(false)
            .fixed_decimals(2)
            .update_while_editing(false),
    );
    if live && (changed || r.changed()) {
        acts.push(WindowAct::Value(i, f, v));
    }
}

fn label(text: &str) -> impl egui::Widget + '_ {
    move |ui: &mut Ui| -> Response {
        let size = Vec2::new(LABEL_WIDTH, ui.spacing().interact_size.y);
        ui.add_sized(size, Label::new(text).sense(Sense::click_and_drag()))
    }
}
