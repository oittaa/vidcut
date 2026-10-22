use std::ops::RangeInclusive;
use std::time::Instant;

use eframe::egui::{self, Align, Layout, RichText, vec2};
use egui_phosphor::regular::{
    ARROW_COUNTER_CLOCKWISE, FLIP_HORIZONTAL, FLIP_VERTICAL, LINK_SIMPLE, LINK_SIMPLE_BREAK,
};
use serde_json::{Value, json};
use vidcut_core::model::{
    Clip, Crop, DynamicZoom, Ease, Picture, PictureTool, Project, TrackKind, ZoomBox,
};
use vidcut_core::ops::picture::has_picture;
use vidcut_core::place::upright_fit;

use super::dialog;
use super::placing::{changed, ease_label, target};
use super::theme;
use super::timeline::tools::landing;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

const REACH: f64 = 100.0;
const ZOOM: (f64, f64) = (0.01, 100.0);
const BOX: (f64, f64) = (0.01, 100.0);
const TURNS: f64 = 3600.0;
const LEFT_SHOWING: f64 = 1.0;
const SAME: f64 = 1e-9;
const FIELD_WIDTH: f32 = 76.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Field {
    Zoom,
    ZoomX,
    ZoomY,
    X,
    Y,
    Rotation,
    Left,
    Right,
    Top,
    Bottom,
    StartZoom,
    StartX,
    StartY,
    EndZoom,
    EndX,
    EndY,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Shape {
    frame: [f64; 2],
    fitted: [f64; 2],
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Zoom => "Zoom",
            Field::ZoomX => "Zoom X",
            Field::ZoomY => "Zoom Y",
            Field::X => "Position X",
            Field::Y => "Position Y",
            Field::Rotation => "Rotation",
            Field::Left => "Left",
            Field::Right => "Right",
            Field::Top => "Top",
            Field::Bottom => "Bottom",
            Field::StartZoom => "Start Zoom",
            Field::StartX => "Start X",
            Field::StartY => "Start Y",
            Field::EndZoom => "End Zoom",
            Field::EndX => "End X",
            Field::EndY => "End Y",
        }
    }

    fn starts(self) -> Option<bool> {
        match self {
            Field::StartZoom | Field::StartX | Field::StartY => Some(true),
            Field::EndZoom | Field::EndX | Field::EndY => Some(false),
            _ => None,
        }
    }

    fn zoom_box(self, z: &DynamicZoom) -> ZoomBox {
        if self.starts() == Some(false) {
            z.end
        } else {
            z.start
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY | Field::StartZoom | Field::EndZoom => "%",
            Field::Rotation => "°",
            _ => "",
        }
    }

    fn speed(self) -> f64 {
        match self {
            Field::Zoom
            | Field::ZoomX
            | Field::ZoomY
            | Field::Rotation
            | Field::StartZoom
            | Field::EndZoom => 0.5,
            _ => 1.0,
        }
    }

    fn usual(self) -> f64 {
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY | Field::EndZoom => 100.0,
            Field::StartZoom => 100.0 / DynamicZoom::default().start.size,
            _ => 0.0,
        }
    }

    fn read(self, p: &Picture, s: &Shape) -> f64 {
        let ([x, y], [zx, zy], c) = (p.place(), p.scale(), p.crop);
        let ([w, h], [fw, fh]) = (s.frame, s.fitted);
        let b = self.zoom_box(&p.dynamic_zoom.unwrap_or_default());
        match self {
            Field::Zoom | Field::ZoomX => zx * 100.0,
            Field::ZoomY => zy * 100.0,
            Field::X => (x - 0.5) * w,
            Field::Y => (0.5 - y) * h,
            Field::Rotation => p.turn(),
            Field::Left => c.left * fw,
            Field::Right => c.right * fw,
            Field::Top => c.top * fh,
            Field::Bottom => c.bottom * fh,
            Field::StartZoom | Field::EndZoom => 100.0 / b.size,
            Field::StartX | Field::EndX => (0.5 - b.center[0]) / b.size * fw,
            Field::StartY | Field::EndY => (b.center[1] - 0.5) / b.size * fh,
        }
    }

    fn is_usual(self, p: &Picture) -> bool {
        let ([x, y], [zx, zy], c) = (p.place(), p.scale(), p.crop);
        let usual = self.zoom_box(&DynamicZoom::default());
        let b = p.dynamic_zoom.map_or(usual, |z| self.zoom_box(&z));
        match self {
            Field::Zoom => zx == 1.0 && zy == 1.0,
            Field::ZoomX => zx == 1.0,
            Field::ZoomY => zy == 1.0,
            Field::X => x == 0.5,
            Field::Y => y == 0.5,
            Field::Rotation => p.turn() == 0.0,
            Field::Left => c.left == 0.0,
            Field::Right => c.right == 0.0,
            Field::Top => c.top == 0.0,
            Field::Bottom => c.bottom == 0.0,
            Field::StartZoom | Field::EndZoom => b.size == usual.size,
            Field::StartX | Field::EndX => b.center[0] == usual.center[0],
            Field::StartY | Field::EndY => b.center[1] == usual.center[1],
        }
    }

    fn range(self, p: &Picture, s: &Shape) -> RangeInclusive<f64> {
        let ([w, h], [fw, fh], c) = (s.frame, s.fitted, p.crop);
        let side = |other: f64, span: f64| 0.0..=((1.0 - other) * span - LEFT_SHOWING).max(0.0);
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY => ZOOM.0 * 100.0..=ZOOM.1 * 100.0,
            Field::X => (-REACH - 0.5) * w..=(REACH - 0.5) * w,
            Field::Y => (0.5 - REACH) * h..=(0.5 + REACH) * h,
            Field::Rotation => -TURNS..=TURNS,
            Field::Left => side(c.right, fw),
            Field::Right => side(c.left, fw),
            Field::Top => side(c.bottom, fh),
            Field::Bottom => side(c.top, fh),
            Field::StartZoom | Field::EndZoom => 100.0 / BOX.1..=100.0 / BOX.0,
            Field::StartX | Field::EndX => -REACH * fw / BOX.0..=REACH * fw / BOX.0,
            Field::StartY | Field::EndY => -REACH * fh / BOX.0..=REACH * fh / BOX.0,
        }
    }

    fn put(self, p: &Picture, s: &Shape, v: f64) -> Picture {
        let r = self.range(p, s);
        let v = v.clamp(*r.start(), *r.end());
        if (self.read(p, s) - v).abs() < SAME {
            return p.clone();
        }
        let ([x, y], [zx, zy]) = (p.place(), p.scale());
        let ([w, h], [fw, fh]) = (s.frame, s.fitted);
        let zoom = |v: f64| (v / 100.0).clamp(ZOOM.0, ZOOM.1);
        let at = |v: f64| v.clamp(-REACH, REACH);
        let mut out = p.clone();
        match self {
            Field::Zoom => out.zoom = Some([zoom(v); 2]),
            Field::ZoomX => out.zoom = Some([zoom(v), zy]),
            Field::ZoomY => out.zoom = Some([zx, zoom(v)]),
            Field::X => out.position = Some([at(0.5 + v / w), y]),
            Field::Y => out.position = Some([x, at(0.5 - v / h)]),
            Field::Rotation => out.rotation = Some(v),
            Field::Left => out.crop.left = v / fw,
            Field::Right => out.crop.right = v / fw,
            Field::Top => out.crop.top = v / fh,
            Field::Bottom => out.crop.bottom = v / fh,
            Field::StartZoom
            | Field::StartX
            | Field::StartY
            | Field::EndZoom
            | Field::EndX
            | Field::EndY => {
                let mut z = p.dynamic_zoom.unwrap_or_default();
                let b = if self.starts() == Some(true) {
                    &mut z.start
                } else {
                    &mut z.end
                };
                match self {
                    Field::StartZoom | Field::EndZoom => {
                        let size = (100.0 / v).clamp(BOX.0, BOX.1);
                        let keep = |c: f64| at(0.5 - (0.5 - c) / b.size * size);
                        b.center = [keep(b.center[0]), keep(b.center[1])];
                        b.size = size;
                    }
                    Field::StartX | Field::EndX => b.center[0] = at(0.5 - v / fw * b.size),
                    _ => b.center[1] = at(0.5 + v / fh * b.size),
                }
                out.dynamic_zoom = Some(z);
            }
        }
        out
    }
}

