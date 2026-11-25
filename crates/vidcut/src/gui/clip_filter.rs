use eframe::egui::{self, Color32, CornerRadius};
use vidcut_core::model::ClipFilter;
use vidcut_core::ops;

use super::theme;
use super::timeline::paint;

pub(super) fn menu(ui: &mut egui::Ui, now: &ClipFilter) -> Option<ClipFilter> {
    let mut picked = None;
    let p = &mut picked;
    item(ui, now, ClipFilter::All, "All Clips", p);
    absent(ui, "Selected Clips");
    ui.separator();
    item(ui, now, ClipFilter::Graded, "Graded Clips", p);
    item(
        ui,
        now,
        ClipFilter::Composition,
        "Clips with Composition",
        p,
    );
    item(ui, now, ClipFilter::Ungraded, "Ungraded Clips", p);
    absent_menu(ui, "Modified Clips");
    absent(ui, "Unrendered Clips");
    ui.separator();
    let clip: Vec<_> = (ops::CLIP_COLORS.iter())
        .map(|(name, [r, g, b])| (*name, Color32::from_rgb(*r, *g, *b)))
        .collect();
    ui.menu_button("Clip Color", |ui| {
        let none = (ClipFilter::NoClipColor, "No Clip Color");
        let of = |color| ClipFilter::ClipColor { color };
        colors(ui, now, p, (none, "Any Clip Color"), &clip, of);
    });
    absent_menu(ui, "Flagged Clips");
    let marker: Vec<_> = (ops::MARKER_COLORS.iter())
        .map(|name| (*name, paint::marker_color(name)))
        .collect();
    ui.menu_button("Marked Clips", |ui| {
        let none = (ClipFilter::NoMarker, "No Marker");
        let of = |color| ClipFilter::Marked { color };
        colors(ui, now, p, (none, "Any Marker"), &marker, of);
    });
    absent_menu(ui, "Node Color");
    ui.separator();
    for name in ["Keywords", "People", "Stereoscopic", "Grouped"] {
        absent_menu(ui, name);
    }
    for name in ["Tracked", "HDR Graded", "Magic Mask", "Noise Reduction"] {
        absent(ui, name);
    }
    item(ui, now, ClipFilter::Effects, "Video Effects", p);
    item(ui, now, ClipFilter::OtherRate, "Different Frame Rate", p);
    ui.separator();
    item(ui, now, ClipFilter::Sizing, "Edit Sizing", p);
    for name in [
        "Input Sizing",
        "Common Media Pool Source",
        "Clips in Bin",
        "Matte Nodes",
        "Mattes Available",
    ] {
        absent(ui, name);
    }
    ui.separator();
    absent(ui, "Create Smart Filter…");
    picked
}

pub(super) fn underline(ui: &egui::Ui, r: &egui::Response) {
    let stroke = egui::Stroke::new(2.0, theme::colors().error);
    let y = r.rect.bottom() - 1.0;
    ui.painter().hline(r.rect.x_range(), y, stroke);
}

fn item(
    ui: &mut egui::Ui,
    now: &ClipFilter,
    f: ClipFilter,
    name: &str,
    picked: &mut Option<ClipFilter>,
) {
    if ui.radio(*now == f, name).clicked() {
        *picked = Some(f);
        ui.close();
    }
}

fn absent(ui: &mut egui::Ui, name: &str) {
    ui.add_enabled(false, egui::Button::new(name));
}

fn absent_menu(ui: &mut egui::Ui, name: &str) {
    ui.add_enabled_ui(false, |ui| ui.menu_button(name, |_| {}));
}

fn colors(
    ui: &mut egui::Ui,
    now: &ClipFilter,
    picked: &mut Option<ClipFilter>,
    ((none, no), any): ((ClipFilter, &str), &str),
    list: &[(&str, Color32)],
    of: impl Fn(Option<String>) -> ClipFilter,
) {
    item(ui, now, none, no, picked);
    item(ui, now, of(None), any, picked);
    for (name, color) in list {
        let f = of(Some(name.to_string()));
        let on = *now == f;
        let clicked = ui
            .horizontal(|ui| {
                let size = egui::vec2(12.0, 12.0);
                let (swatch, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter()
                    .rect_filled(swatch, CornerRadius::same(2), *color);
                let title = theme::color_title(name);
                ui.add(egui::Button::selectable(on, title)).clicked()
            })
            .inner;
        if clicked {
            *picked = Some(f);
            ui.close();
        }
    }
}
