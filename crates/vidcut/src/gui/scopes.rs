use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, PopupCloseBehavior, Pos2, Rect, Sense,
    pos2, vec2,
};
use eframe::emath::GuiRounding;
use egui_phosphor::regular as icon;
use serde::Serialize;
use serde_json::json;
use vidcut_av::screen::scope::marks;
use vidcut_core::preferences::{
    ChromaticityMode, HistogramMode, ParadeMode, Scope, ScopeScale, WaveformMode,
};

use super::theme;
use super::workspace::Workspace;

const COLUMN: f32 = 0.3;

const NARROWEST: f32 = 120.0;

const WIDEST: f32 = 320.0;

const GAP: f32 = 4.0;

const MARK_FONT: f32 = 10.0;

const MARK_ROOM: f32 = 12.0;

const OPTIONS_SIDE: f32 = 16.0;

const OPTIONS_ICON: f32 = 12.0;

fn options_name(scope: Scope) -> String {
    format!("{} Options", scope.name())
}

fn field(scope: Scope) -> &'static str {
    match scope {
        Scope::Waveform => "waveform",
        Scope::Parade => "parade",
        Scope::Vectorscope => "vectorscope",
        Scope::Histogram => "histogram",
        Scope::Chromaticity => "chromaticity",
    }
}

fn side(v: f32) -> Align {
    if v <= 0.0 {
        Align::Min
    } else if v >= 1.0 {
        Align::Max
    } else {
        Align::Center
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
                Scope::Waveform | Scope::Parade | Scope::Histogram | Scope::Chromaticity => cell,
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
        ui.separator();
        for scope in Scope::ALL {
            ui.menu_button(options_name(scope), |ui| self.scope_options(ui, scope));
        }
        ui.menu_button("Waveform Scale Style", |ui| {
            let now = self.snap.scopes.scale;
            for scale in ScopeScale::ALL {
                if ui.radio(now == scale, scale.name()).clicked() && now != scale {
                    self.send("viewer.scopes", json!({ "scale": scale }));
                }
            }
            ui.separator();
            let video = self.snap.scopes.video_levels;
            for (on, name) in [(true, "Video Level Scopes"), (false, "Data Level Scopes")] {
                if ui.radio(video == on, name).clicked() && video != on {
                    self.send("viewer.scopes", json!({ "video_levels": on }));
                }
            }
        });
    }

    fn choose<T: Copy + PartialEq + Serialize>(
        &self,
        ui: &mut egui::Ui,
        key: &str,
        now: T,
        (all, name): (&[T], fn(T) -> &'static str),
    ) {
        for &choice in all {
            if ui.radio(now == choice, name(choice)).clicked() && now != choice {
                self.send("viewer.scopes", json!({ key: choice }));
            }
        }
    }

    fn scope_options(&self, ui: &mut egui::Ui, scope: Scope) {
        let s = self.snap.scopes;
        match scope {
            Scope::Waveform => {
                let modes = (&WaveformMode::ALL[..], WaveformMode::name as fn(_) -> _);
                self.choose(ui, "waveform_mode", s.waveform_mode, modes);
                ui.separator();
                ui.add_enabled_ui(s.waveform_mode == WaveformMode::Rgb, |ui| {
                    for (name, key, on) in [
                        ("R", "waveform_red", s.waveform_red),
                        ("G", "waveform_green", s.waveform_green),
                        ("B", "waveform_blue", s.waveform_blue),
                    ] {
                        let mut on = on;
                        if ui.checkbox(&mut on, name).clicked() {
                            self.send("viewer.scopes", json!({ key: on }));
                        }
                    }
                });
            }
            Scope::Parade => {
                let modes = (&ParadeMode::ALL[..], ParadeMode::name as fn(_) -> _);
                self.choose(ui, "parade_mode", s.parade_mode, modes);
            }
            Scope::Vectorscope => {
                let mut on = s.skin_tone;
                if ui.checkbox(&mut on, "Show Skin Tone Indicator").clicked() {
                    self.send("viewer.scopes", json!({ "skin_tone": on }));
                }
            }
            Scope::Histogram => {
                let modes = (&HistogramMode::ALL[..], HistogramMode::name as fn(_) -> _);
                self.choose(ui, "histogram_mode", s.histogram_mode, modes);
            }
            Scope::Chromaticity => {
                let modes = (
                    &ChromaticityMode::ALL[..],
                    ChromaticityMode::name as fn(_) -> _,
                );
                self.choose(ui, "chromaticity_mode", s.chromaticity_mode, modes);
            }
        }
    }

    fn options_button(&self, ui: &egui::Ui, scope: Scope, button: Rect) {
        let id = ui.scope_id().with(("scope options", field(scope)));
        let response = ui
            .interact(button, id, Sense::click())
            .on_hover_text(options_name(scope));
        let colors = theme::colors();
        let tint = match response.hovered() {
            true => colors.text,
            false => colors.text_dim,
        };
        ui.painter_at(button).text(
            button.center(),
            Align2::CENTER_CENTER,
            icon::SLIDERS_HORIZONTAL,
            FontId::proportional(OPTIONS_ICON),
            tint,
        );
        egui::Popup::from_toggle_button_response(&response)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.scope_options(ui, scope));
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
        let prefs = self.snap.scopes;
        let row = ui.painter().fonts_mut(|f| f.row_height(&font));
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
            for mark in marks(scope, (colors, prefs), size.max((1, 1))) {
                let at = rect.min + vec2(mark.at[0] * rect.width(), mark.at[1] * rect.height());
                let at = match scope {
                    Scope::Vectorscope => at + (at - rect.center()).normalized() * 12.0,
                    Scope::Waveform | Scope::Parade => at + vec2(2.0, 0.0),
                    Scope::Histogram => pos2(
                        at.x.clamp(rect.left() + MARK_ROOM, rect.right() - MARK_ROOM),
                        at.y + 2.0,
                    ),
                    Scope::Chromaticity => at,
                } + vec2(mark.shift[0], mark.shift[1]) * MARK_FONT;
                let align = Align2([side(mark.align[0]), side(mark.align[1])]);
                let lines: Vec<&str> = mark.label.lines().collect();
                let top = at.y - align.y().to_factor() * row * lines.len() as f32;
                for (i, line) in lines.into_iter().enumerate() {
                    let y = top + row * i as f32;
                    let align = Align2([align.x(), Align::Min]);
                    painter.text(pos2(at.x, y), align, line, font.clone(), dim);
                }
            }
            let corner = match scope {
                Scope::Histogram => Align2::RIGHT_BOTTOM,
                _ => Align2::RIGHT_TOP,
            };
            let button = corner.align_size_within_rect(
                vec2(OPTIONS_SIDE, OPTIONS_SIDE),
                rect.shrink2(vec2(2.0, 1.0)),
            );
            let name = pos2(button.left() - 1.0, button.center().y);
            painter.text(name, Align2::RIGHT_CENTER, scope.name(), font.clone(), dim);
            self.options_button(ui, scope, button);
        }
    }
}
