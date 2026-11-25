use eframe::egui;
use egui_phosphor::regular as icon;
use serde_json::{Value, json};
use vidcut_core::key_sets::{OWN, same};
use vidcut_core::shortcuts::Bindings;

use super::dialog;
use super::export::{Ask, Picked};
use super::workspace::Workspace;

enum Want {
    Use(String),
    Save(String, bool),
    Delete(String),
    Export(String),
    Import,
}

#[derive(Default)]
pub(super) struct KeySetsUi {
    sets: Vec<(String, bool)>,
    listed: bool,
    listing: Option<Ask>,
    doing: Option<Ask>,
    exporting: Option<(String, Picked)>,
    importing: Option<Picked>,
    naming: Option<String>,
    confirm_use: Option<String>,
    confirm_delete: bool,
    want: Option<Want>,
    error: Option<String>,
}

impl KeySetsUi {
    pub(super) fn close(&mut self) {
        self.listed = false;
        self.naming = None;
        self.confirm_use = None;
        self.confirm_delete = false;
        self.error = None;
    }

    fn mine(&self, name: &str) -> bool {
        self.sets
            .iter()
            .any(|(n, built_in)| !built_in && same(n, name))
    }
}

pub(super) fn bar(ui: &mut egui::Ui, s: &mut KeySetsUi, b: &Bindings) {
    let in_force = b.set_name().unwrap_or(OWN).to_string();
    let changed = b.any_custom();
    let mine = s.mine(&in_force);
    ui.horizontal(|ui| {
        let shown = match changed {
            true => format!("{in_force} (Changed)"),
            false => in_force.clone(),
        };
        egui::ComboBox::from_id_salt("key-set")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (name, _) in &s.sets {
                    let current = same(name, &in_force);
                    if !ui.selectable_label(current, name).clicked() || (current && !changed) {
                        continue;
                    }
                    match changed {
                        true => s.confirm_use = Some(name.clone()),
                        false => s.want = Some(Want::Use(name.clone())),
                    }
                }
            });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(icon::DOTS_THREE, |ui| {
                if ui
                    .add_enabled(mine && changed, egui::Button::new("Save"))
                    .clicked()
                {
                    s.want = Some(Want::Save(in_force.clone(), true));
                    ui.close();
                }
                if ui.button("Save As New Set…").clicked() {
                    s.naming = Some(String::new());
                    ui.close();
                }
                ui.menu_button("Export Set", |ui| {
                    for (name, _) in &s.sets {
                        if ui.button(name).clicked() {
                            s.want = Some(Want::Export(name.clone()));
                            ui.close();
                        }
                    }
                });
                if ui.button("Import Set…").clicked() {
                    s.want = Some(Want::Import);
                    ui.close();
                }
            });
            if ui
                .add_enabled(mine, egui::Button::new(icon::TRASH))
                .on_hover_text("Delete Set")
                .clicked()
            {
                s.confirm_delete = true;
            }
        });
    });
    naming_row(ui, s);
    if let Some(name) = s.confirm_use.clone() {
        dialog::warn(ui, format!("The changes to {in_force} are lost"));
        ui.horizontal(|ui| {
            if dialog::main_button(ui, "Discard Changes", true) {
                s.want = Some(Want::Use(name));
                s.confirm_use = None;
            }
            if ui.button("Cancel").clicked() {
                s.confirm_use = None;
            }
        });
    }
    if s.confirm_delete {
        ui.horizontal(|ui| {
            if dialog::main_button(ui, &format!("Delete {in_force}"), mine) {
                s.want = Some(Want::Delete(in_force.clone()));
                s.confirm_delete = false;
            }
            if ui.button("Cancel").clicked() {
                s.confirm_delete = false;
            }
        });
    }
    if let Some(e) = &s.error {
        dialog::warn(ui, e.clone());
    }
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
        if !field.has_focus() && name.is_empty() {
            field.request_focus();
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
            match answer {
                Ok(_) => {
                    s.error = None;
                    s.listed = false;
                }
                Err(e) => s.error = Some(e.message),
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
                }
            }
        }
        let Some(want) = s.want.take() else {
            return;
        };
        let (method, params) = match want {
            Want::Use(set) => ("keys.use", json!({ "set": set })),
            Want::Save(name, overwrite) => {
                ("keys.save", json!({ "name": name, "overwrite": overwrite }))
            }
            Want::Delete(name) => ("keys.delete", json!({ "name": name })),
            Want::Export(name) => {
                let file = format!("{}.toml", name.replace(['/', '\\'], "-"));
                let picked = Picked::ask(ctx, move || {
                    dialogs.save_file("Export Key Set", &folder, &file, &["toml"])
                });
                s.exporting = Some((name, picked));
                return;
            }
            Want::Import => {
                let picked = Picked::ask(ctx, move || {
                    dialogs.pick_file("Import Key Set", &folder, &["toml"])
                });
                s.importing = Some(picked);
                return;
            }
        };
        s.doing = Some(Ask::send(&live, ctx, method, params));
    }
}
