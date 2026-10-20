use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use serde_json::{Value, json};
use vidcut_core::preferences::Proxies;

use super::dialog;
use super::export::{Ask, Picked, near_center};
use super::theme;
use super::workspace::Workspace;

#[derive(Default)]
pub(super) struct PreferencesUi {
    pub(super) open: bool,
    ask: Option<Ask>,
    state: Option<Value>,
    error: Option<String>,
    limit: String,
    larger_w: String,
    larger_h: String,
    mbps: String,
    size: String,
    folder: String,
    player_memory: String,
    video_memory: String,
    audio_outputs: Option<Result<Vec<vidcut_av::player::OutputDevice>, String>>,
    listing:
        Option<std::sync::mpsc::Receiver<Result<Vec<vidcut_av::player::OutputDevice>, String>>>,
    importing: Option<Picked>,
    choosing_folder: Option<Picked>,
    confirm_reset: bool,
    reset_saved: bool,
    reset_keep: bool,
    keys: [String; 2],
    commands: [String; 2],
    keyed: Option<(&'static str, Value)>,
    resetting: Option<Ask>,
    forgotten: Option<super::workspace::ViewState>,
    fillers: super::fillers::FillerEditor,
    page: Page,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    General,
    Gpu,
    Memory,
    Sound,
    Cache,
    Proxies,
    Services,
    Fillers,
}

const PAGES: [(Page, &str, &str); 8] = [
    (Page::General, icon::GEAR_SIX, "General"),
    (Page::Gpu, icon::GRAPHICS_CARD, "GPU"),
    (Page::Memory, icon::MEMORY, "Memory"),
    (Page::Sound, icon::SPEAKER_HIGH, "Sound"),
    (Page::Cache, icon::DATABASE, "Cache"),
    (Page::Proxies, icon::FILM_STRIP, "Proxies"),
    (Page::Services, icon::CLOUD, "Services"),
    (Page::Fillers, icon::CHAT_TEXT, "Filler Words"),
];

const LABEL: f32 = 140.0;
const SIDEBAR: f32 = 150.0;
const PAGE: f32 = 500.0;

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Plain,
    Gb,
}

struct NumberField<'a> {
    label: egui::Id,
    saved: u64,
    range: std::ops::RangeInclusive<u64>,
    unit: Unit,
    enabled: bool,
    tip: &'a str,
    refusal: &'a str,
}

impl NumberField<'_> {
    fn show(self, ui: &mut egui::Ui, text: &mut String, error: &mut Option<String>) -> Option<u64> {
        let r = ui
            .add_enabled(
                self.enabled,
                egui::TextEdit::singleline(text).desired_width(56.0),
            )
            .labelled_by(self.label)
            .on_hover_text(self.tip);
        if r.changed() {
            text.retain(|c| {
                c.is_ascii_digit() || (self.unit == Unit::Gb && matches!(c, '.' | ','))
            });
        }
        if !r.lost_focus() {
            return None;
        }
        let frame = ui.ctx().cumulative_frame_nr();
        let key = r.id.with("lost focus");
        let before = ui.data_mut(|d| d.get_temp::<u64>(key));
        ui.data_mut(|d| d.insert_temp(key, frame));
        if before.is_some_and(|b| b + 1 == frame) {
            return None;
        }
        let typed = match self.unit {
            Unit::Plain => text.parse::<u64>().ok(),
            Unit::Gb => mb_of(text),
        };
        let shown = |n: u64| match self.unit {
            Unit::Plain => n.to_string(),
            Unit::Gb => gb(n),
        };
        match typed {
            Some(n) if self.range.contains(&n) => {
                *text = shown(n);
                (n != self.saved).then_some(n)
            }
            _ => {
                *error = Some(self.refusal.to_string());
                *text = shown(self.saved);
                None
            }
        }
    }
}

fn gb(mb: u64) -> String {
    if mb.is_multiple_of(1024) {
        (mb / 1024).to_string()
    } else {
        let text = format!("{:.2}", mb as f64 / 1024.0);
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn about_gb(mb: u64) -> String {
    let gb = mb as f64 / 1024.0;
    if gb >= 10.0 {
        format!("{gb:.0}")
    } else {
        let text = format!("{gb:.1}");
        text.trim_end_matches(".0").to_string()
    }
}

fn mb_of(text: &str) -> Option<u64> {
    let gb: f64 = text.replace(',', ".").parse().ok()?;
    (gb.is_finite() && gb >= 0.0).then(|| (gb * 1024.0).round() as u64)
}

fn page(ui: &mut egui::Ui, title: &str, about: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(title).strong().size(18.0));
    if !about.is_empty() {
        dialog::note(ui, about);
    }
    ui.add_space(10.0);
    add(ui);
}

fn label(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(dim(text))
}

fn grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(LABEL)
        .spacing([12.0, 8.0])
        .show(ui, add);
}

fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(theme::colors().text_dim)
}

