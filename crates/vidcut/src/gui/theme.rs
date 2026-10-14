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
/// A compound clip or nested timeline carried from another editor
/// (deviation X9).
pub const COMPOUND: Color32 = Color32::from_rgb(146, 92, 58);
pub const MARKED: Color32 = Color32::from_rgba_premultiplied(70, 70, 76, 70);
pub const ERROR: Color32 = Color32::from_rgb(235, 98, 88);
pub const OK: Color32 = Color32::from_rgb(120, 190, 120);

/// How Clip Color `name` looks (spec §3); none for a name it doesn't know.
pub fn clip_color(name: &str) -> Option<Color32> {
    vidcut_core::ops::CLIP_COLORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, [r, g, b])| Color32::from_rgb(*r, *g, *b))
}

/// A Clip Color's name as shown: `Teal`.
pub fn color_title(name: &str) -> String {
    let mut title = name.to_string();
    if let Some(first) = title.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    title
}

/// The color all of `colors` have, if they have the same one.
pub fn same_color<'a>(mut colors: impl Iterator<Item = Option<&'a str>>) -> Option<&'a str> {
    let first = colors.next()??;
    colors.all(|c| c == Some(first)).then_some(first)
}

/// The Clip Color submenu (spec §3): each color, the `current` one
/// selected, then Clear Color. Returns the one picked, empty for Clear
/// Color.
pub fn clip_color_menu(ui: &mut egui::Ui, current: Option<&str>) -> Option<String> {
    let mut picked = None;
    for (name, [r, g, b]) in vidcut_core::ops::CLIP_COLORS {
        let title = color_title(name);
        let clicked = ui
            .horizontal(|ui| {
                let (swatch, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    swatch,
                    CornerRadius::same(2),
                    Color32::from_rgb(*r, *g, *b),
                );
                ui.add(egui::Button::selectable(current == Some(*name), title))
                    .clicked()
            })
            .inner;
        if clicked {
            picked = Some(name.to_string());
        }
    }
    ui.separator();
    if ui.button("Clear Color").clicked() {
        picked = Some(String::new());
    }
    picked
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_of_one_color_show_it_in_the_menu() {
        assert_eq!(
            same_color([Some("teal"), Some("teal")].into_iter()),
            Some("teal")
        );
    }

    #[test]
    fn clips_of_mixed_colors_show_none_in_the_menu() {
        assert_eq!(same_color([Some("teal"), None].into_iter()), None);
    }
}