fn typed(text: &str) -> Option<f64> {
    let t: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '%' | '°'))
        .map(|c| if c == '−' { '-' } else { c })
        .collect();
    t.parse().ok()
}

fn shape(p: &Project, c: &Clip) -> Shape {
    let (w, h) = (p.settings.width.max(1), p.settings.height.max(1));
    let frame = [f64::from(w), f64::from(h)];
    let fitted = p
        .media(&c.media)
        .and_then(|m| m.info.video.as_ref())
        .filter(|v| v.width > 0 && v.height > 0)
        .map_or(frame, |v| {
            let sar = (v.sar[0], v.sar[1]);
            let (fw, fh) = upright_fit((v.width, v.height), sar, v.rotation, (w, h));
            [f64::from(fw.max(1)), f64::from(fh.max(1))]
        });
    Shape { frame, fitted }
}

fn name(p: &Project, c: &Clip) -> String {
    match &c.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => p
            .media(&c.media)
            .map_or_else(|| c.id.clone(), |m| m.name.clone()),
    }
}

fn cleared(p: &Picture, tool: PictureTool) -> Picture {
    match tool {
        PictureTool::Transform => Picture {
            crop: p.crop,
            dynamic_zoom: p.dynamic_zoom,
            ..Picture::default()
        },
        PictureTool::Crop => Picture {
            crop: Crop::default(),
            ..p.clone()
        },
        PictureTool::DynamicZoom => Picture {
            dynamic_zoom: None,
            ..p.clone()
        },
    }
}

