use eframe::egui;
use vidcut_core::request::GuiPanel;

use super::theme::{self, Deco};
use super::workspace::Workspace;

const METERS_HEIGHT: f32 = 220.0;
const METERS_LEAST: f32 = 120.0;
const VIEWER_WIDTH: f32 = 380.0;
const VIEWER_LEAST: f32 = 200.0;
const MIXER_WIDTH: f32 = 360.0;
const SIDE_WIDTH: f32 = 380.0;
const SIDE_LEAST: f32 = 260.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPage {
    pub mixer: bool,
    pub meters: bool,
    pub media: bool,
    pub inspector: bool,
}

impl Default for AudioPage {
    fn default() -> Self {
        Self {
            mixer: true,
            meters: true,
            media: false,
            inspector: false,
        }
    }
}

impl AudioPage {
    pub fn panel(&mut self, panel: GuiPanel) -> Option<&mut bool> {
        match panel {
            GuiPanel::Mixer => Some(&mut self.mixer),
            GuiPanel::Meters => Some(&mut self.meters),
            GuiPanel::Media => Some(&mut self.media),
            GuiPanel::Inspector => Some(&mut self.inspector),
            _ => None,
        }
    }
}

impl Workspace {
    pub(super) fn audio_page_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let panel = theme::colors().panel;
        let page = self.audio_page;
        if page.meters {
            egui::Panel::top("audio-meters")
                .resizable(true)
                .default_size(METERS_HEIGHT)
                .min_size(METERS_LEAST)
                .frame(theme::card(panel))
                .show(ui, |ui| {
                    theme::decorated(ui, Deco::Card, |ui| self.meter_panel(ui))
                });
        }
        let media = page.media.then(|| {
            egui::Panel::left("audio-media")
                .resizable(true)
                .default_size(SIDE_WIDTH)
                .min_size(SIDE_LEAST)
                .frame(theme::card(panel))
                .show(ui, |ui| {
                    theme::decorated(ui, Deco::Card, |ui| self.pool_panel(ui))
                })
                .response
                .rect
        });
        if media.is_none() {
            self.pool_gone();
        }
        if page.inspector {
            egui::Panel::right("audio-inspector")
                .resizable(true)
                .default_size(SIDE_WIDTH)
                .min_size(SIDE_LEAST)
                .frame(theme::card(panel))
                .show(ui, |ui| {
                    theme::decorated(ui, Deco::Card, |ui| self.inspector_panel(ui))
                });
        } else {
            self.leave_inspector();
            self.watch_inspector();
            self.settle_transition_drafts(None, &ctx);
        }
        self.end_title_hover();
        if !self.key_tray {
            self.key_editor.leave();
        }
        self.sync_tick();
        if page.mixer {
            egui::Panel::right("audio-mixer")
                .resizable(true)
                .default_size(MIXER_WIDTH)
                .frame(theme::card(panel))
                .show(ui, |ui| {
                    theme::decorated(ui, Deco::Card, |ui| self.mixer_panel(ui))
                });
        }
        let rows = egui::CentralPanel::default()
            .frame(theme::card(panel))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Card, |ui| self.timeline_panel(ui))
            })
            .response
            .rect;
        self.settle_picture_wheel(&ctx);
        self.media_drag_overlay(&ctx);
        self.drop_hint(&ctx, media.unwrap_or(rows));
    }

    fn meter_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("audio-viewer")
            .resizable(true)
            .default_size(VIEWER_WIDTH)
            .min_size(VIEWER_LEAST)
            .frame(theme::card(theme::colors().panel_dark))
            .show(ui, |ui| {
                theme::decorated(ui, Deco::Viewer, |ui| self.viewer_panel(ui))
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.meter_row(ui));
    }
}
