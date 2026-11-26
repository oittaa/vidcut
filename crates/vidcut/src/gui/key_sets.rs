use std::collections::VecDeque;

use eframe::egui;
use egui_phosphor::regular as icon;
use serde_json::{Value, json};
use vidcut_core::key_sets::{self, OWN, same};
use vidcut_core::shortcuts::{Bindings, Custom};

use super::dialog;
use super::export::{Ask, Picked};
use super::workspace::Workspace;

const DRAFT_STEPS: usize = 100;

enum Want {
    Use(String),
    Save(String, bool),
    Delete(String),
    Export(String),
    Import,
}

enum Sent {
    InForce,
    Delete(String),
    Export,
}

#[derive(Default)]
pub(super) struct KeySetsUi {
    sets: Vec<(String, bool)>,
    listed: bool,
    listing: Option<Ask>,
    doing: Option<Ask>,
    sent: Option<Sent>,
    exporting: Option<(String, Picked)>,
    importing: Option<Picked>,
    naming: Option<String>,
    focus_name: bool,
    focus_cancel: bool,
    confirm_use: Option<String>,
    confirm_delete: Option<String>,
    confirm_close: bool,
    want: Option<Want>,
    error: Option<String>,
    draft: Option<Bindings>,
    back: VecDeque<Option<Bindings>>,
    forward: Vec<Option<Bindings>>,
}

impl KeySetsUi {
    pub(super) fn close(&mut self) {
        self.listed = false;
        self.naming = None;
        self.confirm_use = None;
        self.confirm_delete = None;
        self.confirm_close = false;
        self.error = None;
        self.discard();
    }

    fn mine(&self, name: &str) -> bool {
        self.sets
            .iter()
            .any(|(n, built_in)| !built_in && same(n, name))
    }

    pub(super) fn shown<'a>(&'a self, in_force: &'a Bindings) -> &'a Bindings {
        self.draft.as_ref().unwrap_or(in_force)
    }