fn uses(p: &Picture, tool: PictureTool) -> bool {
    cleared(p, tool) != *p
}

fn merged(params: &[Value]) -> Option<Value> {
    let rest = |v: &Value| {
        let mut v = v.clone();
        v.as_object_mut()?.remove("clips");
        Some(v)
    };
    let first = rest(params.first()?)?;
    params
        .iter()
        .all(|p| rest(p).as_ref() == Some(&first))
        .then(|| {
            let mut one = first.clone();
            one["clips"] = json!(
                params
                    .iter()
                    .map(|p| p["clips"].clone())
                    .collect::<Vec<_>>()
            );
            one
        })
}

#[derive(Debug, Clone)]
struct Chosen {
    id: String,
    stored: Picture,
    shown: Picture,
    shape: Shape,
}

#[derive(Debug, Clone)]
struct Held {
    field: Field,
    value: f64,
    from: f64,
    start: Vec<Chosen>,
    dragging: bool,
}

#[derive(Debug, Clone)]
struct Landing {
    since: Instant,
    pictures: Vec<(String, Picture, Picture)>,
}

#[derive(Debug, Default)]
pub struct Inspector {
    unlinked: bool,
    held: Option<Held>,
    landing: Option<Landing>,
    seen: Option<(Vec<String>, Vec<String>)>,
    focus: Option<egui::Id>,
    moved: bool,
    commit: bool,
}

enum Act {
    Hold(Field, f64),
    Drag(Field, f64),
    Land(Field),
    Put(Field, f64),
    Edit(Field, f64),
    Release(Field),
    Link(bool),
    Flips([Option<bool>; 2]),
    Zooming(bool),
    Boxes,
    Ease(Ease),
    Swap,
    Clear(PictureTool),
}

fn label(ui: &mut egui::Ui, text: &str, set: bool) {
    let c = theme::colors();
    ui.label(RichText::new(text).color(if set { c.accent } else { c.text_dim }));
}

fn reset(ui: &mut egui::Ui, shown: bool) -> bool {
    let r = ui.add_visible(
        shown,
        egui::Button::new(ARROW_COUNTER_CLOCKWISE).frame(false),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, shown, "Reset"));
    r.clicked()
}

