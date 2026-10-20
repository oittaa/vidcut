use std::collections::HashMap;

use eframe::egui::{self, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions};
use vidcut_av::burn::Sprite;
use vidcut_core::looks::{Look, Styled};
use vidcut_core::model::{Place, SubtitleStyle};

use super::title_browser::picture;
use super::{dialog, theme};

const SAMPLE: &str = "say it";
const SAMPLE_WORDS: [i64; 2] = [0, 10];
const GAP: f32 = 8.0;
const TILE: egui::Vec2 = egui::vec2((dialog::WIDTH - 4.0 * GAP) / 5.0, 44.0);

fn sample(style: &SubtitleStyle, (w, h): (u32, u32)) -> Vec<Sprite> {
    let style = SubtitleStyle {
        size: Some(0.34),
        margin: Some(0.03),
        ..style.clone()
    };
    let shown = vidcut_core::subtitles::shown(SAMPLE, &SAMPLE_WORDS, &style, 0, 30.0);
    vidcut_av::burn::draw(&[(&shown, Place::Middle)], w, h, &style).all()
}

#[derive(Default)]
pub(super) struct Tiles {
    kept: HashMap<String, TextureHandle>,
}

impl std::fmt::Debug for Tiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tiles")
            .field("kept", &self.kept.len())
            .finish()
    }
}

impl Tiles {
    fn texture(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        style: &SubtitleStyle,
        size: (u32, u32),
    ) -> egui::TextureId {
        let key = format!(
            "{name} {}x{} {}",
            size.0,
            size.1,
            serde_json::to_string(style).unwrap_or_default()
        );
        self.kept
            .entry(key)
            .or_insert_with(|| {
                ctx.load_texture(
                    format!("look-tile-{name}"),
                    picture(&sample(style, size), size, 0),
                    TextureOptions::LINEAR,
                )
            })
            .id()
    }

    pub(super) fn show<'a>(
        &mut self,
        ui: &mut egui::Ui,
        looks: &'a [Look],
        chosen: &str,
    ) -> Option<&'a Look> {
        let ppp = ui.ctx().pixels_per_point();
        let size = ((TILE.x * ppp).round() as u32, (TILE.y * ppp).round() as u32);
        let c = theme::colors();
        let corners = ui.visuals().widgets.inactive.corner_radius;
        let mut took = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
            for look in looks {
                let Styled::Subtitles(l) = &look.styled else {
                    continue;
                };
                let id = self.texture(ui.ctx(), &look.name, &l.style, size);
                let on = look.name.eq_ignore_ascii_case(chosen);
                let r = ui
                    .vertical(|ui| {
                        ui.set_width(TILE.x);
                        ui.spacing_mut().item_spacing.y = 3.0;
                        let tile = ui.add(
                            egui::Image::new((id, TILE))
                                .corner_radius(corners)
                                .sense(Sense::click()),
                        );
                        let stroke = match (on, tile.hovered()) {
                            (true, _) => Stroke::new(2.0, c.selection),
                            (false, true) => Stroke::new(1.0, c.text_dim),
                            (false, false) => Stroke::new(1.0, c.line),
                        };
                        ui.painter()
                            .rect_stroke(tile.rect, corners, stroke, StrokeKind::Outside);
                        let name = RichText::new(&look.name).small().color(match on {
                            true => c.text,
                            false => c.text_dim,
                        });
                        let label = ui.add(egui::Label::new(name).truncate().sense(Sense::click()));
                        tile.union(label)
                    })
                    .inner;
                let r = match look.description {
                    "" => r,
                    d => r.on_hover_text(d),
                };
                if r.clicked() {
                    took = Some(look);
                }
            }
        });
        took
    }
}