impl Workspace {
    pub(super) fn open_preferences(&mut self, ctx: &egui::Context) {
        self.preferences.open = true;
        self.preferences.ask = Some(Ask::send(&self.live, ctx, "preferences.get", json!({})));
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        let _ = crate::threads::spawn(
            "vidcut-audio-devices",
            crate::threads::Work::Interactive,
            move || {
                let _ = tx.send(vidcut_av::player::output_devices().map_err(|e| e.to_string()));
                repaint.request_repaint();
            },
        );
        self.preferences.listing = Some(rx);
    }

    pub(super) fn reread_preferences(&mut self, ctx: &egui::Context) {
        if self.preferences.open && self.preferences.ask.is_none() {
            self.preferences.ask = Some(Ask::send(&self.live, ctx, "preferences.get", json!({})));
        }
    }

    pub(super) fn preferences_window(&mut self, ctx: &egui::Context) {
        self.reset_answered(ctx);
        self.reset_window(ctx);
        let project = self.project_folder();
        let ui_state = &mut self.preferences;
        if !ui_state.open {
            return;
        }
        if let Some(answer) = ui_state.ask.as_ref().and_then(Ask::take) {
            ui_state.ask = None;
            match answer {
                Ok(r) => {
                    let p = &r.result["preferences"];
                    let n = |v: &Value| v.as_u64().unwrap_or(0).to_string();
                    ui_state.limit = n(&p["thumbnails"]["cache_limit_mb"]);
                    let size = |key: &str, auto: &str| match p["player"][key].as_u64().unwrap_or(0)
                    {
                        0 => gb(r.result[auto].as_u64().unwrap_or(1024)),
                        mb => gb(mb),
                    };
                    ui_state.player_memory = size("memory_mb", "player_memory_auto_mb");
                    ui_state.video_memory = size("video_memory_mb", "player_video_memory_auto_mb");
                    if let (Some(tiles), Ok(t)) = (
                        &mut self.tiles,
                        serde_json::from_value(p["thumbnails"].clone()),
                    ) {
                        tiles.set_prefs(&t);
                    }
                    ui_state.larger_w = n(&p["proxies"]["larger_than"][0]);
                    ui_state.larger_h = n(&p["proxies"]["larger_than"][1]);
                    ui_state.mbps = n(&p["proxies"]["above_mbps"]);
                    ui_state.size = n(&p["proxies"]["size"]);
                    ui_state.folder = proxy_folder(p).to_string();
                    let command =
                        |k: &str| p["services"][k].as_str().unwrap_or_default().to_string();
                    ui_state.commands = [command("xai_key_command"), command("google_key_command")];
                    ui_state.state = Some(r.result);
                    ui_state.error = None;
                }
                Err(e) => {
                    if let Some(state) = &ui_state.state {
                        ui_state.folder = proxy_folder(&state["preferences"]).to_string();
                    }
                    ui_state.error = Some(e.message);
                }
            }
        }
        let mut open = true;
        let mut patch = None;
        if let Some(picked) = ui_state.importing.as_ref().and_then(Picked::take) {
            ui_state.importing = None;
            if let Some(path) = picked {
                patch = Some(json!({ "appearance": { "theme": path } }));
            }
        }
        if let Some(picked) = ui_state.choosing_folder.as_ref().and_then(Picked::take) {
            ui_state.choosing_folder = None;
            if let Some(path) = picked {
                patch = Some(json!({ "proxies": { "folder": path } }));
            }
        }
        if let Some(listed) = ui_state.listing.as_ref().and_then(|r| r.try_recv().ok()) {
            ui_state.audio_outputs = Some(listed);
            ui_state.listing = None;
        }
        let dialogs = self.dialogs.clone();
        let display = self.display.clone();
        let height = (ctx.content_rect().height() * 0.7).clamp(240.0, 420.0);
        let mut reset = false;
        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, SIDEBAR + PAGE + 40.0, height))
            .min_width(SIDEBAR + PAGE + 16.0)
            .max_width(SIDEBAR + PAGE + 16.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let Some(state) = ui_state.state.clone() else {
                    let Some(e) = &ui_state.error else {
                        ui.spinner();
                        return;
                    };
                    ui.set_max_width(PAGE);
                    ui.add(egui::Label::new(RichText::new(e).color(theme::colors().error)).wrap());
                    ui.horizontal(|ui| {
                        if ui
                            .button("Show in Folder")
                            .on_hover_text("The preferences file, to fix by hand")
                            .clicked()
                        {
                            super::file_manager::show(&crate::preferences::path());
                        }
                        reset = ui
                            .add_enabled(ui_state.ask.is_none(), egui::Button::new("Reset Preferences"))
                            .on_hover_text("Back to the defaults, keeping the old file as preferences.toml.bak")
                            .clicked();
                    });
                    return;
                };
                let busy = ui_state.ask.is_some() || ui_state.resetting.is_some();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(SIDEBAR);
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            ui.spacing_mut().button_padding.y = 6.0;
                            for (page, icon, name) in PAGES {
                                if ui
                                    .selectable_label(ui_state.page == page, format!("{icon}  {name}"))
                                    .clicked()
                                {
                                    ui_state.page = page;
                                }
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_min_size(egui::vec2(PAGE, height));
                        egui::ScrollArea::vertical()
                            .max_height(height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(PAGE - 16.0);
                                ui.spacing_mut().item_spacing.y = 8.0;
                                match ui_state.page {
                                    Page::General => {
                                        general(ui, &state, busy, ui_state, &dialogs, &mut patch)
                                    }
                                    Page::Gpu => gpu(ui, &state, busy, display.as_deref(), &mut patch),
                                    Page::Memory => memory(ui, &state, busy, ui_state, &mut patch),
                                    Page::Sound => sound(ui, &state, busy, ui_state, &mut patch),
                                    Page::Cache => cache(ui, &state, busy, ui_state, &mut patch),
                                    Page::Services => {
                                        services(ui, &state, busy, ui_state, &mut patch)
                                    }
                                    Page::Proxies => {
                                        let folders = Folders {
                                            dialogs: &dialogs,
                                            project: project.as_deref(),
                                        };
                                        proxies(ui, &state, busy, ui_state, &folders, &mut patch);
                                    }
                                    Page::Fillers => fillers(ui, &state, busy, ui_state, &mut patch),
                                }
                                if let Some(e) = &ui_state.error {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(e).color(theme::colors().error),
                                        )
                                        .wrap(),
                                    );
                                }
                            });
                    });
                });
            });
        ui_state.open = open;
        if reset {
            ui_state.ask = Some(Ask::send(&self.live, ctx, "preferences.reset", json!({})));
        }
        if let Some(p) = patch {
            ui_state.ask = Some(Ask::send(&self.live, ctx, "preferences.set", p));
        }
        if let Some((method, params)) = ui_state.keyed.take() {
            self.send(method, params);
            self.preferences.ask = Some(Ask::send(&self.live, ctx, "preferences.get", json!({})));
        }
    }

    fn reset_window(&mut self, ctx: &egui::Context) {
        let ui_state = &mut self.preferences;
        if !ui_state.confirm_reset {
            return;
        }
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::unique("reset-vidcut")).show(ctx, |ui| {
            ui.set_width(dialog::WIDTH);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.heading("Reset vidcut?");
            ui.add(
                egui::Label::new(
                    "Deletes the preferences, caches, undo histories, recent projects and view \
                     settings.",
                )
                .wrap(),
            );
            dialog::note(
                ui,
                "Projects, media and your proxy folder stay. It can't be undone.",
            );
            ui.checkbox(
                &mut ui_state.reset_saved,
                "Also delete my presets, themes and looks",
            );
            ui.checkbox(
                &mut ui_state.reset_keep,
                "Keep downloaded models and paid replies",
            );
            dialog::footer(ui, |ui| {
                if ui
                    .button(RichText::new("Reset").color(theme::colors().error))
                    .clicked()
                {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        if choice.is_none() && super::workspace::escape_cancels(ctx, &modal) {
            choice = Some(false);
        }
        match choice {
            Some(true) => {
                let saved = ui_state.reset_saved;
                let keep = ui_state.reset_keep;
                ui_state.confirm_reset = false;
                let forgotten = self.forget_view_state();
                self.preferences.forgotten = Some(forgotten);
                let params = json!({ "saved": saved, "keep_downloads": keep });
                self.preferences.resetting = Some(Ask::send(&self.live, ctx, "reset", params));
            }
            Some(false) => ui_state.confirm_reset = false,
            None => {}
        }
    }

    fn reset_answered(&mut self, ctx: &egui::Context) {
        let Some(answer) = self.preferences.resetting.as_ref().and_then(Ask::take) else {
            return;
        };
        self.preferences.resetting = None;
        match answer {
            Ok(r) => {
                let n = r.result["removed"].as_array().map_or(0, Vec::len);
                self.preferences.forgotten = None;
                self.after_reset();
                self.say(format!("Reset: {n} files and folders removed"));
                if self.preferences.open {
                    self.preferences.ask =
                        Some(Ask::send(&self.live, ctx, "preferences.get", json!({})));
                }
            }
            Err(e) => {
                if let Some(old) = self.preferences.forgotten.take() {
                    self.restore_view_state(old);
                }
                self.preferences.error = Some(e.message);
            }
        }
    }
}

fn general(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    dialogs: &std::sync::Arc<dyn super::export::Dialogs>,
    patch: &mut Option<Value>,
) {
    page(ui, "General", "", |ui| {
        grid(ui, "preferences-general", |ui| {
            let chosen = state["preferences"]["appearance"]["theme"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let themes = state["themes"].as_array().cloned().unwrap_or_default();
            let title = |name: &str| {
                themes
                    .iter()
                    .find(|t| t["name"] == name)
                    .and_then(|t| t["title"].as_str())
                    .unwrap_or(name)
                    .to_string()
            };
            let mut pick = chosen.clone();
            label(ui, "Theme");
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    egui::ComboBox::from_id_salt("preferences-theme")
                        .selected_text(title(&chosen))
                        .show_ui(ui, |ui| {
                            for t in &themes {
                                let name = t["name"].as_str().unwrap_or_default();
                                let r = ui.add_enabled(
                                    t["error"].is_null(),
                                    egui::Button::selectable(pick == name, title(name)),
                                );
                                if r.clicked() {
                                    pick = name.to_string();
                                }
                                if let Some(e) = t["error"].as_str() {
                                    r.on_disabled_hover_text(e);
                                } else if let Some(p) = t["path"].as_str() {
                                    r.on_hover_text(p);
                                }
                            }
                        });
                });
                if ui
                    .add_enabled(
                        !busy && ui_state.importing.is_none(),
                        egui::Button::new("Import…"),
                    )
                    .on_hover_text("Copies a theme file (.toml) into the themes folder")
                    .clicked()
                {
                    let dialogs = dialogs.clone();
                    ui_state.importing = Some(Picked::ask(ui.ctx(), move || {
                        dialogs.pick_file("Import Theme", &std::path::PathBuf::new(), &["toml"])
                    }));
                }
            });
            if pick != chosen {
                *patch = Some(json!({ "appearance": { "theme": pick } }));
            }
            ui.end_row();
            let path = state["path"].as_str().unwrap_or_default();
            label(ui, "Preferences File");
            ui.horizontal(|ui| {
                if ui.button("Show in Folder").on_hover_text(path).clicked() {
                    super::file_manager::show(Path::new(path));
                }
            });
            ui.end_row();
            label(ui, "Reset");
            if ui
                .add_enabled(
                    ui_state.resetting.is_none(),
                    egui::Button::new("Reset vidcut…"),
                )
                .on_hover_text("Back to a fresh install, keeping projects and media")
                .clicked()
            {
                ui_state.confirm_reset = true;
                ui_state.reset_saved = false;
                ui_state.reset_keep = true;
            }
            ui.end_row();
        });
        let ignored: Vec<&str> = state["ignored"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !ignored.is_empty() {
            dialog::note(
                ui,
                format!(
                    "No longer preferences, dropped at the next change: {}",
                    ignored.join(", ")
                ),
            );
        }
    });
}

fn gpu(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    display: Option<&str>,
    patch: &mut Option<Value>,
) {
    page(
        ui,
        "GPU",
        "Decoding, scaling and encoding on the graphics card",
        |ui| {
            grid(ui, "preferences-gpu", |ui| {
                let mut on = state["preferences"]["gpu"]["acceleration"] != false;
                label(ui, "Acceleration");
                if ui
                    .add_enabled(
                        !busy,
                        egui::Checkbox::new(&mut on, "Decode, scale and encode on the GPU"),
                    )
                    .on_hover_text("Off: everything runs on the CPU, for a troublesome driver")
                    .changed()
                {
                    *patch = Some(json!({ "gpu": { "acceleration": on } }));
                }
                ui.end_row();
                let mut decoding = state["preferences"]["gpu"]["decoding"] != false;
                label(ui, "Decoding");
                if ui
                    .add_enabled(
                        !busy && on,
                        egui::Checkbox::new(&mut decoding, "Decode on the GPU too"),
                    )
                    .on_hover_text("Off where a small GPU decodes slower than the CPU")
                    .changed()
                {
                    *patch = Some(json!({ "gpu": { "decoding": decoding } }));
                }
                ui.end_row();
                label(ui, "GPU in Use");
                let gpus = state["gpus"].as_array().cloned().unwrap_or_default();
                let name = |g: &Value| g["name"].as_str().unwrap_or_default().to_string();
                if !on {
                    ui.label(dim("None: everything runs on the CPU"));
                } else if gpus.len() > 1 {
                    gpu_choice(ui, state, busy, &gpus, patch);
                } else if let Some(g) = gpus.first() {
                    ui.label(name(g));
                } else {
                    ui.label(display.unwrap_or("This computer's GPU"));
                }
                ui.end_row();
            });
        },
    );
}

fn gpu_choice(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    gpus: &[Value],
    patch: &mut Option<Value>,
) {
    let name = |g: &Value| g["name"].as_str().unwrap_or_default().to_string();
    let automatic = match state["gpu_automatic"].is_object() {
        true => format!("Automatic ({})", name(&state["gpu_automatic"])),
        false => "Automatic".to_string(),
    };
    let chosen = state["preferences"]["gpu"]["device"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let found = state["gpu_chosen"]["id"].as_str().map(String::from);
    let shown = match &found {
        _ if chosen.is_empty() => automatic.clone(),
        Some(_) => name(&state["gpu_chosen"]),
        None => format!("{chosen} (not here: automatic)"),
    };
    let current = found.unwrap_or_else(|| chosen.clone());
    let mut pick = current.clone();
    ui.add_enabled_ui(!busy, |ui| {
        egui::ComboBox::from_id_salt("preferences-gpu-device")
            .selected_text(shown)
            .width(300.0)
            .truncate()
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut pick, String::new(), &automatic);
                for g in gpus {
                    let id = g["id"].as_str().unwrap_or_default();
                    ui.selectable_value(&mut pick, id.to_string(), name(g));
                }
            })
            .response
            .on_hover_text("Automatic: NVIDIA's when there is one, else the first other");
    });
    if pick != current {
        *patch = Some(json!({ "gpu": { "device": pick } }));
    }
}

