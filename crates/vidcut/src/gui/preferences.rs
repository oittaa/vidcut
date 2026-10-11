//! File > Preferences (Ctrl+,): application preferences, holding only what
//! vidcut needs. Each change is one `preferences.set`, sent when it is made.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::export::{Ask, near_center};
use super::theme;
use super::workspace::Workspace;

#[derive(Default)]
pub(super) struct PreferencesUi {
    open: bool,
    /// The request in flight (`preferences.get` or `preferences.set`).
    ask: Option<Ask>,
    /// The latest answer (`preferences.get`'s result).
    state: Option<Value>,
    error: Option<String>,
    /// The thumbnail cache limit as typed (MB).
    limit: String,
}

/// The thumbnail cache limit's range, in MB.
const LIMIT_MB: std::ops::RangeInclusive<u64> = 1..=1_000_000;

impl Workspace {
    pub(super) fn open_preferences(&mut self, ctx: &egui::Context) {
        self.preferences.open = true;
        self.preferences.ask = Some(Ask::send(&self.live, ctx, "preferences.get", json!({})));
    }

    pub(super) fn preferences_window(&mut self, ctx: &egui::Context) {
        let ui_state = &mut self.preferences;
        if !ui_state.open {
            return;
        }
        if let Some(answer) = ui_state.ask.as_ref().and_then(Ask::take) {
            ui_state.ask = None;
            match answer {
                Ok(r) => {
                    ui_state.limit = r.result["preferences"]["thumbnails"]["cache_limit_mb"]
                        .as_u64()
                        .unwrap_or(1)
                        .to_string();
                    ui_state.state = Some(r.result);
                    ui_state.error = None;
                }
                Err(e) => ui_state.error = Some(e.message),
            }
        }
        let mut open = true;
        let mut patch = None;
        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 380.0, 170.0))
            .open(&mut open)
            .show(ctx, |ui| {
                let Some(state) = &ui_state.state else {
                    match &ui_state.error {
                        Some(e) => ui.label(RichText::new(e).color(theme::ERROR)),
                        None => ui.spinner(),
                    };
                    return;
                };
                let thumbs = &state["preferences"]["thumbnails"];
                let cache = &state["thumbnail_cache"];
                let busy = ui_state.ask.is_some();
                ui.label(RichText::new("Thumbnails").strong());
                egui::Grid::new("preferences-thumbnails")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let mut disk = thumbs["disk_cache"] == true;
                        ui.label("Disk cache");
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Checkbox::new(&mut disk, "Keep thumbnails on disk"),
                            )
                            .on_hover_text(format!(
                                "Projects open with their thumbnails shown, without decoding \
                                 every clip again ({}). Turning this off empties the cache.",
                                cache["path"].as_str().unwrap_or_default()
                            ))
                            .changed()
                        {
                            patch = Some(json!({ "thumbnails": { "disk_cache": disk } }));
                        }
                        ui.end_row();
                        let label = ui.label("Size limit");
                        // Typed, and sent on Enter or when the field loses focus.
                        let saved = thumbs["cache_limit_mb"].as_u64();
                        let r = ui
                            .horizontal(|ui| {
                                let r = ui
                                    .add_enabled(
                                        !busy && disk,
                                        egui::TextEdit::singleline(&mut ui_state.limit)
                                            .desired_width(80.0),
                                    )
                                    .labelled_by(label.id)
                                    .on_hover_text(
                                        "Above it, the least recently used thumbnails go first",
                                    );
                                ui.label("MB");
                                r
                            })
                            .inner;
                        if r.changed() {
                            ui_state.limit.retain(|c| c.is_ascii_digit());
                        }
                        if r.lost_focus() {
                            match ui_state.limit.parse::<u64>() {
                                Ok(mb) if LIMIT_MB.contains(&mb) => {
                                    if Some(mb) != saved {
                                        patch = Some(json!({
                                            "thumbnails": { "cache_limit_mb": mb }
                                        }));
                                    }
                                }
                                _ => {
                                    ui_state.error = Some(format!(
                                        "The size limit is {} to {} MB",
                                        LIMIT_MB.start(),
                                        LIMIT_MB.end()
                                    ));
                                    ui_state.limit = saved.unwrap_or(1).to_string();
                                }
                            }
                        }
                        ui.end_row();
                        ui.label("");
                        ui.label(
                            RichText::new(format!(
                                "Holds {} MB in {} files",
                                cache["mb"], cache["files"]
                            ))
                            .color(theme::TEXT_DIM),
                        );
                        ui.end_row();
                    });
                if let Some(e) = &ui_state.error {
                    ui.label(RichText::new(e).color(theme::ERROR));
                }
                ui.label(
                    RichText::new(state["path"].as_str().unwrap_or_default())
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
        ui_state.open = open;
        if let Some(p) = patch {
            ui_state.ask = Some(Ask::send(&self.live, ctx, "preferences.set", p));
        }
    }
}
