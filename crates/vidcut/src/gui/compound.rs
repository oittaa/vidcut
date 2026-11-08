use eframe::egui::{self, Key};
use serde_json::json;

use super::workspace::Workspace;

#[derive(Debug)]
pub(super) struct NewCompound {
    clips: Vec<String>,
    name: String,
    focus: bool,
}

impl NewCompound {
    pub(super) fn new(clips: Vec<String>) -> Self {
        Self {
            clips,
            name: String::new(),
            focus: true,
        }
    }
}

impl Workspace {
    pub(super) fn compound_window_tick(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.new_compound.take() else {
            return;
        };
        let mut open = true;
        let (mut create, mut cancel) = (false, false);
        super::dialog::window(ctx, "New Compound Clip", &mut open, |ui| {
            let mut enter = false;
            super::dialog::form(ui, "compound-form", |ui| {
                super::dialog::row(ui, "Name", |ui| {
                    let field = ui.text_edit_singleline(&mut d.name);
                    if d.focus {
                        field.request_focus();
                        d.focus = false;
                    }
                    enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                });
            });
            super::dialog::footer(ui, |ui| {
                create = super::dialog::main_button(ui, "Create", true) || enter;
                cancel = ui.button("Cancel").clicked();
            });
        });
        if create {
            self.send(
                "edit.compound",
                json!({ "clips": d.clips, "name": d.name.trim() }),
            );
        } else if open && !cancel {
            self.new_compound = Some(d);
        }
    }
}