fn number(
    ui: &mut egui::Ui,
    (f, link): (Field, Option<bool>),
    i: &Inspector,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let first = &chosen[0];
    let set = chosen.iter().any(|c| !f.is_usual(&c.shown));
    label(ui, f.label(), set);
    let held = i.held.as_ref().filter(|h| h.field == f);
    let mut v = held.map_or_else(|| f.read(&first.shown, &first.shape), |h| h.value);
    let before = v;
    ui.horizontal(|ui| {
        let r = ui
            .push_id(f, |ui| {
                ui.add(
                    egui::DragValue::new(&mut v)
                        .update_while_editing(false)
                        .clamp_existing_to_range(false)
                        .range(f.range(&first.shown, &first.shape))
                        .speed(f.speed())
                        .max_decimals(2)
                        .suffix(f.suffix())
                        .custom_parser(typed)
                        .min_size(vec2(FIELD_WIDTH, ui.spacing().interact_size.y)),
                )
            })
            .inner;
        if r.drag_started() {
            acts.push(Act::Hold(f, before));
        }
        if r.dragged() && r.changed() {
            acts.push(Act::Drag(f, v));
        }
        if r.drag_stopped() || (held.is_some_and(|h| h.dragging) && !r.dragged()) {
            acts.push(Act::Land(f));
        } else if r.changed() && !r.dragged() {
            acts.push(Act::Put(f, v));
        } else if r.has_focus() {
            acts.push(Act::Edit(f, v));
        } else if held.is_some() && !r.dragged() {
            acts.push(Act::Release(f));
        }
        if let Some(lit) = link {
            let icon = if lit { LINK_SIMPLE } else { LINK_SIMPLE_BREAK };
            let b = ui.add(egui::Button::selectable(lit, icon));
            b.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, lit, "Link Zoom")
            });
            if b.clicked() {
                acts.push(Act::Link(!lit));
            }
        }
    });
    if reset(ui, set) {
        acts.push(Act::Put(f, f.usual()));
    }
    ui.end_row();
}

fn section(
    ui: &mut egui::Ui,
    (name, tool): (&str, PictureTool),
    set: bool,
    acts: &mut Vec<Act>,
    rows: impl FnOnce(&mut egui::Ui, &mut Vec<Act>),
) {
    let id = ui.make_persistent_id(("inspector", name));
    let (_, header, _) =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.label(RichText::new(name).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled(set, egui::Button::new("Reset")).clicked()
                })
                .inner
            })
            .body(|ui| {
                egui::Grid::new(("inspector-rows", name))
                    .num_columns(3)
                    .min_col_width(84.0)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| rows(ui, acts));
            });
    if header.inner {
        acts.push(Act::Clear(tool));
    }
}

fn transform(ui: &mut egui::Ui, i: &Inspector, chosen: &[Chosen], acts: &mut Vec<Act>) {
    let first = &chosen[0];
    let set = chosen
        .iter()
        .any(|c| uses(&c.shown, PictureTool::Transform));
    section(
        ui,
        ("Transform", PictureTool::Transform),
        set,
        acts,
        |ui, acts| {
            let [zx, zy] = first.shown.scale();
            if !i.unlinked && zx == zy {
                number(ui, (Field::Zoom, Some(true)), i, chosen, acts);
            } else {
                number(ui, (Field::ZoomX, Some(false)), i, chosen, acts);
                number(ui, (Field::ZoomY, None), i, chosen, acts);
            }
            number(ui, (Field::X, None), i, chosen, acts);
            number(ui, (Field::Y, None), i, chosen, acts);
            number(ui, (Field::Rotation, None), i, chosen, acts);
            let flipped = chosen
                .iter()
                .any(|c| c.shown.flip_horizontal || c.shown.flip_vertical);
            label(ui, "Flip", flipped);
            ui.horizontal(|ui| {
                let p = &first.shown;
                for (k, icon, text, on) in [
                    (0, FLIP_HORIZONTAL, "Horizontal", p.flip_horizontal),
                    (1, FLIP_VERTICAL, "Vertical", p.flip_vertical),
                ] {
                    let b = egui::Button::selectable(on, format!("{icon} {text}"));
                    if ui.add(b).clicked() {
                        let mut to = [None; 2];
                        to[k] = Some(!on);
                        acts.push(Act::Flips(to));
                    }
                }
            });
            if reset(ui, flipped) {
                acts.push(Act::Flips([Some(false); 2]));
            }
            ui.end_row();
        },
    );
}

fn crop(ui: &mut egui::Ui, i: &Inspector, chosen: &[Chosen], acts: &mut Vec<Act>) {
    let set = chosen.iter().any(|c| uses(&c.shown, PictureTool::Crop));
    section(ui, ("Crop", PictureTool::Crop), set, acts, |ui, acts| {
        for f in [Field::Left, Field::Right, Field::Top, Field::Bottom] {
            number(ui, (f, None), i, chosen, acts);
        }
    });
}