fn memory(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    patch: &mut Option<Value>,
) {
    page(
        ui,
        "Memory",
        "Pictures kept around the playhead, so stepping and playback wait less",
        |ui| {
            grid(ui, "preferences-memory", |ui| {
                let player = &state["preferences"]["player"];
                let system = state["system_memory_mb"].as_u64();
                let auto_mb = state["player_memory_auto_mb"].as_u64().unwrap_or(1024);
                let viewer = match system {
                    Some(m) if auto_mb > 1024 => {
                        format!(
                            "{} GB, half of this computer's {} GB",
                            about_gb(auto_mb),
                            about_gb(m)
                        )
                    }
                    _ => format!("{} GB", about_gb(auto_mb)),
                };
                let in_use = if state["gpu_chosen"].is_object() {
                    &state["gpu_chosen"]
                } else {
                    &state["gpu_automatic"]
                };
                let video_auto = state["player_video_memory_auto_mb"]
                    .as_u64()
                    .unwrap_or(1024);
                let gpu = &state["preferences"]["gpu"];
                let gpu_decodes = gpu["acceleration"] != false && gpu["decoding"] != false;
                let video = match in_use["memory_mb"].as_u64() {
                    _ if state["player_video_memory_shared"] == true => {
                        format!("Shares the {} GB above", about_gb(video_auto))
                    }
                    Some(m) if gpu_decodes && video_auto > 1024 => {
                        format!(
                            "{} GB, half of the GPU's {} GB",
                            about_gb(video_auto),
                            about_gb(m)
                        )
                    }
                    _ => format!("{} GB", about_gb(video_auto)),
                };
                for (key, name, text, auto, about, tip) in [
                    (
                        "memory_mb",
                        "Viewer Pictures",
                        &mut ui_state.player_memory,
                        viewer,
                        None,
                        "Pictures the CPU decoded, in the computer's memory",
                    ),
                    (
                        "video_memory_mb",
                        "Pictures on the GPU",
                        &mut ui_state.video_memory,
                        video,
                        (!gpu_decodes).then_some("Not used while the CPU decodes"),
                        "Pictures the GPU decoded, kept in its memory",
                    ),
                ] {
                    let saved = player[key].as_u64().unwrap_or(0);
                    let label = label(ui, name).on_hover_text(tip);
                    ui.horizontal(|ui| {
                        let custom = saved != 0;
                        dialog::switch(ui, |ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::selectable(!custom, "Automatic"))
                                .clicked()
                                && custom
                            {
                                *patch = Some(json!({ "player": { key: 0 } }));
                            }
                            if ui
                                .add_enabled(!busy, egui::Button::selectable(custom, "Custom"))
                                .clicked()
                                && !custom
                                && let Some(mb) = mb_of(text)
                            {
                                *patch = Some(json!({ "player": { key: mb.max(64) } }));
                            }
                        });
                        if custom {
                            let field = NumberField {
                                label: label.id,
                                saved,
                                range: 64..=100_000_000,
                                unit: Unit::Gb,
                                enabled: !busy,
                                tip,
                                refusal: "Custom memory is at least 0.0625 GB (64 MB)",
                            };
                            if let Some(mb) = field.show(ui, text, &mut ui_state.error) {
                                *patch = Some(json!({ "player": { key: mb } }));
                            }
                            ui.label("GB");
                        } else {
                            ui.label(dim(auto));
                        }
                    });
                    ui.end_row();
                    if let Some(about) = about {
                        ui.label("");
                        ui.label(dim(about));
                        ui.end_row();
                    }
                }
            });
        },
    );
}

