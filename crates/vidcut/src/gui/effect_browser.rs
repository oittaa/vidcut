use eframe::egui::{
    self, Align, Color32, Layout, Painter, Rect, RichText, Sense, Stroke, StrokeKind, TextStyle,
    vec2,
};
use serde_json::json;
use vidcut_core::model::{Clip, EffectType};

use super::theme;
use super::workspace::{Workspace, dim};

type Card = (&'static str, EffectType);

pub const GROUPS: [(&str, &[Card]); 4] = [
    (
        "Blur",
        &[
            ("Gaussian Blur", EffectType::GaussianBlur),
            ("Mosaic Blur", EffectType::MosaicBlur),
        ],
    ),
    ("Light", &[("Glow", EffectType::Glow)]),
    (
        "Film Emulation",
        &[
            ("Film Grain", EffectType::FilmGrain),
            ("Vignette", EffectType::Vignette),
        ],
    ),
    ("Sharpen", &[("Sharpen", EffectType::Sharpen)]),
];

const CARD_WIDTH: f32 = 120.0;
const CARD_HEIGHT: f32 = 68.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EffectsTab {
    #[default]
    Video,
    Generators,
}

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
        EffectType::MosaicBlur => {
            let side = rect.height() / 5.0;
            for row in 0..5 {
                for column in 0..(rect.width() / side).ceil() as i32 {
                    let a = (20 + (column * 7 + row * 13).rem_euclid(5) * 22) as u8;
                    let fill = Color32::from_rgba_unmultiplied(
                        colors.text.r(),
                        colors.text.g(),
                        colors.text.b(),
                        a,
                    );
                    let at = rect.min + vec2(column as f32, row as f32) * side;
                    let cell = Rect::from_min_size(at, vec2(side, side)).intersect(rect);
                    painter.rect_filled(cell, 0.0, fill);
                }
            }
        }
        EffectType::Glow => {
            let c = rect.center();
            let r = rect.height() * 0.42;
            for k in (1..=8).rev() {
                let t = k as f32 / 8.0;
                let fill = Color32::from_rgba_unmultiplied(
                    colors.text.r(),
                    colors.text.g(),
                    colors.text.b(),
                    ((1.0 - t) * 60.0) as u8 + 8,
                );
                painter.circle_filled(c, r * t, fill);
            }
            painter.circle_filled(c, r * 0.18, colors.text);
        }
        EffectType::FilmGrain => {
            for k in 0..240_i32 {
                let across = (k * 37).rem_euclid(64) as f32 / 64.0;
                let down = (k * 23).rem_euclid(64) as f32 / 64.0;
                let at = rect.min + vec2(across * rect.width(), down * rect.height());
                let fill = Color32::from_rgba_unmultiplied(
                    colors.text.r(),
                    colors.text.g(),
                    colors.text.b(),
                    (60 + (k * 13).rem_euclid(160)) as u8,
                );
                painter.circle_filled(at, 1.0, fill);
            }
        }
        EffectType::Vignette => {
            let c = rect.center();
            let r = rect.height() * 0.32;
            for k in (1..=8).rev() {
                let t = k as f32 / 8.0;
                let a = ((1.0 - t) * 180.0) as u8 + 20;
                let fill = Color32::from_rgba_unmultiplied(
                    colors.text.r(),
                    colors.text.g(),
                    colors.text.b(),
                    a,
                );
                let reach = vec2(r * (1.0 + t) * rect.aspect_ratio(), r * (1.0 + t));
                let oval = egui::epaint::EllipseShape::filled(c, reach, fill);
                painter.add(oval);
            }
        }
        EffectType::Sharpen => {
            let middle = rect.center().x;
            let line = rect.width() / 16.0;
            for k in 0..8 {
                let a = (150 - k * 18) as u8;
                let fill = Color32::from_rgba_unmultiplied(
                    colors.text.r(),
                    colors.text.g(),
                    colors.text.b(),
                    a,
                );
                let at = middle - line * (k + 1) as f32;
                let band = Rect::from_x_y_ranges(at..=at + line, rect.y_range());
                painter.rect_filled(band, 0.0, fill);
            }
            let edge = Rect::from_x_y_ranges(middle..=middle + line, rect.y_range());
            painter.rect_filled(edge, 0.0, colors.text);
        }
    }
}

