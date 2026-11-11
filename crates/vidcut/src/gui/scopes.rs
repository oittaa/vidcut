use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, pos2, vec2};
use eframe::emath::GuiRounding;
use serde_json::json;
use vidcut_av::screen::scope::marks;
use vidcut_core::preferences::{Scope, ScopeScale};

use super::theme;
use super::workspace::Workspace;

const COLUMN: f32 = 0.3;

const NARROWEST: f32 = 120.0;

const WIDEST: f32 = 320.0;

const GAP: f32 = 4.0;

const MARK_FONT: f32 = 10.0;

fn field(scope: Scope) -> &'static str {
    match scope {
        Scope::Waveform => "waveform",
        Scope::Parade => "parade",
        Scope::Vectorscope => "vectorscope",
    }
}

fn places(column: Rect, scopes: &[Scope], ppp: f32) -> Vec<(Scope, Rect)> {
    let count = scopes.len() as f32;
    let tall = (column.height() - GAP * (count - 1.0)) / count;
    scopes
        .iter()
        .enumerate()
        .map(|(i, &scope)| {
            let top = column.top() + (tall + GAP) * i as f32;
            let cell = Rect::from_min_size(
                pos2(column.left(), top),
                vec2(column.width(), tall.max(1.0)),
            );
            let rect = match scope {
                Scope::Vectorscope => Rect::from_center_size(
                    cell.center(),
                    egui::Vec2::splat(cell.width().min(cell.height())),
                ),
                Scope::Waveform | Scope::Parade => cell,
            };
            (scope, rect.round_to_pixels(ppp))
        })
        .collect()
}

impl Workspace {
    pub(super) fn scopes_menu(&mut self, ui: &mut egui::Ui) {
        for scope in Scope::ALL {
            let mut on = self.snap.scopes.has(scope);
            if ui.checkbox(&mut on, scope.name()).clicked() {
                self.send("viewer.scopes", json!({ field(scope): on }));
            }
        }
        ui.menu_button("Waveform Scale Style", |ui| {
            let now = self.snap.scopes.scale;
            for scale in ScopeScale::ALL {
                if ui.radio(now == scale, scale.name()).clicked() && now != scale {
                    self.send("viewer.scopes", json!({ "scale": scale }));
                }
            }
        });
    }

    pub(super) fn scope_column(
        &mut self,
        area: Rect,
        shows: bool,
        ppp: f32,
    ) -> (Rect, Vec<(Scope, Rect)>) {
        let scopes = self.snap.scopes.on();
        if !shows || scopes.is_empty() {
            self.scope_want.clear();
            return (area, Vec::new());
        }
        let wide = (area.width() * COLUMN)
            .clamp(NARROWEST, WIDEST)
            .min(area.width() * 0.5);
        let column = Rect::from_min_max(pos2(area.right() - wide, area.top()), area.max);
        let picture = Rect::from_min_max(area.min, pos2(column.left() - GAP, area.bottom()));
        let plots = places(column, &scopes, ppp);
        self.scope_want = plots
            .iter()
            .map(|&(scope, r)| {
                let size = [r.width(), r.height()].map(|v| (v * ppp).round() as i32);
                (scope, size)
            })
            .collect();
        (picture, plots)
    }

    pub(super) fn paint_scopes(&self, ui: &egui::Ui, plots: &[(Scope, Rect)]) {
        let ppp = ui.ctx().pixels_per_point();
        let colors = self.scope_pictures.colors;
        let dim = theme::colors().text_dim;
        let font = FontId::proportional(MARK_FONT);
        let scale = self.snap.scopes.scale;
        for &(scope, rect) in plots {
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::BLACK);
            let texture = self
                .scope_pictures
                .textures
                .iter()
                .find(|(s, _)| *s == scope);
            if let Some((_, texture)) = texture {
                painter.image(
                    *texture,
                    rect,
                    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let size = (
                (rect.width() * ppp).round() as u32,
                (rect.height() * ppp).round() as u32,
            );
            for mark in marks(scope, (colors, scale), size.max((1, 1))) {
                let at = rect.min + vec2(mark.at[0] * rect.width(), mark.at[1] * rect.height());
                let (at, align) = match scope {
                    Scope::Vectorscope => {
                        let out = (at - rect.center()).normalized() * 12.0;
                        (at + out, Align2::CENTER_CENTER)
                    }
                    Scope::Waveform | Scope::Parade => (at + vec2(2.0, 0.0), Align2::LEFT_BOTTOM),
                };
                painter.text(at, align, mark.label, font.clone(), dim);
            }
            painter.text(
                rect.right_top() + vec2(-3.0, 2.0),
                Align2::RIGHT_TOP,
                scope.name(),
                font.clone(),
                dim,
            );
        }
    }
}