fn sound(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &PreferencesUi,
    patch: &mut Option<Value>,
) {
    page(ui, "Sound", "Where the viewer's sound plays", |ui| {
        grid(ui, "preferences-sound", |ui| {
            let chosen = state["preferences"]["player"]["audio_device"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            label(ui, "Output");
            match &ui_state.audio_outputs {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::colors().error));
                }
                Some(Ok(devices)) => {
                    let default = devices
                        .iter()
                        .find(|d| d.default)
                        .map_or(String::new(), |d| format!(" ({})", d.name));
                    let shown = match devices.iter().find(|d| d.id == chosen) {
                        _ if chosen.is_empty() => format!("System default{default}"),
                        Some(d) => d.name.clone(),
                        None => format!("{chosen} (not connected: the default plays)"),
                    };
                    let mut pick = chosen.clone();
                    ui.add_enabled_ui(!busy, |ui| {
                        egui::ComboBox::from_id_salt("preferences-audio-device")
                            .selected_text(shown)
                            .width(300.0)
                            .truncate()
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut pick,
                                    String::new(),
                                    format!("System default{default}"),
                                );
                                for d in devices {
                                    ui.selectable_value(&mut pick, d.id.clone(), &d.name);
                                }
                            })
                            .response
                            .on_hover_text(
                                "A device that isn't connected plays on the system's default",
                            );
                    });
                    if pick != chosen {
                        *patch = Some(json!({ "player": { "audio_device": pick } }));
                    }
                }
            }
            ui.end_row();
        });
    });
}