fn dynamic_zoom(
    ui: &mut egui::Ui,
    i: &Inspector,
    chosen: &[Chosen],
    boxes: bool,
    acts: &mut Vec<Act>,
) {
    let zooms: Vec<DynamicZoom> = chosen.iter().filter_map(|c| c.shown.dynamic_zoom).collect();
    let on = !zooms.is_empty();
    let first = chosen[0].shown.dynamic_zoom;
    let section_name = ("Dynamic Zoom", PictureTool::DynamicZoom);
    section(ui, section_name, on, acts, |ui, acts| {
        label(ui, "Dynamic Zoom", on);
        dialog::switch(ui, |ui| {
            if ui
                .add(egui::Button::selectable(first.is_none(), "Off"))
                .clicked()
                && on
            {
                acts.push(Act::Zooming(false));
            }
            if ui
                .add(egui::Button::selectable(first.is_some(), "On"))
                .clicked()
                && zooms.len() < chosen.len()
            {
                acts.push(Act::Zooming(true));
            }
        });
        if reset(ui, on) {
            acts.push(Act::Zooming(false));
        }
        ui.end_row();
        for f in [
            Field::StartZoom,
            Field::StartX,
            Field::StartY,
            Field::EndZoom,
            Field::EndX,
            Field::EndY,
        ] {
            number(ui, (f, None), i, chosen, acts);
        }
        let eased = zooms.iter().any(|z| z.ease != Ease::Linear);
        label(ui, "Ease", eased);
        ui.add_enabled_ui(first.is_some(), |ui| {
            let now = first.map_or(Ease::Linear, |z| z.ease);
            egui::ComboBox::from_id_salt("inspector-ease")
                .selected_text(ease_label(now))
                .show_ui(ui, |ui| {
                    for e in Ease::ALL {
                        if ui.selectable_label(e == now, ease_label(e)).clicked() {
                            acts.push(Act::Ease(e));
                        }
                    }
                });
        });
        if reset(ui, eased) {
            acts.push(Act::Ease(Ease::Linear));
        }
        ui.end_row();
        label(ui, "", false);
        ui.horizontal(|ui| {
            let show = egui::Button::selectable(boxes, "Show Boxes");
            if ui.add(show).clicked() {
                acts.push(Act::Boxes);
            }
            if ui.add_enabled(on, egui::Button::new("Swap")).clicked() {
                acts.push(Act::Swap);
            }
        });
        ui.end_row();
    });
}

impl Workspace {
    fn chosen(&self) -> Vec<Chosen> {
        let Some(p) = self.project_shared() else {
            return Vec::new();
        };
        let wanted = self.picture_wanted();
        self.snap
            .selection
            .clips
            .iter()
            .filter_map(|id| {
                let (ti, ci) = p.find_clip(id)?;
                let c = &p.tracks[ti].clips[ci];
                (p.tracks[ti].kind == TrackKind::Video && has_picture(c)).then(|| {
                    let stored = c.picture.clone().unwrap_or_default();
                    let shown = wanted
                        .iter()
                        .find(|(w, _)| *w == c.id)
                        .map_or_else(|| stored.clone(), |(_, x)| x.clone());
                    Chosen {
                        id: c.id.clone(),
                        stored,
                        shown,
                        shape: shape(&p, c),
                    }
                })
            })
            .collect()
    }

    fn inspector_landed(&self) -> Vec<(String, Picture)> {
        let (Some(l), Some(p)) = (&self.inspector.landing, self.project()) else {
            return Vec::new();
        };
        l.pictures
            .iter()
            .filter_map(|(id, from, to)| {
                let stored = p.clip(id)?.picture.clone().unwrap_or_default();
                landing(to, l.since.elapsed(), from, &stored).map(|t| (id.clone(), t.clone()))
            })
            .collect()
    }

    pub(super) fn inspector_wanted(&self) -> Vec<(String, Picture)> {
        match self.inspector.held.as_ref().filter(|h| h.dragging) {
            Some(h) => h
                .start
                .iter()
                .map(|c| (c.id.clone(), h.field.put(&c.shown, &c.shape, h.value)))
                .collect(),
            None => self.inspector_landed(),
        }
    }

