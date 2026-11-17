use std::time::{Duration, Instant};

use egui::{Color32, CursorIcon, DragValue, Label, Rect, Response, Sense, Stroke, Ui, Vec2};
use egui_phosphor::regular as icon;
use serde_json::{Map, Value, json};
use vidcut_core::model::{Grade, Picture};
use vidcut_core::ops::grade::{checked, printed};
use vidcut_core::ops::picture::takes_effects;
use vidcut_core::request::{Page, PrinterColor};

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

const PRINTED: [(PrinterColor, &str); 3] = [
    (PrinterColor::Red, "Red"),
    (PrinterColor::Green, "Green"),
    (PrinterColor::Blue, "Blue"),
];

pub(super) struct Graded {
    clip: String,
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

fn changed_grade(g: &Grade, field: &str, value: Value) -> Grade {
    let mut v = serde_json::to_value(g).unwrap_or_default();
    v[field] = value;
    serde_json::from_value(v)
        .ok()
        .filter(|n| checked(n).is_ok())
        .unwrap_or_else(|| g.clone())
}

fn differences(from: &Grade, to: &Grade) -> Map<String, Value> {
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

const BAR: Vec2 = Vec2::new(16.0, 110.0);

fn usual(field: &str) -> f64 {
    serde_json::to_value(Grade::default())
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

fn primaries(g: &Grade) -> [Wheel; 4] {
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

fn logs(g: &Grade) -> [Wheel; 4] {
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
        let c = self.project()?.clip(&h.clip)?;
        let mut picture = c.picture.clone().unwrap_or_default();
        picture.grade = checked(&h.grade).ok()?;
        Some((h.clip.clone(), picture))
    }

    pub(super) fn primaries_panel(&mut self, ui: &mut Ui) {
        self.palette(ui);
        self.sync_picture_preview();
    }

    pub(super) fn color_clip(&self) -> Option<String> {
        let p = self.project()?;
        super::placing::topmost_by(p, self.snap.timeline.position, takes_effects)
    }

    fn palette(&mut self, ui: &mut Ui) {
        let Some(id) = self.color_clip() else {
            self.effects_page.graded = None;
            ui.label("No Clip at the Playhead");
            return;
        };
        let Some(clip) = self.project().and_then(|p| p.clip(&id)) else {
            self.effects_page.graded = None;
            return;
        };
        let name = clip.name.clone().unwrap_or_else(|| id.clone());
        let stored = clip
            .picture
            .as_ref()
            .and_then(|p| p.grade.clone())
            .unwrap_or_default();
        if let Some(h) = &self.effects_page.graded
            && (h.clip != id
                || h.sent
                    .is_some_and(|t| h.grade == stored || t.elapsed() > LANDING))
        {
            self.effects_page.graded = None;
        }
        let g = match &self.effects_page.graded {
            Some(h) => h.grade.clone(),
            None => stored.clone(),
        };
        let key = egui::Id::unique("color-wheels-mode");
        let mut mode = ui.data(|d| d.get_temp::<Mode>(key)).unwrap_or(Mode::Wheels);
        let mut changes: Vec<Change> = Vec::new();
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.strong(name);
            ui.separator();
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
        });
        ui.data_mut(|d| d.insert_temp(key, mode));
        let wheels = match mode {
            Mode::Log => logs(&g),
            Mode::Wheels | Mode::Bars => primaries(&g),
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
        if let Some(point) = point {
            let lights = crate::preferences::load()
                .map(|p| p.color)
                .unwrap_or_default();
            changes.push(Change {
                field: "offset",
                value: json!(printed(g.offset, point, &lights)),
            });
        }
        let c = &mut changes;
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
        if reset {
            self.effects_page.graded = None;
            self.send("grade.clear", json!({ "clips": id }));
            return;
        }
        if !changes.is_empty() {
            let grade = changes
                .into_iter()
                .fold(g, |g, c| changed_grade(&g, c.field, c.value));
            self.effects_page.graded = Some(Graded {
                clip: id.clone(),
                grade,
                sent: None,
            });
        }
        let down = ui.input(|i| i.pointer.any_down());
        let ready = match &mut self.effects_page.graded {
            Some(h) if h.sent.is_none() && !down => {
                h.sent = Some(Instant::now());
                Some(differences(&stored, &h.grade))
            }
            _ => None,
        };
        match ready {
            Some(params) if params.is_empty() => self.effects_page.graded = None,
            Some(mut params) => {
                params.insert("clips".into(), json!(id));
                self.send("grade.set", Value::Object(params));
            }
            None => {}
        }
    }
}