    pub(super) fn drafting(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn change(&mut self, in_force: &Bindings, custom: vidcut_core::Result<Custom>) {
        match custom.and_then(|c| self.shown(in_force).with(&c)) {
            Ok(next) => self.put(in_force, next),
            Err(e) => self.error = Some(e.message),
        }
    }

    fn put(&mut self, in_force: &Bindings, next: Bindings) {
        let next = (next != *in_force).then_some(next);
        if next == self.draft {
            return;
        }
        if self.back.len() == DRAFT_STEPS {
            self.back.pop_front();
        }
        self.back
            .push_back(std::mem::replace(&mut self.draft, next));
        self.forward.clear();
        self.error = None;
    }

    fn pick(&mut self, in_force: &Bindings, name: &str) {
        let picked = key_sets::find(&key_sets::dir(), name).and_then(|set| {
            let named = (!same(&set.name, OWN)).then(|| set.name.clone());
            Bindings::in_set(named, &set.keys, &set.keys)
        });
        match picked {
            Ok(next) => self.put(in_force, next),
            Err(e) => self.error = Some(e.message),
        }
    }

    pub(super) fn undo(&mut self) {
        if let Some(before) = self.back.pop_back() {
            let now = std::mem::replace(&mut self.draft, before);
            self.forward.push(now);
        }
    }

    pub(super) fn redo(&mut self) {
        if let Some(after) = self.forward.pop() {
            let now = std::mem::replace(&mut self.draft, after);
            self.back.push_back(now);
        }
    }

    fn discard(&mut self) {
        self.draft = None;
        self.back.clear();
        self.forward.clear();
    }

    pub(super) fn save(&mut self, in_force: &Bindings) {
        let shown = self.shown(in_force);
        let name = shown.set_name().unwrap_or(OWN).to_string();
        match (shown.any_custom(), self.mine(&name)) {
            (false, _) => self.want = Some(Want::Use(name)),
            (true, true) => self.want = Some(Want::Save(name, true)),
            (true, false) => {
                self.naming = Some(name);
                self.focus_name = true;
            }
        }
    }

    pub(super) fn may_close(&mut self) -> bool {
        if self.draft.is_none() {
            return true;
        }
        self.confirm_close = true;
        self.focus_cancel = true;
        false
    }
}

pub(super) fn bar(ui: &mut egui::Ui, s: &mut KeySetsUi, in_force: &Bindings) {
    let b = s.shown(in_force).clone();
    let name = b.set_name().unwrap_or(OWN).to_string();
    let changed = b.any_custom();
    let mine = s.mine(&name);
    let any_mine = s.sets.iter().any(|(_, built_in)| !built_in);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(icon::DOTS_THREE, |ui| {
                if ui.button("Save As New Set…").clicked() {
                    s.naming = Some(name.clone());
                    s.focus_name = true;
                    ui.close();
                }
                if ui.button("Import Set…").clicked() {
                    s.want = Some(Want::Import);
                    ui.close();
                }
                ui.menu_button("Export Set", |ui| {
                    for (set, _) in &s.sets {
                        if ui.button(set).clicked() {
                            s.want = Some(Want::Export(set.clone()));
                            ui.close();
                        }
                    }
                });
                ui.add_enabled_ui(any_mine, |ui| {
                    ui.menu_button("Delete Set", |ui| {
                        for (set, built_in) in &s.sets {
                            if !built_in && ui.button(set).clicked() {
                                s.confirm_delete = Some(set.clone());
                                s.focus_cancel = true;
                                ui.close();
                            }
                        }
                    });
                });
            });
            let shown = match changed {
                true => format!("{name}*"),
                false => name.clone(),
            };
            let sets = s.sets.clone();
            egui::ComboBox::from_id_salt("key-set")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (set, _) in &sets {
                        let current = same(set, &name);
                        if !ui.selectable_label(current, set).clicked() || (current && !changed) {
                            continue;
                        }
                        match changed {
                            true => {
                                s.confirm_use = Some(set.clone());
                                s.focus_cancel = true;
                            }
                            false => s.pick(in_force, set),
                        }
                    }
                });
            if ui
                .add_enabled(mine, egui::Button::new(icon::TRASH))
                .on_hover_text("Delete Set")
                .clicked()
            {
                s.confirm_delete = Some(name.clone());
                s.focus_cancel = true;
            }
        })
    });
    naming_row(ui, s);
    if let Some(set) = s.confirm_use.clone()
        && let Some(discard) = ask(ui, &mut s.focus_cancel, "Discard Changes?", "Discard")
    {
        if discard {
            s.pick(in_force, &set);
        }
        s.confirm_use = None;
    }
    if let Some(set) = s.confirm_delete.clone()
        && let Some(delete) = ask(
            ui,
            &mut s.focus_cancel,
            &format!("Delete \"{set}\"?"),
            "Delete",
        )
    {
        if delete {
            s.want = Some(Want::Delete(set));
        }
        s.confirm_delete = None;
    }
    if let Some(e) = &s.error {
        dialog::warn(ui, e.clone());
    }
}

pub(super) fn close_row(ui: &mut egui::Ui, s: &mut KeySetsUi) -> bool {
    if !s.confirm_close {
        return false;
    }
    let Some(discard) = ask(ui, &mut s.focus_cancel, "Discard Changes?", "Discard") else {
        return false;
    };
    s.confirm_close = false;
    if discard {
        s.discard();
    }
    discard
}

fn ask(ui: &mut egui::Ui, focus: &mut bool, question: &str, main: &str) -> Option<bool> {
    let mut answer = None;
    ui.horizontal(|ui| {
        ui.label(question);
        if dialog::main_button(ui, main, true) {
            answer = Some(true);
        }
        let cancel = ui.button("Cancel");
        if std::mem::take(focus) {
            cancel.request_focus();
        }
        if cancel.clicked() {
            answer = Some(false);
        }
    });
    answer
}