fn paint_adjustment(painter: &Painter, rect: Rect) {
    let colors = theme::colors();
    painter.rect_filled(rect, 0.0, colors.track);
    let inset = rect.shrink2(vec2(rect.width() * 0.24, rect.height() * 0.22));
    let below = inset.translate(vec2(-6.0, 5.0));
    let above = inset.translate(vec2(6.0, -5.0));
    let dim_fill =
        Color32::from_rgba_unmultiplied(colors.text.r(), colors.text.g(), colors.text.b(), 35);
    let top_fill = Color32::from_rgba_unmultiplied(
        colors.accent.r(),
        colors.accent.g(),
        colors.accent.b(),
        70,
    );
    painter.rect_filled(below, 2.0, dim_fill);
    painter.rect_stroke(
        below,
        2.0,
        Stroke::new(1.0, colors.text_dim),
        StrokeKind::Inside,
    );
    painter.rect_filled(above, 2.0, top_fill);
    painter.rect_stroke(
        above,
        2.0,
        Stroke::new(1.2, colors.accent),
        StrokeKind::Inside,
    );
}

fn card_frame(
    ui: &mut egui::Ui,
    cell: egui::Vec2,
    label: &str,
    draw: impl FnOnce(&Painter, Rect),
) -> egui::Response {
    ui.allocate_ui_with_layout(cell, Layout::top_down(Align::Min), |ui| {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(CARD_WIDTH, CARD_HEIGHT), Sense::click_and_drag());
        draw(ui.painter(), rect);
        if resp.hovered() {
            ui.painter().rect_stroke(
                rect,
                2.0,
                Stroke::new(1.5, theme::colors().accent),
                StrokeKind::Inside,
            );
        }
        ui.label(label);
        resp
    })
    .inner
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
                    vidcut_core::ops::picture::takes_effects(&p.tracks[ti].clips[ci])
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
        ui.horizontal(|ui| {
            for (tab, label) in [
                (EffectsTab::Video, "Video"),
                (EffectsTab::Generators, "Generators"),
            ] {
                if ui
                    .selectable_label(self.effects_tab == tab, label)
                    .clicked()
                {
                    self.effects_tab = tab;
                }
            }
        });
        if self.effects_tab == EffectsTab::Generators {
            self.generators_panel(ui);
            return;
        }
        let cell = vec2(
            CARD_WIDTH,
            CARD_HEIGHT + ui.spacing().item_spacing.y + ui.text_style_height(&TextStyle::Body),
        );
        let selected = self.pictured_selection();
        let mut adj_grab = None;
        let mut adj_smart = false;
        let mut adj_top = false;
        let mut grabbed = None;
        let mut added = None;
        let mut doubled = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(RichText::new("Effects").strong());
                ui.horizontal_wrapped(|ui| {
                    let resp = card_frame(ui, cell, Clip::ADJUSTMENT_CLIP, paint_adjustment);
                    if resp.drag_started()
                        && let Some(press) = ui.input(|i| i.pointer.press_origin())
                    {
                        adj_grab = Some(press);
                    }
                    if resp.double_clicked() {
                        adj_smart = true;
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Smart Insert").clicked() {
                            adj_smart = true;
                            ui.close();
                        }
                        if ui.button("Place On Top").clicked() {
                            adj_top = true;
                            ui.close();
                        }
                    });
                });
                ui.add_space(6.0);
                for (group, cards) in GROUPS {
                    ui.label(RichText::new(group).strong());
                    ui.horizontal_wrapped(|ui| {
                        for (label, kind) in cards {
                            let resp = card_frame(ui, cell, label, |painter, rect| {
                                paint(painter, rect, *kind);
                            });
                            if resp.drag_started()
                                && let Some(press) = ui.input(|i| i.pointer.press_origin())
                            {
                                grabbed = Some(((*kind, *label), press));
                            }
                            if resp.double_clicked() {
                                doubled = Some(*kind);
                            }
                            resp.context_menu(|ui| {
                                let item = egui::Button::new("Add to Selected Clips");
                                if ui.add_enabled(!selected.is_empty(), item).clicked() {
                                    added = Some(*kind);
                                    ui.close();
                                }
                            });
                        }
                    });
                    ui.add_space(6.0);
                }
            });
        if let Some(press) = adj_grab {
            let len = self
                .project()
                .map_or(150, vidcut_core::ops::titles::default_length);
            self.start_adjustment_drag(len, press);
        }
        if adj_smart {
            self.send("edit.smart_insert", json!({ "adjustment": true }));
        }
        if adj_top {
            self.send("edit.place_on_top", json!({ "adjustment": true }));
        }
        if let Some((card, press)) = grabbed {
            self.start_effect_drag(card, press);
        }
        if let Some(kind) = doubled
            && let Some(first) = selected.first()
        {
            self.send("effects.add", json!({ "clips": [first], "kind": kind }));
        }
        if let Some(kind) = added.filter(|_| !selected.is_empty()) {
            self.send("effects.add", json!({ "clips": selected, "kind": kind }));
        }
    }
}