fn folder_row(ui: &mut egui::Ui, name: &str, held: &Value, what: &str) {
    label(ui, name);
    ui.horizontal(|ui| {
        ui.label(dim(format!(
            "{} MB in {} {what}",
            held["mb"], held["files"]
        )));
        if ui.button("Show in Folder").clicked() {
            super::file_manager::show(Path::new(held["path"].as_str().unwrap_or_default()));
        }
    });
    ui.end_row();
}

fn cache(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    patch: &mut Option<Value>,
) {
    let thumbs = &state["preferences"]["thumbnails"];
    let cache = &state["thumbnail_cache"];
    page(
        ui,
        "Cache",
        "Kept here, so it isn't made, downloaded or paid for again",
        |ui| {
            grid(ui, "preferences-cache", |ui| {
                folder_row(ui, "Speech Models", &state["cache"]["models"], "files");
                folder_row(ui, "Paid Replies", &state["cache"]["replies"], "replies");
                let mut disk = thumbs["disk_cache"] == true;
                label(ui, "Thumbnails");
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut disk, "Keep them on disk"))
                    .on_hover_text("Projects open with their pictures at once; off empties it")
                    .changed()
                {
                    *patch = Some(json!({ "thumbnails": { "disk_cache": disk } }));
                }
                ui.end_row();
                let label = label(ui, "Size Limit");
                ui.horizontal(|ui| {
                    let field = NumberField {
                        label: label.id,
                        saved: thumbs["cache_limit_mb"].as_u64().unwrap_or(1),
                        range: 1..=1_000_000,
                        unit: Unit::Plain,
                        enabled: !busy && disk,
                        tip: "Above it, the pictures used least recently go first",
                        refusal: "The size limit is 1 to 1000000 MB",
                    };
                    if let Some(mb) = field.show(ui, &mut ui_state.limit, &mut ui_state.error) {
                        *patch = Some(json!({ "thumbnails": { "cache_limit_mb": mb } }));
                    }
                    ui.label("MB");
                    ui.label(dim(format!(
                        "Holds {} MB in {} files",
                        cache["mb"], cache["files"]
                    )))
                    .on_hover_text(cache["path"].as_str().unwrap_or_default());
                });
                ui.end_row();
            });
        },
    );
}

