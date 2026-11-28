use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;
use vidcut_core::loudness::{INTEGRATED_TOLERANCE, Reading, SHORT_LIMIT, TARGET};
use vidcut_core::model::LoudnessScale;

use super::theme;
use super::workspace::Workspace;

const STEEL: Color32 = Color32::from_rgb(70, 130, 180);

const LOW: Color32 = Color32::from_rgb(90, 150, 230);

const WITHIN: Color32 = Color32::from_rgb(220, 200, 60);

const HIGH: Color32 = Color32::from_rgb(225, 70, 60);

const BAR_W: f32 = 8.0;

const SUM_W: f32 = 14.0;

struct Scale {
    target: f64,
    low: f64,
    high: f64,
    absolute: bool,
}

impl Scale {
    fn new(target: f64, scale: LoudnessScale, absolute: bool) -> Self {
        let (low, high) = scale.span();
        Self {
            target,
            low,
            high,
            absolute,
        }
    }

    fn lu(&self, lufs: Option<f64>) -> Option<f64> {
        lufs.map(|l| l - self.target).filter(|lu| *lu >= self.low)
    }

    fn place(&self, lufs: Option<f64>) -> f32 {
        self.lu(lufs).map_or(0.0, |lu| {
            ((lu - self.low) / (self.high - self.low)).clamp(0.0, 1.0) as f32
        })
    }

    fn text(&self, lufs: Option<f64>) -> String {
        match self.lu(lufs) {
            None => "--".into(),
            Some(lu) if self.absolute => format!("{:.1}", lu + self.target),
            Some(lu) if lu.abs() < 0.05 => "0.0".into(),
            Some(lu) => format!("{lu:+.1}"),
        }
    }

    fn unit(&self) -> &'static str {
        if self.absolute { "LUFS" } else { "LU" }
    }
}

fn integrated_color(lu: Option<f64>) -> Color32 {
    match lu {
        None => theme::colors().text,
        Some(x) if x < -INTEGRATED_TOLERANCE => LOW,
        Some(x) if x > INTEGRATED_TOLERANCE => HIGH,
        Some(_) => WITHIN,
    }
}

fn short_color(lu: Option<f64>) -> Color32 {
    match lu {
        None => theme::colors().text,
        Some(x) if x > SHORT_LIMIT => HIGH,
        Some(_) => LOW,
    }
}

fn bar(painter: &egui::Painter, rect: Rect, place: f32, zero: f32) {
    painter.rect_filled(rect, CornerRadius::same(2), theme::colors().panel_dark);
    let y = |p: f32| rect.bottom() - p * rect.height();
    if place > 0.0 {
        let top = y(place);
        let under = Rect::from_x_y_ranges(rect.x_range(), top.max(y(zero))..=rect.bottom());
        painter.rect_filled(under, CornerRadius::ZERO, STEEL);
        if place > zero {
            let over = Rect::from_x_y_ranges(rect.x_range(), top..=y(zero));
            painter.rect_filled(over, CornerRadius::ZERO, WITHIN);
        }
    }
    painter.line_segment(
        [pos2(rect.left(), y(zero)), pos2(rect.right(), y(zero))],
        Stroke::new(1.0, theme::colors().text_dim),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, Color32::from_white_alpha(25)),
        StrokeKind::Inside,
    );
}

fn meter_bar(ui: &mut egui::Ui, size: egui::Vec2, place: f32, zero: f32, label: &str) {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, label));
    bar(ui.painter(), rect, place, zero);
}

fn readout(ui: &mut egui::Ui, name: &str, value: String, color: Color32) {
    ui.label(RichText::new(name).small().color(theme::colors().text_dim));
    ui.label(RichText::new(value).small().monospace().color(color));
    ui.end_row();
}

fn button_text(r: &Reading) -> &'static str {
    match (r.running, r.momentary_max.is_some()) {
        (true, _) => "Pause",
        (false, true) => "Resume",
        (false, false) => "Start",
    }
}

impl Workspace {
    pub(super) fn loudness_meter(&mut self, ui: &mut egui::Ui, bars: f32) {
        let reading = (self.snap.loudness.lock())
            .unwrap_or_else(|e| e.into_inner())
            .reading();
        let (target, scale) = self.project().map_or((TARGET, LoudnessScale::Ebu9), |p| {
            (p.settings.target_loudness, p.settings.loudness_scale)
        });
        let absolute = self.snap.loudness_absolute;
        let scale = Scale::new(target, scale, absolute);
        let text = |ui: &egui::Ui| ui.text_style_height(&egui::TextStyle::Small);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new("Loudness").strong()).extend());
                ui.label(
                    RichText::new(scale.unit())
                        .small()
                        .color(theme::colors().text_dim),
                );
                ui.menu_button(egui_phosphor::regular::DOTS_THREE, |ui| {
                    let mut on = absolute;
                    if ui.checkbox(&mut on, "Absolute Scale").changed() {
                        self.send("loudness.set", json!({ "absolute": on }));
                        ui.close();
                    }
                });
            });
            let zero = scale.place(Some(target));
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.vertical(|ui| {
                    ui.label(RichText::new("M").small());
                    let tall = (bars - text(ui)).max(1.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for level in &reading.channels {
                            let size = vec2(BAR_W, tall);
                            meter_bar(ui, size, scale.place(*level), zero, "momentary loudness");
                        }
                    });
                });
                ui.vertical(|ui| {
                    let max = reading.momentary_max;
                    ui.label(RichText::new(scale.text(max)).small().monospace());
                    let tall = (bars - text(ui)).max(1.0);
                    let place = scale.place(reading.momentary);
                    meter_bar(ui, vec2(SUM_W, tall), place, zero, "momentary loudness sum");
                });
                ui.add_space(8.0);
                egui::Grid::new("loudness readouts")
                    .num_columns(2)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        let short = |l| short_color(scale.lu(l));
                        readout(ui, "Short", scale.text(reading.short), short(reading.short));
                        let max = reading.short_max;
                        readout(ui, "Short Max", scale.text(max), short(max));
                        let range = format!("{:.1}", reading.range);
                        readout(ui, "Range", range, theme::colors().text);
                        let i = reading.integrated;
                        let color = integrated_color(scale.lu(i));
                        readout(ui, "Integrated", scale.text(i), color);
                        let peak = reading
                            .true_peak
                            .map_or("--".into(), |p| format!("{p:.1} dBTP"));
                        readout(ui, "True Peak", peak, theme::colors().text);
                    });
            });
            ui.horizontal(|ui| {
                if ui.button(button_text(&reading)).clicked() {
                    let running = !reading.running;
                    self.send("loudness.set", json!({ "running": running }));
                }
                if ui.button("Reset").clicked() {
                    self.send("loudness.reset", json!({}));
                }
            });
        });
    }
}
