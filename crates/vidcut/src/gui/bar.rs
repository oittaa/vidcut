use eframe::egui::{self, RichText};
use vidcut_core::request::{ViewerFrame, ViewerFrameKey, viewer_frame_after};

use super::row::Row;
use super::theme;
use super::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    QuickExport,
    NoPreview,
    Jobs,
    Transcription,
    Mixer,
    Help,
    FullScreen,
}

const LEFT: [Item; 1] = [Item::QuickExport];

const RIGHT: [Item; 6] = [
    Item::NoPreview,
    Item::Jobs,
    Item::Transcription,
    Item::Mixer,
    Item::Help,
    Item::FullScreen,
];

const LEAVING: [Item; 7] = [
    Item::FullScreen,
    Item::Help,
    Item::Mixer,
    Item::Transcription,
    Item::QuickExport,
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
            Item::QuickExport => {
                let r = ui.button("Quick Export");
                if r.clicked() {
                    self.open_quick_export(ui.ctx());
                }
                r
            }
            Item::NoPreview => ui.label(RichText::new("no preview").color(theme::colors().error)),
            Item::Jobs => self.jobs_button(ui),
            Item::Transcription => ui.toggle_value(&mut self.transcription.open, "Transcription"),
            Item::Mixer => ui.toggle_value(&mut self.show_mixer, "Mixer"),
            Item::Help => ui.toggle_value(&mut self.help.open, "Help"),
            Item::FullScreen => {
                let now = self.snap.viewer_frame;
                let r = ui.selectable_label(now == ViewerFrame::FullScreen, "Full Screen");
                if r.clicked() {
                    self.send_frame(viewer_frame_after(now, ViewerFrameKey::FullScreen));
                }
                r
            }
        }
    }
}
