use eframe::egui::{
    self, Align, Layout, Mesh, Painter, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    TextStyle, pos2, vec2,
};
use serde_json::{Value, json};
use vidcut_core::model::{PictureTransition, WipeDirection};

use super::theme;
use super::timeline::Side;
use super::workspace::{Workspace, dim};

type Card = (&'static str, PictureTransition);

pub const GROUPS: [(&str, &[Card]); 2] = [
    (
        "Dissolve",
        &[("Cross Dissolve", PictureTransition::Dissolve)],
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
];

const CARD_WIDTH: f32 = 120.0;
const CARD_HEIGHT: f32 = 68.0;

fn paint(painter: &Painter, rect: Rect, kind: PictureTransition) {
    let colors = theme::colors();
    let (from, to) = (colors.clip, colors.accent);
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
    }
}

impl Workspace {
    fn selected_transition_params(&self, kind: PictureTransition) -> Option<Value> {
        match self.timeline.edit {
            Some(e) if e.side != Side::Audio => {
                let align = match e.side {
                    Side::Out => "end",
                    Side::In => "start",
                    Side::Both | Side::Audio => "center",
                };
                Some(json!({
                    "at": e.at,
                    "track": e.track.id(),
                    "align": align,
                    "transition": kind,
                    "sound": false,
                }))
            }
            _ if !self.snap.selection.clips.is_empty() => Some(json!({
                "at": self.snap.timeline.position,
                "clips": self.snap.selection.clips,
                "transition": kind,
                "sound": false,
            })),
            _ => None,
        }
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
                        for &(label, kind) in cards {
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
                                ui.label(label);
                                if resp.drag_started()
                                    && let Some(press) = ui.input(|i| i.pointer.press_origin())
                                {
                                    grabbed = Some(((kind, label), press));
                                }
                                if resp.double_clicked() {
                                    added = Some(kind);
                                }
                                resp.context_menu(|ui| {
                                    let item =
                                        egui::Button::new("Add to Selected Edit Points and Clips");
                                    if ui.add_enabled(can_select(kind), item).clicked() {
                                        to_selected = Some(kind);
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
        if let Some(params) = to_selected.and_then(|k| self.selected_transition_params(k)) {
            self.send("edit.dissolve", params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GROUPS;
    use vidcut_core::model::{PictureTransition, WipeDirection};

    #[test]
    fn the_edge_wipe_card_comes_in_from_the_bottom() {
        assert_eq!(
            GROUPS[1].1[0],
            (
                "Edge Wipe",
                PictureTransition::Wipe {
                    direction: WipeDirection::Down
                }
            )
        );
    }
}
