use eframe::egui::{
    self, Align, Color32, Layout, Painter, Rect, RichText, Sense, Stroke, StrokeKind, TextStyle,
    vec2,
};
use serde_json::json;
use vidcut_core::model::EffectType;

use super::theme;
use super::workspace::{Workspace, dim};

type Card = (&'static str, EffectType);

pub const GROUPS: [(&str, &[Card]); 1] = [("Blur", &[("Gaussian Blur", EffectType::GaussianBlur)])];

const CARD_WIDTH: f32 = 120.0;
const CARD_HEIGHT: f32 = 68.0;

fn paint(painter: &Painter, rect: Rect, kind: EffectType) {
    let colors = theme::colors();
    painter.rect_filled(rect, 0.0, colors.track);
    match kind {
        EffectType::GaussianBlur => {
            let c = rect.center();
            let r = rect.height() * 0.3;
            for k in (1..=8).rev() {
                let t = k as f32 / 8.0;
                let a = ((1.0 - t) * 90.0) as u8 + 20;
                let fill = Color32::from_rgba_unmultiplied(
                    colors.text.r(),
                    colors.text.g(),
                    colors.text.b(),
                    a,
                );
                painter.circle_filled(c, r * (0.6 + t), fill);
            }
        }
    }
}

impl Workspace {
    fn pictured_selection(&self) -> Vec<String> {
        let Some(p) = self.project() else {
            return Vec::new();
        };
        self.snap
            .selection
            .clips
            .iter()
            .filter(|id| {
                p.find_clip(id).is_some_and(|(ti, ci)| {
                    vidcut_core::ops::picture::has_picture(&p.tracks[ti].clips[ci])
                })
            })
            .cloned()
            .collect()
    }

    pub(super) fn effects_panel(&mut self, ui: &mut egui::Ui) {
        if self.project_shared().is_none() {
            ui.label(dim("Open a project to add effects"));
            return;
        }
        let cell = vec2(
            CARD_WIDTH,
            CARD_HEIGHT + ui.spacing().item_spacing.y + ui.text_style_height(&TextStyle::Body),
        );
        let selected = self.pictured_selection();
        let mut grabbed = None;
        let mut added = None;
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
                                paint(ui.painter(), rect, *kind);
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
                                    grabbed = Some(((*kind, *label), press));
                                }
                                if resp.double_clicked() {
                                    added = Some(*kind);
                                }
                                resp.context_menu(|ui| {
                                    let item = egui::Button::new("Add to Selected Clips");
                                    if ui.add_enabled(!selected.is_empty(), item).clicked() {
                                        added = Some(*kind);
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
            self.start_effect_drag(card, press);
        }
        if let Some(kind) = added.filter(|_| !selected.is_empty()) {
            self.send("effects.add", json!({ "clips": selected, "kind": kind }));
        }
    }
}
