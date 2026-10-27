use std::collections::HashSet;

use eframe::egui::{self, RichText};
use vidcut_core::shortcuts::{Group, SHORTCUTS, Shortcut, key_name};

use super::export::near_center;
use super::theme;
use super::workspace::Workspace;

#[derive(Default)]
pub(super) struct HelpUi {
    pub open: bool,
    search: String,
    explained: HashSet<&'static str>,
}

impl Workspace {
    pub(super) fn help_window(&mut self, ctx: &egui::Context) {
        let help = &mut self.help;
        if !help.open {
            help.search.clear();
            return;
        }
        let mut open = true;
        let height = (ctx.content_rect().height() * 0.8).max(400.0);
        egui::Window::new("Keyboard Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([820.0, height])
            .default_pos(near_center(ctx, 820.0, height))
            .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::same(16)))
            .show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut help.search)
                        .hint_text("Find a key or an action")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(8.0);
                let query = help.search.trim().to_lowercase();
                let groups: Vec<(Group, Vec<&Shortcut>)> = Group::ALL
                    .into_iter()
                    .map(|g| {
                        let rows = SHORTCUTS
                            .iter()
                            .filter(|s| s.group == g && matches(s, &query))
                            .collect::<Vec<_>>();
                        (g, rows)
                    })
                    .filter(|(_, rows)| !rows.is_empty())
                    .collect();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin {
                                left: 6,
                                right: 14,
                                ..Default::default()
                            })
                            .show(ui, |ui| shortcuts(ui, &groups, &query, &mut help.explained));
                    });
            });
        help.open = open;
    }
}

const GAP: i8 = 14;

fn shortcuts(
    ui: &mut egui::Ui,
    groups: &[(Group, Vec<&Shortcut>)],
    query: &str,
    explained: &mut HashSet<&'static str>,
) {
    if groups.is_empty() {
        ui.label(
            RichText::new("Nothing matches. Try a key like J, or a word like trim.")
                .color(theme::colors().text_dim),
        );
        return;
    }
    if ui.available_width() < 640.0 {
        for (g, rows) in groups {
            group(ui, *g, rows, query, explained);
        }
        return;
    }
    let split = halves(groups);
    let middle = ui.available_rect_before_wrap().center().x;
    let top = ui.cursor().top();
    ui.columns(2, |columns| {
        let (left, right) = columns.split_at_mut(1);
        egui::Frame::new()
            .inner_margin(egui::Margin {
                right: GAP,
                ..Default::default()
            })
            .show(&mut left[0], |ui| {
                for (g, rows) in &groups[..split] {
                    group(ui, *g, rows, query, explained);
                }
            });
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: GAP,
                ..Default::default()
            })
            .show(&mut right[0], |ui| {
                for (g, rows) in &groups[split..] {
                    group(ui, *g, rows, query, explained);
                }
            });
    });
    ui.painter().vline(
        middle,
        top..=ui.cursor().top(),
        egui::Stroke::new(1.0, theme::colors().line),
    );
}

fn halves(groups: &[(Group, Vec<&Shortcut>)]) -> usize {
    let total: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
    (1..=groups.len())
        .min_by_key(|&i| {
            let left: usize = groups[..i].iter().map(|(_, rows)| rows.len()).sum();
            left.abs_diff(total - left)
        })
        .unwrap_or(1)
}

fn matches(s: &Shortcut, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let keys = s.keys.join("+").to_lowercase();
    let key = keys == query || keys.split('+').any(|k| k == query);
    key || query.chars().count() > 1
        && (s.action.to_lowercase().contains(query)
            || s.explanation
                .is_some_and(|e| e.to_lowercase().contains(query)))
}

fn group(
    ui: &mut egui::Ui,
    g: Group,
    rows: &[&Shortcut],
    query: &str,
    explained: &mut HashSet<&'static str>,
) {
    ui.label(
        RichText::new(g.title().to_uppercase())
            .small()
            .strong()
            .color(theme::colors().accent),
    );
    ui.add_space(4.0);
    for (i, s) in rows.iter().enumerate() {
        row(ui, s, query, explained, i % 2 == 1);
    }
    ui.add_space(16.0);
}

fn row(
    ui: &mut egui::Ui,
    s: &Shortcut,
    query: &str,
    explained: &mut HashSet<&'static str>,
    stripe: bool,
) {
    let open = s.explanation.is_some()
        && (explained.contains(s.action)
            || (!query.is_empty()
                && s.explanation
                    .is_some_and(|e| e.to_lowercase().contains(query))));
    let mut toggle = false;
    let behind = ui.painter().add(egui::Shape::Noop);
    let shown = ui.vertical(|ui| {
        ui.horizontal(|ui| {
            if s.explanation.is_some() {
                let r = mark(ui, open);
                let name = format!("What {} does", s.action);
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, &name));
                toggle |= r.clicked();
            } else {
                ui.add_space(MARK + ui.spacing().item_spacing.x);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                keys(ui, s.keys);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let name = egui::Label::new(s.action).wrap();
                    if s.explanation.is_some() {
                        toggle |= ui.add(name.sense(egui::Sense::click())).clicked();
                    } else {
                        ui.add(name);
                    }
                });
            });
        });
        if open && let Some(e) = s.explanation {
            ui.horizontal(|ui| {
                ui.add_space(MARK + ui.spacing().item_spacing.x);
                ui.add(
                    egui::Label::new(RichText::new(super::keys(e)).color(theme::colors().text_dim))
                        .wrap(),
                );
            });
        }
    });
    if toggle && !explained.remove(s.action) {
        explained.insert(s.action);
    }
    let rect = shown.response.rect.expand2(egui::vec2(6.0, 2.0));
    let c = theme::colors();
    let fill = match (ui.rect_contains_pointer(rect), stripe) {
        (true, _) => c.accent.gamma_multiply(0.15),
        (false, true) => c.faint,
        (false, false) => egui::Color32::TRANSPARENT,
    };
    ui.painter()
        .set(behind, egui::Shape::rect_filled(rect, 4.0, fill));
    ui.add_space(4.0);
}

const MARK: f32 = 18.0;

fn mark(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(MARK, MARK), egui::Sense::click());
    let c = theme::colors();
    let (fill, text) = match (open, r.hovered()) {
        (true, _) => (c.accent, c.panel),
        (false, true) => (c.accent.gamma_multiply(0.25), c.accent),
        (false, false) => (egui::Color32::TRANSPARENT, c.accent),
    };
    let p = ui.painter();
    p.circle(
        rect.center(),
        MARK / 2.0 - 1.0,
        fill,
        egui::Stroke::new(1.5, c.accent),
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "?",
        egui::FontId::proportional(12.0),
        text,
    );
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn keys(ui: &mut egui::Ui, keys: &[&str]) {
    let c = theme::colors();
    for (i, key) in keys.iter().rev().enumerate() {
        if i > 0 {
            ui.label(RichText::new("+").color(c.text_dim));
        }
        let name = match *key {
            "Left" => "←",
            "Right" => "→",
            "Up" => "↑",
            "Down" => "↓",
            k => key_name(k),
        };
        egui::Frame::new()
            .fill(c.field)
            .stroke(egui::Stroke::new(1.0, c.line))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(6, 1))
            .show(ui, |ui| ui.label(RichText::new(name).strong()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_alone_finds_its_shortcuts() {
        let found: Vec<&str> = SHORTCUTS
            .iter()
            .filter(|s| matches(s, "w"))
            .map(|s| s.action)
            .collect();
        assert_eq!(found, vec!["Dynamic Trim"]);
    }
}