fn naming_row(ui: &mut egui::Ui, s: &mut KeySetsUi) {
    let Some(mut name) = s.naming.take() else {
        return;
    };
    let taken = s.sets.iter().find(|(n, _)| same(n, &name)).cloned();
    let built_in = taken.as_ref().is_some_and(|(_, b)| *b);
    let ok = !name.trim().is_empty() && !built_in;
    let mut keep = true;
    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("Set Name")
                .desired_width(200.0),
        );
        if s.focus_name {
            field.request_focus();
            s.focus_name = false;
        }
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let label = match taken.is_some() {
            true => "Replace",
            false => "Save",
        };
        if dialog::main_button(ui, label, ok) || (ok && entered) {
            s.want = Some(Want::Save(name.trim().to_string(), taken.is_some()));
            keep = false;
        }
        if ui.button("Cancel").clicked() {
            keep = false;
        }
    });
    if built_in {
        dialog::warn(ui, "A built-in set has this name");
    }
    if keep {
        s.naming = Some(name);
    }
}

fn names(result: &Value) -> Vec<(String, bool)> {
    result["sets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = s["name"].as_str()?.to_string();
            Some((name, s["built_in"].as_bool().unwrap_or(false)))
        })
        .collect()
}

impl Workspace {
    pub(super) fn key_sets(&mut self, ctx: &egui::Context) {
        let folder = self.export_folder();
        let dialogs = self.dialogs.clone();
        let live = self.live.clone();
        let in_force = self.snap.bindings.clone();
        let s = &mut self.preferences.key_sets;
        if let Some(answer) = s.listing.as_ref().and_then(Ask::take) {
            s.listing = None;
            match answer {
                Ok(r) => s.sets = names(&r.result),
                Err(e) => s.error = Some(e.message),
            }
        }
        if let Some(answer) = s.doing.as_ref().and_then(Ask::take) {
            s.doing = None;
            match (answer, s.sent.take()) {
                (Err(e), _) => s.error = Some(e.message),
                (Ok(_), sent) => {
                    s.error = None;
                    s.listed = false;
                    let drafted = s.draft.as_ref().and_then(Bindings::set_name);
                    match sent {
                        Some(Sent::InForce) => s.discard(),
                        Some(Sent::Delete(set)) if drafted.is_some_and(|d| same(d, &set)) => {
                            s.discard()
                        }
                        _ => {}
                    }
                }
            }
        }
        if !s.listed && s.listing.is_none() {
            s.listed = true;
            s.listing = Some(Ask::send(&live, ctx, "keys.sets", json!({})));
        }
        if let Some((name, picked)) = s.exporting.take() {
            match picked.take() {
                None => s.exporting = Some((name, picked)),
                Some(None) => {}
                Some(Some(path)) => {
                    let params = json!({ "set": name, "path": path });
                    s.doing = Some(Ask::send(&live, ctx, "keys.export", params));
                    s.sent = Some(Sent::Export);
                }
            }
        }
        if let Some(picked) = s.importing.take() {
            match picked.take() {
                None => s.importing = Some(picked),
                Some(None) => {}
                Some(Some(path)) => {
                    let params = json!({ "path": path });
                    s.doing = Some(Ask::send(&live, ctx, "keys.import", params));
                    s.sent = Some(Sent::InForce);
                }
            }
        }
        let Some(want) = s.want.take() else {
            return;
        };
        let (method, params, sent) = match want {
            Want::Use(set) => ("keys.use", json!({ "set": set }), Sent::InForce),
            Want::Save(name, overwrite) => {
                let shown = s.shown(&in_force);
                let keys = shown.changes();
                let from = shown.set_name().unwrap_or(OWN);
                let params =
                    json!({ "name": name, "overwrite": overwrite, "keys": keys, "from": from });
                ("keys.save", params, Sent::InForce)
            }
            Want::Delete(name) => ("keys.delete", json!({ "name": name }), Sent::Delete(name)),
            Want::Export(name) => {
                let file = format!("{} Keys.txt", name.replace(['/', '\\'], "-"));
                let picked = Picked::ask(ctx, move || {
                    dialogs.save_file("Export Key Set", &folder, &file, &["txt"])
                });
                s.exporting = Some((name, picked));
                return;
            }
            Want::Import => {
                let picked = Picked::ask(ctx, move || {
                    dialogs.pick_file("Import Key Set", &folder, &["txt"])
                });
                s.importing = Some(picked);
                return;
            }
        };
        s.doing = Some(Ask::send(&live, ctx, method, params));
        s.sent = Some(sent);
    }
}