fn services(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    patch: &mut Option<Value>,
) {
    let saved = &state["preferences"]["services"];
    page(
        ui,
        "Services",
        "Paid speech recognition: xAI and Google",
        |ui| {
            for (i, (service, name)) in [("xai", "xAI"), ("google", "Google")]
                .into_iter()
                .enumerate()
            {
                ui.add_space(4.0);
                ui.label(RichText::new(name).strong());
                let info = &state["services"][service];
                let variable = info["variable"].as_str().unwrap_or_default();
                grid(ui, &format!("preferences-{service}"), |ui| {
                    label(ui, "Key");
                    ui.label(match info["key"].as_str() {
                        Some("environment") => format!("From {variable}"),
                        Some("command") => "From the key command".into(),
                        Some("keychain") => "In the keychain".into(),
                        _ => "Not set".into(),
                    });
                    ui.end_row();
                    label(ui, "New Key");
                    ui.horizontal(|ui| {
                        ui.add_enabled(
                            !busy,
                            egui::TextEdit::singleline(&mut ui_state.keys[i])
                                .password(true)
                                .desired_width(200.0),
                        )
                        .on_hover_text(format!("Kept in the keychain; {variable} goes first"));
                        let typed = !ui_state.keys[i].trim().is_empty();
                        if ui
                            .add_enabled(!busy && typed, egui::Button::new("Save"))
                            .clicked()
                        {
                            let key = std::mem::take(&mut ui_state.keys[i]);
                            ui_state.keyed = Some((
                                "services.set_key",
                                json!({ "service": service, "key": key.trim() }),
                            ));
                        }
                        if info["key"] == "keychain"
                            && ui.add_enabled(!busy, egui::Button::new("Remove")).clicked()
                        {
                            ui_state.keyed =
                                Some(("services.remove_key", json!({ "service": service })));
                        }
                    });
                    ui.end_row();
                    let field = format!("{service}_key_command");
                    label(ui, "Key Command");
                    let r = ui
                        .add_enabled(
                            !busy,
                            egui::TextEdit::singleline(&mut ui_state.commands[i])
                                .hint_text(format!("Optional, like pass show {service}"))
                                .desired_width(200.0),
                        )
                        .on_hover_text("Its output is the key, when no variable is set");
                    let typed = ui_state.commands[i].trim().to_string();
                    if r.lost_focus() && typed != saved[&field].as_str().unwrap_or_default() {
                        *patch = Some(json!({ "services": { field: typed } }));
                    }
                    ui.end_row();
                    let ask = format!("ask_{service}");
                    let mut on = saved[&ask] != false;
                    label(ui, "Before Sending");
                    if ui
                        .add_enabled(!busy, egui::Checkbox::new(&mut on, "Ask"))
                        .on_hover_text("Shows the length and the estimated cost first")
                        .changed()
                    {
                        *patch = Some(json!({ "services": { ask: on } }));
                    }
                    ui.end_row();
                });
            }
            if state["services"]["xai"]["keychain"] == false {
                dialog::note(
                    ui,
                    "No keychain answers here: use the variables or a key command",
                );
            }
        },
    );
}

