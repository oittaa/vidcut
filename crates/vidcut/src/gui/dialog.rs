use eframe::egui::{self, RichText};

use super::export::near_center;
use super::theme;

pub(super) const WIDTH: f32 = 420.0;
const LABEL: f32 = 100.0;

pub(super) fn window<R>(
    ctx: &egui::Context,
    title: &str,
    open: &mut bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    window_of(ctx, title, open, WIDTH, add)
}

pub(super) fn window_of<R>(
    ctx: &egui::Context,
    title: &str,
    open: &mut bool,
    width: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .default_pos(near_center(ctx, width, 400.0))
        .open(open)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            add(ui)
        })
        .and_then(|r| r.inner)
}

pub(super) fn tool_window<R>(
    ctx: &egui::Context,
    title: &str,
    open: &mut bool,
    size: [f32; 2],
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_pos(near_center(ctx, size[0], size[1]))
        .default_size(size)
        .open(open)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            add(ui)
        })
        .and_then(|r| r.inner)
}

pub(super) fn form(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(LABEL)
        .spacing([12.0, 8.0])
        .show(ui, add);
}

pub(super) fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.label(RichText::new(label).color(theme::colors().text_dim));
    let r = ui.horizontal(add).inner;
    ui.end_row();
    r
}

pub(super) fn switch<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::colors().line))
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(add).inner
        })
        .inner
}

pub(super) fn advanced(ui: &mut egui::Ui, id: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new("Advanced")
        .id_salt(id)
        .default_open(open)
        .show_unindented(ui, add);
}

pub(super) fn note(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).color(theme::colors().text_dim)).wrap())
}

pub(super) fn warn(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    let color = ui.visuals().warn_fg_color;
    ui.add(egui::Label::new(RichText::new(format!("⚠ {}", text.into())).color(color)).wrap())
}

pub(super) fn why(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).small().color(theme::colors().text_dim));
}

pub(super) fn scroll_room(ui: &egui::Ui) -> f32 {
    let s = &ui.spacing().scroll;
    s.bar_inner_margin + s.bar_width + s.bar_outer_margin
}

pub(super) fn footer(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.separator();
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), add);
}

pub(super) fn main_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    let c = theme::colors();
    let button = egui::Button::new(RichText::new(label).color(c.selected_text)).fill(c.selection);
    ui.add_enabled(enabled, button).clicked()
}
