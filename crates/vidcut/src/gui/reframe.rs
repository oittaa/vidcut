use eframe::egui::{self, CursorIcon, Pos2, Rect, Stroke, Vec2, vec2};
use serde_json::{Value, json};
use vidcut_core::model::{FindModel, Project, Subject};
use vidcut_core::reframe::{SMOOTH, USUAL_SMOOTH};
use vidcut_core::request::{ReframeBox, ReframeMode};

use super::dialog;
use super::inspector::{label, switched_group};
use super::theme;
use super::workspace::Workspace;

const NAME: &str = "Smart Reframe";
const SUBJECTS: [(Subject, &str); 4] = [
    (Subject::Person, "Person"),
    (Subject::Face, "Face"),
    (Subject::Vehicle, "Vehicle"),
    (Subject::Text, "Text"),
];
const AREA: ReframeBox = ReframeBox {
    center: [0.5, 0.5],
    size: [0.25, 0.5],
};
const LEAST: f64 = 0.02;
const GRAB: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hold {
    Move,
    Corner(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Held {
    hold: Hold,
    from: [f64; 2],
    start: ReframeBox,
}

#[derive(Debug, Clone, Copy)]
pub struct Reframing {
    mode: ReframeMode,
    subject: Subject,
    smooth: f64,
    point: bool,
    area: ReframeBox,
    held: Option<Held>,
}

impl Default for Reframing {
    fn default() -> Self {
        Self {
            mode: ReframeMode::Auto,
            subject: Subject::Person,
            smooth: USUAL_SMOOTH,
            point: false,
            area: AREA,
            held: None,
        }
    }
}

impl Reframing {
    fn params(&self, clips: &[String], at: i64) -> Value {
        let mut params = json!({ "clips": clips });
        if self.mode != ReframeMode::Auto {
            params["mode"] = json!(self.mode);
        }
        if self.pointed(clips.len()) {
            params["reference"] = json!(self.area);
            params["at"] = json!(at);
        } else if self.subject != Subject::Person {
            params["subject"] = json!(self.subject);
        }
        if self.smooth != USUAL_SMOOTH {
            params["smooth"] = json!(self.smooth);
        }
        params
    }

    fn pointed(&self, clips: usize) -> bool {
        self.point && clips == 1
    }
}

fn takes(p: &Project, id: &str) -> bool {
    vidcut_core::ops::picture::reframable(p, id).is_ok()
}

fn corners(b: &ReframeBox) -> [[f64; 2]; 4] {
    let [x0, y0] = [0, 1].map(|i| b.center[i] - b.size[i] / 2.0);
    let [x1, y1] = [0, 1].map(|i| b.center[i] + b.size[i] / 2.0);
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn moved(h: &Held, now: [f64; 2]) -> ReframeBox {
    let b = h.start;
    match h.hold {
        Hold::Move => ReframeBox {
            center: [0, 1].map(|i| {
                let half = b.size[i] / 2.0;
                (b.center[i] + now[i] - h.from[i]).clamp(half, 1.0 - half)
            }),
            size: b.size,
        },
        Hold::Corner(k) => {
            let all = corners(&b);
            let (held, fixed) = (all[k], all[(k + 2) % 4]);
            let edge = [0, 1].map(|i| {
                let to = now[i].clamp(0.0, 1.0);
                match held[i] > fixed[i] {
                    true => to.max(fixed[i] + LEAST),
                    false => to.min(fixed[i] - LEAST),
                }
            });
            ReframeBox {
                center: [0, 1].map(|i| (edge[i] + fixed[i]) / 2.0),
                size: [0, 1].map(|i| (edge[i] - fixed[i]).abs()),
            }
        }
    }
}

fn rows(ui: &mut egui::Ui, r: &mut Reframing, clips: usize) -> bool {
    let one = clips == 1;
    label(ui, "Object of Interest", r.pointed(clips));
    let aim = ui
        .horizontal(|ui| {
            ui.add_enabled_ui(one, |ui| {
                dialog::switch(ui, |ui| {
                    for (on, text) in [(false, "Auto"), (true, "Reference Point")] {
                        let lit = on == r.pointed(clips);
                        if ui.add(egui::Button::selectable(lit, text)).clicked() {
                            r.point = on;
                        }
                    }
                });
            });
            let target = egui::Button::new(egui_phosphor::regular::TARGET);
            ui.add_enabled(r.pointed(clips), target)
                .on_hover_text("Smart Reframe Viewer Mode")
                .clicked()
        })
        .inner;
    ui.end_row();
    label(ui, "Mode", r.mode != ReframeMode::Auto);
    dialog::switch(ui, |ui| {
        for m in ReframeMode::ALL {
            if ui
                .add(egui::Button::selectable(m == r.mode, m.label()))
                .clicked()
            {
                r.mode = m;
            }
        }
    });
    ui.end_row();
    if !r.pointed(clips) {
        label(ui, "Subject", r.subject != Subject::Person);
        let shown = SUBJECTS.iter().find(|(s, _)| *s == r.subject);
        egui::ComboBox::from_id_salt("reframe-subject")
            .selected_text(shown.map_or("", |(_, l)| *l))
            .show_ui(ui, |ui| {
                for (s, l) in SUBJECTS {
                    ui.selectable_value(&mut r.subject, s, l);
                }
            });
        ui.end_row();
    }
    label(ui, "Smooth", r.smooth != USUAL_SMOOTH);
    ui.add(
        egui::DragValue::new(&mut r.smooth)
            .range(SMOOTH.0..=SMOOTH.1)
            .speed(0.01)
            .fixed_decimals(2)
            .suffix(" s"),
    );
    ui.end_row();
    aim
}

impl Workspace {
    pub(super) fn reframe_section(&mut self, ui: &mut egui::Ui, p: &Project, ids: &[String]) {
        let clips: Vec<String> = ids.iter().filter(|id| takes(p, id)).cloned().collect();
        if clips.is_empty() {
            return;
        }
        let running = (self.snap.jobs.iter()).any(|j| j.unfinished() && j.kind == "reframe");
        let mut r = self.inspector.reframing;
        let (mut go, mut aim) = (false, false);
        switched_group(
            ui,
            NAME,
            None,
            |_| (),
            |ui| {
                aim = rows(ui, &mut r, clips.len());
                label(ui, "", false);
                go = ui
                    .add_enabled(!running, egui::Button::new("Reframe"))
                    .clicked();
                ui.end_row();
            },
        );
        self.inspector.reframing = r;
        if aim {
            self.show_reframe_tool();
        }
        if go {
            let at = self.snap.timeline.position;
            let ask = ("picture.reframe", r.params(&clips, at));
            match r.pointed(clips.len()) {
                true => self.ask(vec![ask], None),
                false => self.ask(vec![ask], Some((r.subject, FindModel::Fast))),
            }
        }
    }

    pub(super) fn reframe_shown(&self) -> bool {
        let g = &self.timeline.picture_grab;
        g.reframe && g.tool.is_none() && !g.color
    }

    pub(super) fn show_reframe_tool(&mut self) {
        let g = &mut self.timeline.picture_grab;
        (g.tool, g.color, g.reframe) = (None, false, true);
        self.select_shown();
    }

    pub(super) fn reframe_tool_button(&mut self, ui: &mut egui::Ui, clip: Option<&str>) {
        let lit = self.reframe_shown();
        let found = (self.project_shared()).is_some_and(|p| clip.is_some_and(|id| takes(&p, id)));
        let b = egui::Button::selectable(lit, NAME);
        if ui.add_enabled(found || lit, b).clicked() {
            match lit {
                true => self.timeline.picture_grab.reframe = false,
                false => self.show_reframe_tool(),
            }
        }
    }

    pub(super) fn reframe_controls(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        picture: Rect,
    ) -> bool {
        if !self.reframe_shown() {
            self.inspector.reframing.held = None;
            return false;
        }
        let back = |p: Pos2| {
            [
                f64::from((p.x - picture.min.x) / picture.width()),
                f64::from((p.y - picture.min.y) / picture.height()),
            ]
        };
        let at = |[x, y]: [f64; 2]| {
            picture.min + vec2(x as f32 * picture.width(), y as f32 * picture.height())
        };
        let grip = |b: &ReframeBox, p: Pos2| {
            let all = corners(b);
            match all.iter().position(|c| at(*c).distance(p) <= GRAB * 1.6) {
                Some(k) => Some(Hold::Corner(k)),
                None => {
                    let ([x, y], [x0, y0], [x1, y1]) = (back(p), all[0], all[2]);
                    (x >= x0 && x <= x1 && y >= y0 && y <= y1).then_some(Hold::Move)
                }
            }
        };
        let r = &mut self.inspector.reframing;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if resp.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some(hold) = grip(&r.area, press)
        {
            r.held = Some(Held {
                hold,
                from: back(press),
                start: r.area,
            });
        }
        if resp.dragged()
            && let (Some(h), Some(p)) = (r.held, pointer)
        {
            r.area = moved(&h, back(p));
        }
        if resp.drag_stopped() {
            r.held = None;
        }
        let hover = pointer
            .filter(|_| resp.hovered())
            .and_then(|p| grip(&r.area, p));
        let cursor = match r.held.map(|h| h.hold).or(hover) {
            Some(Hold::Move) => Some(CursorIcon::Move),
            Some(Hold::Corner(k)) if k % 2 == 0 => Some(CursorIcon::ResizeNwSe),
            Some(Hold::Corner(_)) => Some(CursorIcon::ResizeNeSw),
            None => None,
        };
        if let Some(c) = cursor {
            ui.ctx().set_cursor_icon(c);
        }
        let painter = ui.painter_at(picture);
        let accent = theme::colors().accent;
        let points = corners(&r.area).map(at);
        for i in 0..4 {
            painter.line_segment([points[i], points[(i + 1) % 4]], Stroke::new(1.5, accent));
        }
        for c in points {
            painter.rect_filled(
                Rect::from_center_size(c, Vec2::splat(GRAB * 1.4)),
                1.0,
                accent,
            );
        }
        painter.circle_filled(at(r.area.center), GRAB * 0.5, accent);
        true
    }
}
