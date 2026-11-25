use eframe::egui;
use serde_json::json;
use vidcut_core::model::EffectType;

use super::dialog;
use super::effect_browser::GROUPS;
use super::workspace::Workspace;

#[derive(Default)]
pub(super) struct EffectsSearch {
    open: bool,
    text: String,
    at: usize,
    focus: bool,
}

fn found(text: &str) -> Vec<(&'static str, EffectType)> {
    let text = text.trim().to_lowercase();
    let mut found: Vec<(bool, (&'static str, EffectType))> = GROUPS
        .iter()
        .flat_map(|(group, cards)| cards.iter().map(move |card| (*group, *card)))
        .filter_map(|(group, (label, kind))| {
            let name = label.to_lowercase();
            let hit = name.contains(&text) || group.to_lowercase().contains(&text);
            hit.then_some((!name.starts_with(&text), (label, kind)))
        })
        .collect();
    found.sort_by_key(|(later, _)| *later);
    found.into_iter().map(|(_, card)| card).collect()
}

impl Workspace {
    pub(super) fn open_effects_search(&mut self) {
        let s = &mut self.effects_search;
        s.open = true;
        s.focus = true;
        s.at = 0;
    }

    pub(super) fn close_effects_search(&mut self) {
        self.effects_search.open = false;
    }

    pub(super) fn effects_search_open(&self) -> bool {
        self.effects_search.open
    }

    pub(super) fn effects_search_window(&mut self, ctx: &egui::Context) {
        if !self.effects_search.open {
            return;
        }
        let clip = self.pictured_selection().into_iter().next();
        let s = &mut self.effects_search;
        let mut open = true;
        let mut add = None;
        let mut changed = false;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let (up, down, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
            )
        });
        dialog::window(ctx, "Effects Search", &mut open, |ui| {
            dialog::form(ui, "effects-search-form", |ui| {
                dialog::row(ui, "Find", |ui| {
                    let output = egui::TextEdit::singleline(&mut s.text)
                        .desired_width(260.0)
                        .show(ui);
                    let edit = output.response;
                    if !edit.has_focus() {
                        edit.request_focus();
                    }
                    if s.focus {
                        let mut state = output.state;
                        let all = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(s.text.chars().count()),
                        );
                        state.cursor.set_char_range(Some(all));
                        state.store(ui.ctx(), edit.id);
                        s.focus = false;
                    }
                    if edit.changed() {
                        s.at = 0;
                        changed = true;
                    }
                });
            });
            let found = found(&s.text);
            let last = found.len().saturating_sub(1);
            if down {
                s.at = (s.at + 1).min(last);
            }
            if up {
                s.at = s.at.saturating_sub(1);
            }
            let room = ui.spacing();
            let text = ui.text_style_height(&egui::TextStyle::Body);
            let row = room.interact_size.y.max(text + 2.0 * room.button_padding.y);
            let rows = GROUPS.iter().map(|(_, cards)| cards.len()).sum::<usize>() as f32;
            let height = rows * row + (rows - 1.0) * room.item_spacing.y;
            egui::ScrollArea::vertical()
                .min_scrolled_height(height)
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (k, (label, kind)) in found.iter().enumerate() {
                        let row = ui.selectable_label(k == s.at, *label);
                        if k == s.at && (up || down || changed) {
                            row.scroll_to_me(None);
                        }
                        if row.clicked() {
                            s.at = k;
                            add = Some(*kind);
                        }
                    }
                });
            if clip.is_none() {
                dialog::warn(ui, "Select a clip with a picture");
            }
            let picked = found.get(s.at).map(|(_, kind)| *kind);
            dialog::footer(ui, |ui| {
                let ready = picked.is_some() && clip.is_some();
                if dialog::main_button(ui, "Add", ready) || enter {
                    add = add.or(picked);
                }
                cancel |= ui.button("Cancel").clicked();
            });
        });
        let add = add.filter(|_| clip.is_some());
        if add.is_some() || cancel || !open {
            self.effects_search.open = false;
        }
        if let (Some(kind), Some(clip)) = (add, clip) {
            self.send("effects.add", json!({ "clips": [clip], "kind": kind }));
        }
    }
}