fn fillers(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    patch: &mut Option<Value>,
) {
    let lists: super::fillers::Lists =
        serde_json::from_value(state["preferences"]["transcripts"]["fillers"].clone())
            .unwrap_or_default();
    page(
        ui,
        "Filler Words",
        "Words Delete Filler Words takes out, unless a project has its own",
        |ui| {
            ui_state.fillers.scope = super::fillers::Scope::All;
            if let Some(change) =
                super::fillers::editor(ui, &mut ui_state.fillers, None, &lists, busy)
            {
                *patch = Some(change.request().1);
            }
        },
    );
}

fn proxy_folder(preferences: &Value) -> &str {
    preferences["proxies"]["folder"]
        .as_str()
        .unwrap_or_default()
}

struct Folders<'a> {
    dialogs: &'a std::sync::Arc<dyn super::export::Dialogs>,
    project: Option<&'a Path>,
}

fn proxies(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    folders: &Folders,
    patch: &mut Option<Value>,
) {
    let proxies = &state["preferences"]["proxies"];
    page(
        ui,
        "Proxies",
        "Smaller copies the viewer plays; exports read the originals",
        |ui| {
            grid(ui, "preferences-proxies", |ui| {
                let size = label(ui, "Size");
                ui.horizontal(|ui| {
                    let saved = proxies["size"].as_u64().unwrap_or(960);
                    let field = NumberField {
                        label: size.id,
                        saved,
                        range: 16..=7680,
                        unit: Unit::Plain,
                        enabled: !busy,
                        tip: "The longest side of a proxy, automatic or not",
                        refusal: "The proxy size is even, from 16 to 7680",
                    };
                    if let Some(n) = field.show(ui, &mut ui_state.size, &mut ui_state.error) {
                        if n.is_multiple_of(2) {
                            *patch = Some(json!({ "proxies": { "size": n } }));
                        } else {
                            ui_state.error = Some("The proxy size is even, from 16 to 7680".into());
                            ui_state.size = saved.to_string();
                        }
                    }
                    ui.label("px, longest side");
                });
                ui.end_row();
                codec(ui, state, busy, patch);
                folder(
                    ui,
                    proxy_folder(&state["preferences"]),
                    busy,
                    ui_state,
                    folders,
                    patch,
                );
                let mut auto = proxies["auto"] == true;
                label(ui, "Automatic");
                if ui
                    .add_enabled(
                        !busy,
                        egui::Checkbox::new(&mut auto, "Make proxies by themselves, for:"),
                    )
                    .on_hover_text("While Proxies is on in the Media Pool")
                    .changed()
                {
                    *patch = Some(json!({ "proxies": { "auto": auto } }));
                }
                ui.end_row();
                let indent = ui.spacing().icon_width + ui.spacing().icon_spacing;
                let mut seq = proxies["image_sequences"] == true;
                ui.label("");
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    if ui
                        .add_enabled(
                            !busy && auto,
                            egui::Checkbox::new(&mut seq, "Image sequences"),
                        )
                        .on_hover_text("Every picture is decoded on the CPU as it plays")
                        .changed()
                    {
                        *patch = Some(json!({ "proxies": { "image_sequences": seq } }));
                    }
                });
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let label = ui.label("Videos larger than");
                    let saved = |i: usize| proxies["larger_than"][i].as_u64().unwrap_or(1);
                    let field = |i: usize| NumberField {
                        label: label.id,
                        saved: saved(i),
                        range: 1..=100_000,
                        unit: Unit::Plain,
                        enabled: !busy && auto,
                        tip: "Pictures with more pixels than this get a proxy",
                        refusal: "The size needs a width and height of at least 1",
                    };
                    let w = field(0).show(ui, &mut ui_state.larger_w, &mut ui_state.error);
                    ui.label("×");
                    let h = field(1).show(ui, &mut ui_state.larger_h, &mut ui_state.error);
                    if w.is_some() || h.is_some() {
                        let size = [w.unwrap_or(saved(0)), h.unwrap_or(saved(1))];
                        *patch = Some(json!({ "proxies": { "larger_than": size } }));
                    }
                });
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let label = ui.label("Videos above");
                    let field = NumberField {
                        label: label.id,
                        saved: proxies["above_mbps"].as_u64().unwrap_or(1),
                        range: 1..=100_000,
                        unit: Unit::Plain,
                        enabled: !busy && auto,
                        tip: "Files averaging more, such as camera formats the GPU can't decode",
                        refusal: "The bitrate is at least 1 Mb/s",
                    };
                    if let Some(n) = field.show(ui, &mut ui_state.mbps, &mut ui_state.error) {
                        *patch = Some(json!({ "proxies": { "above_mbps": n } }));
                    }
                    ui.label("Mb/s");
                });
                ui.end_row();
            });
        },
    );
}

