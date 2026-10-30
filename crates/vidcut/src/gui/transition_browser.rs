use eframe::egui::{
    self, Align, Layout, Mesh, Painter, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    TextStyle, pos2, vec2,
};
use serde_json::{Value, json};
use vidcut_core::model::{Motion, PictureTransition, PushPreset, SlidePreset, WipeDirection};

use super::theme;
use super::timeline::Side;
use super::workspace::{Workspace, dim};

type Card = (&'static str, PictureTransition);

pub const GROUPS: [(&str, &[Card]); 3] = [
    (
        "Dissolve",
        &[
            ("Cross Dissolve", PictureTransition::Dissolve),
            (
                "Dip To Color Dissolve",
                PictureTransition::DipToColor { color: [0, 0, 0] },
            ),
        ],
    ),
    (
        "Wipe",
        &[(
            "Edge Wipe",
            PictureTransition::Wipe {
                direction: WipeDirection::Down,
            },
        )],
    ),
    (
        "Motion",
        &[
            (
                "Push",
                PictureTransition::Push {
                    preset: PushPreset::Left,
                    motion: Motion::PLAIN,
                },
            ),
            (
                "Slide",
                PictureTransition::Slide {
                    preset: SlidePreset::LeftRight,
                    out: false,
                    motion: Motion::PLAIN,
                },
            ),
        ],
    ),
];

const CARD_WIDTH: f32 = 120.0;
const CARD_HEIGHT: f32 = 68.0;

fn paint(painter: &Painter, rect: Rect, kind: &PictureTransition) {
    let colors = theme::colors();
    let (from, to) = (colors.track, colors.clip);
    match kind {
        PictureTransition::Dissolve => {
            let mut mesh = Mesh::default();
            for (pos, color) in [
                (rect.left_top(), from),
                (rect.right_top(), to),
                (rect.right_bottom(), to),
                (rect.left_bottom(), from),
            ] {
                mesh.colored_vertex(pos, color);
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(Shape::mesh(mesh));
        }
        PictureTransition::DipToColor { color } => {
            let [r, g, b] = *color;
            let dip = egui::Color32::from_rgb(r, g, b);
            let mid = rect.center().x;
            let mut mesh = Mesh::default();
            for (x, c) in [(rect.left(), from), (mid, dip), (rect.right(), to)] {
                mesh.colored_vertex(pos2(x, rect.top()), c);
                mesh.colored_vertex(pos2(x, rect.bottom()), c);
            }
            for i in [0, 2] {
                mesh.add_triangle(i, i + 1, i + 3);
                mesh.add_triangle(i, i + 3, i + 2);
            }
            painter.add(Shape::mesh(mesh));
        }
        PictureTransition::Wipe { direction } => {
            painter.rect_filled(rect, 0.0, from);
            let c = rect.center();
            let incoming = match direction {
                WipeDirection::Left => Rect::from_min_max(rect.min, pos2(c.x, rect.bottom())),
                WipeDirection::Right => Rect::from_min_max(pos2(c.x, rect.top()), rect.max),
                WipeDirection::Up => Rect::from_min_max(rect.min, pos2(rect.right(), c.y)),
                WipeDirection::Down => Rect::from_min_max(pos2(rect.left(), c.y), rect.max),
            };
            painter.rect_filled(incoming, 0.0, to);
        }
        PictureTransition::Push { .. } | PictureTransition::Slide { .. } => {
            painter.rect_filled(rect, 0.0, from);
            let edge = rect.left() + rect.width() * 0.45;
            let incoming = Rect::from_min_max(rect.min, pos2(edge, rect.bottom()));
            painter.rect_filled(incoming, 0.0, to);
            let line = Stroke::new(2.0, colors.text);
            if matches!(kind, PictureTransition::Push { .. }) {
                let c = rect.center();
                painter.arrow(pos2(c.x - 18.0, c.y), vec2(36.0, 0.0), line);
            } else {
                painter.line_segment([pos2(edge, rect.top()), pos2(edge, rect.bottom())], line);
            }
        }
    }
}

impl Workspace {
    fn selected_transition_params(&self, kind: &PictureTransition) -> Option<Value> {
        let audio = self.timeline.edit.is_some_and(|e| e.side == Side::Audio);
        let none = self.timeline.edit.is_none() && self.snap.selection.clips.is_empty();
        if audio || none {
            return None;
        }
        let mut params = self.dissolve_target();
        params["transition"] = json!(kind);
        params["sound"] = json!(false);
        Some(params)
    }

    pub(super) fn transitions_panel(&mut self, ui: &mut egui::Ui) {
        if self.project_shared().is_none() {
            ui.label(dim("Open a project to add transitions"));
            return;
        }
        let cell = vec2(
            CARD_WIDTH,
            CARD_HEIGHT + ui.spacing().item_spacing.y + ui.text_style_height(&TextStyle::Body),
        );
        let mut grabbed = None;
        let mut added = None;
        let mut to_selected = None;
        let can_select = |kind| self.selected_transition_params(kind).is_some();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (group, cards) in GROUPS {
                    ui.label(RichText::new(group).strong());
                    ui.horizontal_wrapped(|ui| {
                        for (label, kind) in cards {
                            ui.allocate_ui_with_layout(cell, Layout::top_down(Align::Min), |ui| {
                                let (rect, resp) = ui.allocate_exact_size(
                                    vec2(CARD_WIDTH, CARD_HEIGHT),
                                    Sense::click_and_drag(),
                                );
                                paint(ui.painter(), rect, kind);
                                if resp.hovered() {
                                    ui.painter().rect_stroke(
                                        rect,
                                        2.0,
                                        Stroke::new(1.5, theme::colors().accent),
                                        StrokeKind::Inside,
                                    );
                                }
                                ui.label(*label);
                                if resp.drag_started()
                                    && let Some(press) = ui.input(|i| i.pointer.press_origin())
                                {
                                    grabbed = Some(((kind.clone(), *label), press));
                                }
                                if resp.double_clicked() {
                                    added = Some(kind.clone());
                                }
                                resp.context_menu(|ui| {
                                    let item =
                                        egui::Button::new("Add to Selected Edit Points and Clips");
                                    if ui.add_enabled(can_select(kind), item).clicked() {
                                        to_selected = Some(kind.clone());
                                        ui.close();
                                    }
                                });
                            });
                        }
                    });
                    ui.add_space(6.0);
                }
            });
        if let Some((card, press)) = grabbed {
            self.start_transition_drag(card, press);
        }
        if let Some(kind) = added {
            let at = self.snap.timeline.position;
            self.send(
                "edit.dissolve",
                json!({ "at": at, "transition": kind, "sound": false }),
            );
        }
        if let Some(params) = to_selected.and_then(|k| self.selected_transition_params(&k)) {
            self.send("edit.dissolve", params);
        }
    }
}
