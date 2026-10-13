//! A dark theme (spec §2: dark gray, with a red playhead and colored clips).

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};

pub const BG: Color32 = Color32::from_rgb(28, 28, 30);
pub const PANEL: Color32 = Color32::from_rgb(38, 38, 41);
pub const PANEL_DARK: Color32 = Color32::from_rgb(24, 24, 26);
pub const TRACK: Color32 = Color32::from_rgb(20, 20, 22);
/// The target track's row and header: a little lighter and warmer.
pub const TRACK_TARGET: Color32 = Color32::from_rgb(38, 35, 31);
pub const PANEL_TARGET: Color32 = Color32::from_rgb(60, 53, 44);
pub const TEXT: Color32 = Color32::from_rgb(205, 205, 210);
pub const TEXT_DIM: Color32 = Color32::from_rgb(130, 130, 138);
pub const ACCENT: Color32 = Color32::from_rgb(232, 150, 52);
pub const PLAYHEAD: Color32 = Color32::from_rgb(231, 72, 58);
pub const CLIP: Color32 = Color32::from_rgb(58, 96, 150);
pub const CLIP_DISABLED: Color32 = Color32::from_rgb(52, 58, 68);
/// A clip whose file is missing, and the viewer's Media Offline frame.
pub const OFFLINE: Color32 = Color32::from_rgb(150, 46, 42);
pub const AUDIO: Color32 = Color32::from_rgb(52, 120, 84);
/// A generator or title carried from another editor (deviation X9).
pub const GENERATOR: Color32 = Color32::from_rgb(112, 84, 156);
pub const MARKED: Color32 = Color32::from_rgba_premultiplied(70, 70, 76, 70);
pub const ERROR: Color32 = Color32::from_rgb(235, 98, 88);
pub const OK: Color32 = Color32::from_rgb(120, 190, 120);

pub fn apply(ctx: &egui::Context) {
    // Phosphor's icons, mixed into ordinary text.
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = PANEL_DARK;
    v.faint_bg_color = Color32::from_rgb(44, 44, 48);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(150, 92, 32);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(52, 52, 56));
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(3);
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(52, 52, 57);
    v.widgets.inactive.bg_fill = Color32::from_rgb(52, 52, 57);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(66, 66, 72);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(80, 80, 88);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}
