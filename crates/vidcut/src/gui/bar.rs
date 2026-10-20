use eframe::egui::{self, RichText};

use super::theme;
use super::workspace::{FOLDER_HINT, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Import,
    ImportFolder,
    QuickExport,
    Export,
    NoPreview,
    Jobs,
    History,
    Transcription,
    Mixer,
    Log,
    Help,
}

const ORDER: [Item; 11] = [
    Item::Import,
    Item::ImportFolder,
    Item::QuickExport,
    Item::Export,
    Item::NoPreview,
    Item::Jobs,
    Item::History,
    Item::Transcription,
    Item::Mixer,
    Item::Log,
    Item::Help,
];

const LEAVING: [Item; 11] = [
    Item::Help,
    Item::Log,
    Item::Mixer,
    Item::Transcription,
    Item::History,
    Item::Export,
    Item::QuickExport,
    Item::ImportFolder,
    Item::Import,
    Item::Jobs,
    Item::NoPreview,
];

impl Item {
    fn left(self) -> bool {
        matches!(
            self,
            Item::Import | Item::ImportFolder | Item::QuickExport | Item::Export
        )
    }
}

fn overflow(room: f32, more: f32, widths: &[f32]) -> usize {
    let mut need: f32 = widths.iter().sum();
    if need <= room {
        return 0;
    }
    need += more;
    for (n, w) in widths.iter().enumerate() {
        if need <= room {
            return n;
        }
        need -= w;
    }
    widths.len()
}

fn more_button(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    ui.menu_button(egui_phosphor::regular::DOTS_THREE, add)
        .response
        .on_hover_text("The buttons the window is too narrow for")
}

impl Workspace {
    pub(super) fn bar_buttons(&mut self, ui: &mut egui::Ui) {
        self.watch_jobs(ui.input(|i| i.time));
        let items: Vec<Item> = LEAVING.into_iter().filter(|&i| self.bar_has(i)).collect();
        let gap = ui.spacing().item_spacing.x;
        let mut sizer = ui.new_child(egui::UiBuilder::new().sizing_pass().invisible());
        let widths: Vec<f32> = items
            .iter()
            .map(|&i| self.bar_item(&mut sizer, i).rect.width() + gap)
            .collect();
        let more = more_button(&mut sizer, |_| {}).rect.width() + gap;
        let hidden = &items[..overflow(ui.available_width(), more, &widths)];
        let shown = |i: &Item| items.contains(i) && !hidden.contains(i);
        for i in ORDER.into_iter().filter(|i| i.left() && shown(i)) {
            self.bar_item(ui, i);
        }
        let right = egui::UiBuilder::new()
            .scope_id(egui::Id::unique("top-bar-right"))
            .layout(egui::Layout::right_to_left(egui::Align::Center));
        ui.scope_builder(right, |ui| {
            if !hidden.is_empty() {
                more_button(ui, |ui| {
                    let mut after_left = false;
                    for i in ORDER.into_iter().filter(|i| hidden.contains(i)) {
                        if after_left && !i.left() {
                            ui.separator();
                        }
                        after_left = i.left();
                        self.bar_item(ui, i);
                    }
                });
            }
            for i in ORDER.into_iter().rev().filter(|i| !i.left() && shown(i)) {
                self.bar_item(ui, i);
            }
        });
    }

    fn bar_has(&self, item: Item) -> bool {
        match item {
            Item::NoPreview => self.snap.preview_error.is_some(),
            Item::Jobs | Item::Log | Item::Help => true,
            _ => self.project().is_some(),
        }
    }

    fn bar_item(&mut self, ui: &mut egui::Ui, item: Item) -> egui::Response {
        match item {
            Item::Import => {
                let r = ui.button("Import").on_hover_text(super::keys("Ctrl+I"));
                if r.clicked() {
                    self.import_media();
                }
                r
            }
            Item::ImportFolder => {
                let r = ui.button("Import Folder").on_hover_text(FOLDER_HINT);
                if r.clicked() {
                    self.import_folder();
                }
                r
            }
            Item::QuickExport => {
                let r = ui
                    .button("Quick Export")
                    .on_hover_text("Export the In–Out range, or the whole timeline, with a preset");
                if r.clicked() {
                    self.open_quick_export(ui.ctx());
                }
                r
            }
            Item::Export => {
                let r = ui.button("Export").on_hover_text("All export settings");
                if r.clicked() {
                    self.open_export(ui.ctx());
                }
                r
            }
            Item::NoPreview => {
                let why = self.snap.preview_error.clone().unwrap_or_default();
                ui.label(RichText::new("no preview").color(theme::colors().error))
                    .on_hover_text(why)
            }
            Item::Jobs => self.jobs_button(ui),
            Item::History => {
                let mut open = self.history.open;
                let r = ui.toggle_value(&mut open, "History").on_hover_text(
                    "Every step, who made it and when; hover one to see the timeline as it was",
                );
                if r.changed() {
                    if open {
                        self.history.show();
                    } else {
                        self.history.close();
                    }
                }
                r
            }
            Item::Transcription => ui
                .toggle_value(&mut self.transcription.open, "Transcription")
                .on_hover_text(
                    "The words said in the source clip or the timeline: go to them, mark them, \
                     cut them",
                ),
            Item::Mixer => ui
                .toggle_value(&mut self.show_mixer, "Mixer")
                .on_hover_text("Track faders, mute, solo and meters"),
            Item::Log => ui.toggle_value(&mut self.show_log, "Command log"),
            Item::Help => ui
                .toggle_value(&mut self.help.open, "Help")
                .on_hover_text("Keyboard shortcuts, with what the less obvious ones do (F1)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::overflow;

    #[test]
    fn a_bar_that_fits_hides_nothing() {
        assert_eq!(overflow(100.0, 10.0, &[30.0, 30.0, 30.0]), 0);
    }

    #[test]
    fn a_narrow_bar_hides_until_the_rest_and_the_menu_fit() {
        assert_eq!(overflow(80.0, 10.0, &[30.0, 30.0, 30.0]), 1);
    }

    #[test]
    fn a_bar_too_narrow_for_any_hides_all() {
        assert_eq!(overflow(5.0, 10.0, &[30.0, 30.0, 30.0]), 3);
    }
}