    pub(super) fn settle_inspector(&mut self) {
        if self.inspector.landing.is_some() && self.inspector_landed().is_empty() {
            self.inspector.landing = None;
        }
    }

    pub(super) fn leave_inspector(&mut self) {
        self.inspector.held = None;
        self.inspector.seen = None;
        self.close_forms();
    }

    pub(super) fn close_forms(&mut self) {
        self.close_title_form();
        self.close_cue_form();
        self.close_track_form();
    }

    fn send_pictures(&mut self, chosen: &[Chosen], to: impl Fn(&Chosen) -> Option<Picture>) {
        let list: Vec<(String, Picture, Picture)> = chosen
            .iter()
            .filter_map(|c| {
                let p = to(c).filter(|p| *p != c.shown)?;
                Some((c.id.clone(), c.stored.clone(), p))
            })
            .collect();
        let params: Vec<Value> = list
            .iter()
            .filter_map(|(id, from, to)| changed(id, from, to))
            .collect();
        if params.is_empty() {
            return;
        }
        self.end_picture_wheel();
        match merged(&params) {
            Some(one) => self.send("picture.set", one),
            None => {
                let commands: Vec<Value> = params
                    .into_iter()
                    .map(|p| json!({ "method": "picture.set", "params": p }))
                    .collect();
                self.send("apply", json!({ "commands": commands }));
            }
        }
        self.inspector.landing = Some(Landing {
            since: Instant::now(),
            pictures: list,
        });
    }

    fn clear_pictures(&mut self, chosen: &[Chosen], tool: PictureTool) {
        let used: Vec<&Chosen> = chosen.iter().filter(|c| uses(&c.shown, tool)).collect();
        if used.is_empty() {
            return;
        }
        let ids: Vec<&str> = used.iter().map(|c| c.id.as_str()).collect();
        self.end_picture_wheel();
        self.timeline.picture_grab.cancel_landing();
        self.send("picture.clear", json!({ "clips": ids, "tool": tool }));
        self.inspector.landing = Some(Landing {
            since: Instant::now(),
            pictures: used
                .iter()
                .map(|c| (c.id.clone(), c.stored.clone(), cleared(&c.shown, tool)))
                .collect(),
        });
    }

    fn show_boxes(&mut self, chosen: &[Chosen]) {
        self.timeline.picture_grab.tool = Some(PictureTool::DynamicZoom);
        let Some(p) = self.project_shared() else {
            return;
        };
        let at = self.snap.player(PlayerId::Timeline).position;
        let ids: Vec<String> = chosen.iter().map(|c| c.id.clone()).collect();
        if target(&p, &ids, at).is_none()
            && let Some(c) = chosen.first().and_then(|c| p.clip(&c.id))
        {
            self.seek(PlayerId::Timeline, c.start, false);
        }
    }

