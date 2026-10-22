use eframe::egui::{self, RichText};

use super::row::Row;
use super::theme;
use super::workspace::Workspace;

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
    Help,
}

const LEFT: [Item; 4] = [
    Item::Import,
    Item::ImportFolder,
    Item::QuickExport,
    Item::Export,
];

const RIGHT: [Item; 6] = [
    Item::NoPreview,
    Item::Jobs,
    Item::History,
    Item::Transcription,
    Item::Mixer,
    Item::Help,
];

const LEAVING: [Item; 10] = [
    Item::Help,
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
            Item::Jobs | Item::Help => true,
            _ => self.project().is_some(),
        }
    }

    fn bar_item(&mut self, ui: &mut egui::Ui, item: Item) -> egui::Response {
        match item {
            Item::Import => {
                let r = ui.button("Import");
                if r.clicked() {
                    self.import_media();
                }
                r
            }
            Item::ImportFolder => {
                let r = ui.button("Import Folder");
                if r.clicked() {
                    self.import_folder();
                }
                r
            }
            Item::QuickExport => {
                let r = ui.button("Quick Export");
                if r.clicked() {
                    self.open_quick_export(ui.ctx());
                }
                r
            }
            Item::Export => {
                let r = ui.button("Export");
                if r.clicked() {
                    self.open_export(ui.ctx());
                }
                r
            }
            Item::NoPreview => ui.label(RichText::new("no preview").color(theme::colors().error)),
            Item::Jobs => self.jobs_button(ui),
            Item::History => {
                let mut open = self.history.open;
                let r = ui.toggle_value(&mut open, "History");
                if r.changed() {
                    if open {
                        self.history.show();
                    } else {
                        self.history.close();
                    }
                }
                r
            }
            Item::Transcription => ui.toggle_value(&mut self.transcription.open, "Transcription"),
            Item::Mixer => ui.toggle_value(&mut self.show_mixer, "Mixer"),
            Item::Help => ui.toggle_value(&mut self.help.open, "Help"),
        }
    }
}
