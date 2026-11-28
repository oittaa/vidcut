use eframe::egui;
use egui_phosphor::regular::CHECK;
use serde_json::json;
use vidcut_core::model::Project;
use vidcut_core::ops::color_groups::proposed_name;

use super::dialog;
use super::workspace::Workspace;

struct Naming {
    group: Option<String>,
    clips: Vec<String>,
    text: String,
    focus: bool,
}

#[derive(Default)]
pub(super) struct ColorGroups {
    seen: Option<String>,
    current: Option<String>,
    naming: Option<Naming>,
    deleting: Option<String>,
}

pub(super) enum GroupAct {
    Collapse,
    AddCurrent,
    AddNew,
    Remove,
    Assign(String),
    Load(String),
    Rename(String),
    Delete(String),
}

fn ticked(on: bool, text: &str) -> String {
    format!("{}  {text}", if on { CHECK } else { "   " })
}

pub(super) fn group_items(
    ui: &mut egui::Ui,
    p: &Project,
    (clip, current): (&str, Option<&str>),
    act: &mut Option<GroupAct>,
) {
    let grouped = (p.clip(clip).and_then(|c| c.color_group.as_deref()))
        .is_some_and(|g| p.color_group(g).is_some());
    let mut pick = |ui: &mut egui::Ui, on: bool, text: &str, a: GroupAct| {
        if ui.add_enabled(on, egui::Button::new(text)).clicked() {
            *act = Some(a);
            ui.close();
        }
    };
    pick(ui, grouped, "Collapse Group Grades", GroupAct::Collapse);
    pick(
        ui,
        current.is_some(),
        "Add into Current Group",
        GroupAct::AddCurrent,
    );
    pick(ui, true, "Add into a New Group", GroupAct::AddNew);
    pick(ui, grouped, "Remove from Group", GroupAct::Remove);
    ui.add_enabled_ui(!p.color_groups.is_empty(), |ui| {
        ui.menu_button("Groups", |ui| {
            for g in &p.color_groups {
                let on = current == Some(g.id.as_str());
                ui.menu_button(ticked(on, &g.name), |ui| {
                    let id = || g.id.clone();
                    pick(ui, true, "Assign to Group", GroupAct::Assign(id()));
                    pick(ui, true, "Load", GroupAct::Load(id()));
                    pick(ui, true, "Rename", GroupAct::Rename(id()));
                    pick(ui, true, "Delete", GroupAct::Delete(id()));
                });
            }
        });
    });
}

impl Workspace {
    pub(super) fn current_color_group(&self) -> Option<String> {
        let p = self.project()?;
        let id = self.color_groups.current.as_deref()?;
        p.color_group(id).map(|g| g.id.clone())
    }

    pub(super) fn keep_color_group(&mut self) {
        let clip = self.color_clip();
        if clip == self.color_groups.seen {
            return;
        }
        self.color_groups.seen = clip;
        if let Some(id) = self.color_group() {
            self.color_groups.current = Some(id);
        }
    }

    pub(super) fn set_color_group(&mut self, id: String) -> Result<(), String> {
        let known = self.project().is_some_and(|p| p.color_group(&id).is_some());
        if !known {
            return Err(format!("No color group {id}"));
        }
        self.color_groups.current = Some(id);
        Ok(())
    }

    pub(super) fn group_act(&mut self, act: GroupAct, (clip, clips): (String, Vec<String>)) {
        let Some(p) = self.project_shared() else {
            return;
        };
        match act {
            GroupAct::Collapse => self.send("color_group.collapse", json!({ "clips": [clip] })),
            GroupAct::Remove => self.send("color_group.unassign", json!({ "clips": [clip] })),
            GroupAct::AddCurrent => {
                if let Some(group) = self.current_color_group() {
                    self.send(
                        "color_group.assign",
                        json!({ "group": group, "clips": clips }),
                    );
                }
            }
            GroupAct::Assign(group) => {
                self.send(
                    "color_group.assign",
                    json!({ "group": group, "clips": clips }),
                );
                self.color_groups.current = Some(group);
            }
            GroupAct::AddNew => {
                self.color_groups.naming = Some(Naming {
                    group: None,
                    clips,
                    text: proposed_name(&p),
                    focus: true,
                });
            }
            GroupAct::Rename(group) => {
                let Some(g) = p.color_group(&group) else {
                    return;
                };
                self.color_groups.naming = Some(Naming {
                    text: g.name.clone(),
                    group: Some(group),
                    clips: Vec::new(),
                    focus: true,
                });
            }
            GroupAct::Load(group) => self.color_groups.current = Some(group),
            GroupAct::Delete(group) => self.color_groups.deleting = Some(group),
        }
    }

    pub(super) fn color_group_windows(&mut self, ctx: &egui::Context) {
        self.group_name_window(ctx);
        self.delete_group_window(ctx);
    }

    fn group_name_window(&mut self, ctx: &egui::Context) {
        let Some(n) = &mut self.color_groups.naming else {
            return;
        };
        let (mut open, mut ok) = (true, false);
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        dialog::window(ctx, "Group Name", &mut open, |ui| {
            let mut enter = false;
            dialog::form(ui, "group-name-form", |ui| {
                dialog::row(ui, "Name", |ui| {
                    let output = egui::TextEdit::singleline(&mut n.text)
                        .desired_width(260.0)
                        .show(ui);
                    let edit = output.response;
                    if n.focus {
                        edit.request_focus();
                        let mut state = output.state;
                        let all = egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(n.text.chars().count()),
                        );
                        state.cursor.set_char_range(Some(all));
                        state.store(ui.ctx(), edit.id);
                        n.focus = false;
                    }
                    enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
            });
            let named = !n.text.trim().is_empty();
            dialog::footer(ui, |ui| {
                ok = dialog::main_button(ui, "OK", named) || (enter && named);
                cancel |= ui.button("Cancel").clicked();
            });
        });
        if !(ok || cancel || !open) {
            return;
        }
        let Some(n) = self.color_groups.naming.take() else {
            return;
        };
        if !ok {
            return;
        }
        let name = n.text.trim();
        match n.group {
            Some(group) => {
                self.send(
                    "color_group.rename",
                    json!({ "group": group, "name": name }),
                );
            }
            None => {
                let next = self
                    .project()
                    .map(|p| format!("cg{}", p.next_id.color_group));
                self.send("color_group.add", json!({ "clips": n.clips, "name": name }));
                self.color_groups.current = next;
            }
        }
    }

    fn delete_group_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.color_groups.deleting.take() else {
            return;
        };
        let Some(name) = (self.project()).and_then(|p| Some(p.color_group(&id)?.name.clone()))
        else {
            return;
        };
        let (mut open, mut delete, mut cancel) = (true, false, false);
        dialog::window(ctx, "Delete Clip Group?", &mut open, |ui| {
            ui.label(format!("Delete {name}?"));
            dialog::footer(ui, |ui| {
                delete = dialog::main_button(ui, "Delete", true);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if delete {
            self.send("color_group.remove", json!({ "group": id }));
        } else if open && !cancel {
            self.color_groups.deleting = Some(id);
        }
    }
}