fn folder(
    ui: &mut egui::Ui,
    saved: &str,
    busy: bool,
    ui_state: &mut PreferencesUi,
    folders: &Folders,
    patch: &mut Option<Value>,
) {
    let label = label(ui, "Folder");
    ui.horizontal(|ui| {
        let r = ui
            .add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut ui_state.folder).desired_width(230.0),
            )
            .labelled_by(label.id)
            .on_hover_text("A folder in each project's folder, or one path for all");
        let typed = ui_state.folder.trim();
        if r.lost_focus() && !busy && typed != saved {
            *patch = Some(json!({ "proxies": { "folder": typed } }));
        }
        let choose = ui
            .add_enabled(
                !busy && ui_state.choosing_folder.is_none(),
                egui::Button::new("Choose…"),
            )
            .on_hover_text("Pick a folder for the proxies of every project");
        if choose.clicked() {
            let dialogs = folders.dialogs.clone();
            let start = match folders.project {
                Some(dir) => dir.join(saved),
                None => PathBuf::from(saved),
            };
            let start = if start.is_dir() {
                start
            } else {
                folders.project.map(Path::to_path_buf).unwrap_or_default()
            };
            ui_state.choosing_folder = Some(Picked::ask(ui.ctx(), move || {
                dialogs.pick_folder("Proxy Folder", &start)
            }));
        }
        let default = Proxies::default().folder;
        if Path::new(saved) != default
            && ui
                .add_enabled(!busy, egui::Button::new("Reset"))
                .on_hover_text("A proxies folder in each project's folder")
                .clicked()
        {
            *patch = Some(json!({ "proxies": { "folder": default } }));
        }
    });
    ui.end_row();
    let shown = match folders.project {
        _ if Path::new(saved).is_absolute() => "Every project's proxies are here".to_string(),
        Some(dir) => format!("This project's: {}", dir.join(saved).display()),
        None => "In each project's folder".to_string(),
    };
    ui.label("");
    ui.add(egui::Label::new(dim(shown)).wrap());
    ui.end_row();
}

fn codec(ui: &mut egui::Ui, state: &Value, busy: bool, patch: &mut Option<Value>) {
    let current = state["preferences"]["proxies"]["codec"]
        .as_str()
        .unwrap_or("auto");
    let choices = state["proxy_codecs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let made = |c: &Value| {
        let uses = c["uses"].as_str().unwrap_or_default();
        let (codec, by) = uses.split_once(", ").unwrap_or((uses, ""));
        (codec.to_string(), by.to_string())
    };
    let name = |c: &Value| {
        let (codec, _) = made(c);
        match c["codec"].as_str().unwrap_or_default() {
            "auto" if codec.is_empty() => "Automatic".to_string(),
            "auto" => format!("Automatic ({codec})"),
            _ if codec.is_empty() => c["codec"].as_str().unwrap_or_default().to_string(),
            _ => codec,
        }
    };
    let how = |c: &Value| {
        let (_, by) = made(c);
        let plays = match c["plays_on"].as_str() {
            Some(decoder) => format!("played on the GPU ({decoder})"),
            None => "played on the CPU".to_string(),
        };
        match by.as_str() {
            "" => "Nothing here makes it".to_string(),
            by => format!("Made by {by}, {plays}"),
        }
    };
    let selected = choices.iter().find(|c| c["codec"] == current);
    label(ui, "Codec");
    ui.add_enabled_ui(!busy, |ui| {
        egui::ComboBox::from_id_salt("proxy-codec")
            .selected_text(selected.map_or(current.to_string(), name))
            .show_ui(ui, |ui| {
                for c in &choices {
                    if ui
                        .selectable_label(c["codec"] == current, name(c))
                        .on_hover_text(how(c))
                        .clicked()
                    {
                        *patch = Some(json!({ "proxies": { "codec": c["codec"] } }));
                    }
                }
            })
            .response
            .on_hover_text("Automatic: the first the GPU both makes and plays");
    });
    ui.end_row();
    if let Some(c) = selected {
        ui.label("");
        ui.add(egui::Label::new(dim(how(c))).wrap());
        ui.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::{about_gb, gb, mb_of};

    #[test]
    fn whole_gigabytes_show_without_decimals() {
        assert_eq!(gb(8192), "8");
    }

    #[test]
    fn a_part_of_a_gigabyte_shows_two_decimals_at_most() {
        assert_eq!(gb(1536), "1.5");
    }

    #[test]
    fn a_large_size_reads_in_whole_gigabytes() {
        assert_eq!(about_gb(31569), "31");
    }

    #[test]
    fn a_comma_reads_as_a_decimal_point() {
        assert_eq!(mb_of("1,5"), Some(1536));
    }
}
