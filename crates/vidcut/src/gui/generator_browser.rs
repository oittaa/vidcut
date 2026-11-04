use eframe::egui::{
    self, Align, Color32, ColorImage, Context, Id, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, TextStyle, TextureHandle, TextureOptions, pos2, vec2,
};
use serde_json::json;
use vidcut_core::generators::{Pattern, draw};
use vidcut_core::model::{Generator, GeneratorKind};
use vidcut_core::ops::generators::made;
use vidcut_core::render::Generated;
use vidcut_core::request::GeneratorSettings;

use super::theme;
use super::workspace::Workspace;

type Card = (&'static str, GeneratorKind);

pub const CARDS: [Card; 2] = [
    (
        Generator::FOUR_COLOR_GRADIENT,
        GeneratorKind::FourColorGradient,
    ),
    (Generator::SOLID_COLOR, GeneratorKind::SolidColor),
];

const CARD_WIDTH: f32 = 120.0;
const CARD_HEIGHT: f32 = 68.0;
const CARD_PIXELS: (u32, u32) = (240, 136);
const RGB48: usize = 6;

fn pattern_of(kind: GeneratorKind) -> Option<Pattern> {
    let g = made(kind, &GeneratorSettings::default()).ok()?;
    Some(Generated::of(&g, 0)?.at(0))
}

fn eight(word: &[u8; 2]) -> u8 {
    ((u32::from(u16::from_le_bytes(*word)) + 128) / 257) as u8
}

fn card_image(pattern: Pattern) -> ColorImage {
    let (w, h) = CARD_PIXELS;
    let stride = w as usize * RGB48;
    let mut deep = vec![0; stride * h as usize];
    draw(pattern, CARD_PIXELS, None, &mut deep, stride);
    let rgb: Vec<u8> = deep.as_chunks::<2>().0.iter().map(eight).collect();
    ColorImage::from_rgb([w as usize, h as usize], &rgb)
}

fn card_texture(ctx: &Context, kind: GeneratorKind) -> Option<TextureHandle> {
    let id = Id::unique(("generator card", kind));
    if let Some(kept) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(kept);
    }
    let image = card_image(pattern_of(kind)?);
    let made = ctx.load_texture("generator card", image, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, made.clone()));
    Some(made)
}

fn paint(ui: &egui::Ui, rect: Rect, kind: GeneratorKind) {
    let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    match card_texture(ui.ctx(), kind) {
        Some(t) => {
            ui.painter().image(t.id(), rect, whole, Color32::WHITE);
        }
        None => {
            ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
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
                            paint(ui, rect, kind);
                            if resp.hovered() {
                                ui.painter().rect_stroke(
                                    rect,
                                    2.0,
                                    Stroke::new(1.5, theme::colors().accent),
                                    StrokeKind::Inside,
                                );
                            }
                            ui.add(egui::Label::new(label).truncate());
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
