use eframe::egui::{
    self, Align, Color32, Layout, Painter, Rect, RichText, Sense, Stroke, StrokeKind, TextStyle,
    vec2,
};
use serde_json::json;
use vidcut_core::model::{Generator, GeneratorKind};

use super::theme;
use super::workspace::Workspace;

type Card = (&'static str, GeneratorKind);

pub const CARDS: [Card; 1] = [(Generator::SOLID_COLOR, GeneratorKind::SolidColor)];

const CARD_WIDTH: f32 = 120.0;
const CARD_HEIGHT: f32 = 68.0;

fn paint(painter: &Painter, rect: Rect, kind: GeneratorKind) {
    match kind {
        GeneratorKind::SolidColor => {
            painter.rect_filled(rect, 0.0, Color32::BLACK);
        }
    }
}

impl Workspace {
    pub(super) fn generators_panel(&mut self, ui: &mut egui::Ui) {
        let cell = vec2(
            CARD_WIDTH,
            CARD_HEIGHT + ui.spacing().item_spacing.y + ui.text_style_height(&TextStyle::Body),
        );
        let mut grabbed = None;
        let mut doubled = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(RichText::new("Generators").strong());
                ui.horizontal_wrapped(|ui| {
                    for (label, kind) in CARDS {
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
                                doubled = Some(kind);
                            }
                        });
                    }
                });
            });
        if let Some((card, press)) = grabbed
            && let Some(p) = self.project_shared()
        {
            self.start_generator_drag(&p, card, press);
        }
        if let Some(kind) = doubled {
            let at = self.snap.timeline.position;
            self.send("generators.add", json!({ "kind": kind, "at": at }));
        }
    }
}