    fn inspect(&mut self, act: Act, chosen: &[Chosen]) {
        let held_on = |i: &Inspector, f: Field| i.held.as_ref().is_some_and(|h| h.field == f);
        match act {
            Act::Hold(field, value) => {
                self.inspector.held = Some(Held {
                    field,
                    value,
                    from: value,
                    start: chosen.to_vec(),
                    dragging: true,
                });
            }
            Act::Drag(field, value) => {
                if let Some(h) = self.inspector.held.as_mut().filter(|h| h.field == field) {
                    h.value = value;
                }
            }
            Act::Land(field) => {
                let Some(h) = self
                    .inspector
                    .held
                    .take_if(|h| h.field == field && h.dragging)
                else {
                    return;
                };
                if h.value != h.from {
                    self.send_pictures(&h.start, |c| Some(field.put(&c.shown, &c.shape, h.value)));
                }
            }
            Act::Put(field, value) => {
                if held_on(&self.inspector, field) {
                    self.inspector.held = None;
                }
                self.send_pictures(chosen, |c| Some(field.put(&c.shown, &c.shape, value)));
            }
            Act::Edit(field, value) => {
                if !held_on(&self.inspector, field) {
                    self.inspector.held = Some(Held {
                        field,
                        value,
                        from: value,
                        start: Vec::new(),
                        dragging: false,
                    });
                }
            }
            Act::Release(field) => {
                if self
                    .inspector
                    .held
                    .as_ref()
                    .is_some_and(|h| h.field == field && !h.dragging)
                {
                    self.inspector.held = None;
                }
            }
            Act::Link(on) => {
                self.inspector.unlinked = !on;
                if on {
                    self.send_pictures(chosen, |c| {
                        let [zx, zy] = c.shown.scale();
                        (zx != zy).then(|| Picture {
                            zoom: Some([zx, zx]),
                            ..c.shown.clone()
                        })
                    });
                }
            }
            Act::Flips([across, down]) => self.send_pictures(chosen, |c| {
                Some(Picture {
                    flip_horizontal: across.unwrap_or(c.shown.flip_horizontal),
                    flip_vertical: down.unwrap_or(c.shown.flip_vertical),
                    ..c.shown.clone()
                })
            }),
            Act::Zooming(on) => {
                self.send_pictures(chosen, |c| {
                    let z = c.shown.dynamic_zoom;
                    Some(Picture {
                        dynamic_zoom: if on {
                            z.or(Some(DynamicZoom::default()))
                        } else {
                            None
                        },
                        ..c.shown.clone()
                    })
                });
                if on {
                    self.show_boxes(chosen);
                }
            }
            Act::Boxes => {
                if self.timeline.picture_grab.tool == Some(PictureTool::DynamicZoom) {
                    self.timeline.picture_grab.tool = None;
                } else {
                    self.show_boxes(chosen);
                }
            }
            Act::Ease(ease) => self.send_pictures(chosen, |c| {
                let z = c.shown.dynamic_zoom?;
                Some(Picture {
                    dynamic_zoom: Some(DynamicZoom { ease, ..z }),
                    ..c.shown.clone()
                })
            }),
            Act::Swap => self.send_pictures(chosen, |c| {
                let z = c.shown.dynamic_zoom?;
                Some(Picture {
                    dynamic_zoom: Some(z.swapped()),
                    ..c.shown.clone()
                })
            }),
            Act::Clear(tool) => self.clear_pictures(chosen, tool),
        }
    }

    pub(super) fn inspector_saw_selection(&mut self) {
        let s = &self.snap.selection;
        self.inspector.seen = Some((s.clips.clone(), s.cues.clone()));
    }

    fn follow_selection(&mut self) {
        let s = &self.snap.selection;
        let now = (s.clips.clone(), s.cues.clone());
        if self.inspector.seen.as_ref() == Some(&now) {
            return;
        }
        let title = self.project_shared().and_then(|p| {
            now.0
                .iter()
                .find(|id| p.clip(id).is_some_and(|c| c.title.is_some()))
                .cloned()
        });
        if let Some(id) = title {
            self.inspect_title(&id, false);
        } else if let Some(id) = now.1.first().cloned() {
            self.inspect_cue(&id, false);
        } else {
            self.close_forms();
        }
        self.inspector.seen = Some(now);
    }

    pub(super) fn inspector_commits(&self) -> bool {
        self.inspector.commit
    }

    pub(super) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let focus = ui.ctx().memory(|m| m.focused());
        let i = &mut self.inspector;
        i.moved |= focus != i.focus;
        i.focus = focus;
        i.commit = (focus.is_none() || i.moved) && !ui.input(|i| i.pointer.any_down());
        i.moved &= !i.commit;
        self.follow_selection();
        let chosen = self.chosen();
        let mut acts = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let forms = self.title_form(ui) | self.cue_form(ui) | self.track_form(ui);
                let Some(p) = self.project_shared() else {
                    return;
                };
                let Some(first) = chosen.first().and_then(|c| p.clip(&c.id)) else {
                    self.inspector.held = None;
                    if !forms {
                        ui.label(dim("Select a clip with a picture"));
                    }
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name(&p, first)).strong());
                    if chosen.len() > 1 {
                        ui.label(dim(format!("{} clips selected", chosen.len())));
                    }
                });
                let boxes = self.timeline.picture_grab.tool == Some(PictureTool::DynamicZoom);
                let i = &self.inspector;
                transform(ui, i, &chosen, &mut acts);
                crop(ui, i, &chosen, &mut acts);
                dynamic_zoom(ui, i, &chosen, boxes, &mut acts);
            });
        for act in acts {
            self.inspect(act, &chosen);
        }
        self.sync_picture_preview();
    }
}
