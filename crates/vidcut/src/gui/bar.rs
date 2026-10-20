use eframe::egui::{self, RichText};

use super::row::Row;
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

const LEFT: [Item; 4] = [
    Item::Import,
    Item::ImportFolder,
    Item::QuickExport,
    Item::Export,
];

const RIGHT: [Item; 7] = [
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

impl Workspace {
    pub(super) fn bar_buttons(&mut self, ui: &mut egui::Ui) {
        self.watch_jobs(ui.input(|i| i.time));
        let left: Vec<Item> = LEFT.into_iter().filter(|&i| self.bar_has(i)).collect();
        let right: Vec<Item> = RIGHT.into_iter().filter(|&i| self.bar_has(i)).collect();
        let row = Row {
            left: &left,
            fill: None,
            right: &right,
            leaving: &LEAVING,
            group: |i| u8::from(RIGHT.contains(&i)),
        };
        self.fit_row(ui, row, Self::bar_item);
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
                let r = ui
                    .toggle_value(&mut open, "History")
                    .on_hover_text("Every step, who made it and when");
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
                .on_hover_text("Go to, mark and cut the words said"),
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
