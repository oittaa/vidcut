use eframe::egui::{self, Key};
use serde_json::json;

use super::workspace::Workspace;

#[derive(Debug)]
enum Making {
    Clips(Vec<String>),
    Marks,
    Pool,
}

#[derive(Debug)]
pub(super) struct NewCompound {
    making: Making,
    name: String,
    focus: bool,
}

impl NewCompound {
    fn of(making: Making) -> Self {
        Self {
            making,
            name: String::new(),
            focus: true,
        }
    }

    pub(super) fn new(clips: Vec<String>) -> Self {
        Self::of(Making::Clips(clips))
    }

    pub(super) fn of_marks() -> Self {
        Self::of(Making::Marks)
    }

    pub(super) fn in_pool() -> Self {
        Self::of(Making::Pool)
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
            let name = d.name.trim();
            let bin = self.snap.pool.bin.clone();
            let (method, params) = match &d.making {
                Making::Clips(clips) => (
                    "edit.compound",
                    json!({ "clips": clips, "name": name, "bin": bin }),
                ),
                Making::Marks => ("edit.compound_range", json!({ "name": name, "bin": bin })),
                Making::Pool => ("media.compound", json!({ "name": name, "bin": bin })),
            };
            self.send(method, params);
        } else if open && !cancel {
            self.new_compound = Some(d);
        }
    }
}
