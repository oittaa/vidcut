use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::request::ViewerMode;

use super::workspace::Workspace;

#[derive(Debug)]
pub(super) struct NewSubclip {
    media: String,
    range: (i64, i64),
    name: String,
    focus: bool,
}

impl NewSubclip {
    pub(super) fn new(media: &str, range: (i64, i64)) -> Self {
        Self {
            media: media.to_string(),
            range,
            name: String::new(),
            focus: true,
        }
    }
}

impl Workspace {
    fn source_clip(&self) -> Option<&str> {
        if self.snap.viewer != ViewerMode::SourceClip {
            return None;
        }
        self.snap.source_at().map(|(media, _)| media)
    }

    pub(super) fn marked_range(&self, media: &str) -> (i64, i64) {
        let marks = self
            .project()
            .and_then(|p| p.media(media).map(|m| m.marks.clone()))
            .unwrap_or_default();
        (
            marks.mark_in.unwrap_or(0),
            marks.mark_out.unwrap_or(self.media_frames(media)),
        )
    }

    pub(super) fn open_new_subclip(&mut self) {
        if let Some(media) = self.source_clip().map(str::to_string) {
            self.new_subclip = Some(NewSubclip::new(&media, self.marked_range(&media)));
        }
    }

    pub(super) fn create_subclip_button(&mut self, ui: &mut egui::Ui) {
        let button = egui::Button::new("Create Subclip").shortcut_text(super::keys("Alt+B"));
        if ui
            .add_enabled(self.source_clip().is_some(), button)
            .clicked()
        {
            self.open_new_subclip();
            ui.close();
        }
    }

    pub(super) fn new_subclip_tick(&mut self, ctx: &egui::Context) {
        let mut state = self.new_subclip.take();
        self.subclip_window(ctx, "New Subclip", &mut state);
        self.new_subclip = state;
    }

    pub(super) fn subclip_window(
        &mut self,
        ctx: &egui::Context,
        title: &str,
        state: &mut Option<NewSubclip>,
    ) {
        let Some(mut d) = state.take() else {
            return;
        };
        let mut open = true;
        let (mut create, mut cancel) = (false, false);
        super::dialog::window(ctx, title, &mut open, |ui| {
            let mut enter = false;
            super::dialog::form(ui, "subclip-form", |ui| {
                super::dialog::row(ui, "Name", |ui| {
                    let field = ui.text_edit_singleline(&mut d.name);
                    if d.focus {
                        field.request_focus();
                        d.focus = false;
                    }
                    enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                });
            });
            let ready = !d.name.trim().is_empty();
            super::dialog::footer(ui, |ui| {
                create = super::dialog::main_button(ui, "Create", ready) || (enter && ready);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if create {
            let (a, b) = d.range;
            self.send(
                "media.subclip",
                json!({
                    "media": d.media,
                    "range": format!("{a}f..{b}f"),
                    "name": d.name.trim(),
                    "bin": self.snap.pool.bin,
                }),
            );
        } else if open && !cancel {
            *state = Some(d);
        }
    }
}
