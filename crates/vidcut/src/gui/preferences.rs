use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use serde_json::{Value, json};

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
    after_secs: String,
    folders: [String; 2],
    player_memory: String,
    video_memory: String,
    audio_outputs: Option<Result<Vec<vidcut_av::player::OutputDevice>, String>>,
    listing:
        Option<std::sync::mpsc::Receiver<Result<Vec<vidcut_av::player::OutputDevice>, String>>>,
    importing: Option<Picked>,
    choosing_folder: Option<(Folder, Picked)>,
    confirm_reset: bool,
    reset_saved: bool,
    reset_keep: bool,
    keys: [String; 2],
    keyed: Option<(&'static str, Value)>,
    resetting: Option<Ask>,
    forgotten: Option<super::workspace::ViewState>,
    fillers: super::fillers::FillerEditor,
    page: Page,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Folder {
    Files,
    RenderCache,
}

impl Folder {
    const ALL: [Self; 2] = [Self::Files, Self::RenderCache];

    fn key(self) -> &'static str {
        match self {
            Self::Files => "folder",
            Self::RenderCache => "render_cache",
        }
    }

    fn saved(self, preferences: &Value) -> &str {
        preferences["playback_files"][self.key()]
            .as_str()
            .unwrap_or_default()
    }

    fn patch(self, path: &Path) -> Value {
        json!({ "playback_files": { self.key(): path } })
    }
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
    refusal: &'a str,
}

impl NumberField<'_> {
    fn show(self, ui: &mut egui::Ui, text: &mut String, error: &mut Option<String>) -> Option<u64> {
        let r = ui
            .add_enabled(
                self.enabled,
                egui::TextEdit::singleline(text).desired_width(56.0),
            )
            .labelled_by(self.label);
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
                    ui_state.limit = match p["cache"]["limit"].as_str() {
                        Some("gb") => n(&p["cache"]["gb"]),
                        _ => n(&p["cache"]["percent"]),
                    };
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
                    ui_state.after_secs = n(&p["render_cache"]["after_secs"]);
                    ui_state.folders = Folder::ALL.map(|f| f.saved(p).to_string());
                    ui_state.state = Some(r.result);
                    ui_state.error = None;
                }
                Err(e) => {
                    if let Some(state) = &ui_state.state {
                        let p = &state["preferences"];
                        ui_state.folders = Folder::ALL.map(|f| f.saved(p).to_string());
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
        if let Some((which, picked)) = ui_state
            .choosing_folder
            .as_ref()
            .and_then(|(which, p)| p.take().map(|picked| (*which, picked)))
        {
            ui_state.choosing_folder = None;
            if let Some(path) = picked {
                patch = Some(which.patch(&path));
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
                        if ui.button("Show in Folder").clicked() {
                            super::file_manager::show(&crate::preferences::path());
                        }
                        reset = ui
                            .add_enabled(
                                ui_state.ask.is_none(),
                                egui::Button::new("Reset Preferences"),
                            )
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
                                    .selectable_label(
                                        ui_state.page == page,
                                        format!("{icon}  {name}"),
                                    )
                                    .clicked()
                                {
                                    ui_state.page = page;
                                }
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_width(PAGE);
                        ui.set_min_height(height);
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
                                    Page::Gpu => {
                                        gpu(ui, &state, busy, display.as_deref(), &mut patch)
                                    }
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
                                    Page::Fillers => {
                                        fillers(ui, &state, busy, ui_state, &mut patch)
                                    }
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
                                let shown = match t["error"].as_str() {
                                    Some(e) => format!("{} · {e}", title(name)),
                                    None => title(name),
                                };
                                let r = ui.add_enabled(
                                    t["error"].is_null(),
                                    egui::Button::selectable(pick == name, shown),
                                );
                                if r.clicked() {
                                    pick = name.to_string();
                                }
                            }
                        });
                });
                if ui
                    .add_enabled(
                        !busy && ui_state.importing.is_none(),
                        egui::Button::new("Import…"),
                    )
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
                if ui.button("Show in Folder").clicked() {
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
        None if state["gpu_automatic"].is_object() => {
            format!("{chosen} (not here: {})", name(&state["gpu_automatic"]))
        }
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
            });
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
                let video_set = match player["video_memory_mb"].as_u64() {
                    Some(0) | None => video_auto,
                    Some(mb) => mb,
                };
                let viewer_set = match player["memory_mb"].as_u64() {
                    Some(0) | None => auto_mb,
                    Some(mb) => mb,
                };
                let viewer_about = state["player_memory_lent_mb"]
                    .as_u64()
                    .filter(|&mb| mb < viewer_set)
                    .map(|mb| format!("{} GB now, while AI Super Scale works", about_gb(mb)));
                let video_about = if gpu_decodes {
                    state["player_video_memory_lent_mb"]
                        .as_u64()
                        .filter(|&mb| mb < video_set)
                        .map(|mb| format!("{} GB now, while AI Super Scale works", about_gb(mb)))
                } else {
                    Some("Not used while the CPU decodes".to_string())
                };
                for (key, name, text, auto, about) in [
                    (
                        "memory_mb",
                        "Viewer Pictures",
                        &mut ui_state.player_memory,
                        viewer,
                        viewer_about,
                    ),
                    (
                        "video_memory_mb",
                        "Pictures on the GPU",
                        &mut ui_state.video_memory,
                        video,
                        video_about,
                    ),
                ] {
                    let saved = player[key].as_u64().unwrap_or(0);
                    let label = label(ui, name);
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
                    let usual = devices.iter().find(|d| d.default);
                    let default = usual.map_or(String::new(), |d| format!(" ({})", d.name));
                    let plays = usual.map_or("the default", |d| d.name.as_str());
                    let shown = match devices.iter().find(|d| d.id == chosen) {
                        _ if chosen.is_empty() => format!("System default{default}"),
                        Some(d) => d.name.clone(),
                        None => format!("{chosen} (not connected: {plays} plays)"),
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
                            });
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
    let saved = &state["preferences"]["cache"];
    let held = &state["cache"];
    page(
        ui,
        "Cache",
        "Kept here, so it isn't made, downloaded or paid for again",
        |ui| {
            grid(ui, "preferences-cache", |ui| {
                folder_row(ui, "Speech Models", &held["models"], "files");
                folder_row(ui, "Paid Replies", &held["replies"], "replies");
                folder_row(ui, "Other Files", held, "files");
                let fixed = saved["limit"] == "gb";
                let named = label(ui, "Size Limit");
                ui.horizontal(|ui| {
                    dialog::switch(ui, |ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::selectable(!fixed, "Share of Disk"))
                            .clicked()
                            && fixed
                        {
                            *patch = Some(json!({ "cache": { "limit": "percent" } }));
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::selectable(fixed, "Fixed Size"))
                            .clicked()
                            && !fixed
                        {
                            let gb = saved["gb"].as_u64().unwrap_or_else(|| {
                                (held["chosen_mb"].as_u64().unwrap_or(0) / 1024).max(1)
                            });
                            *patch = Some(json!({ "cache": { "limit": "gb", "gb": gb } }));
                        }
                    });
                    let (key, range, unit, refusal) = if fixed {
                        ("gb", 1..=u64::MAX, "GB", "The size limit is at least 1 GB")
                    } else {
                        ("percent", 1..=100, "%", "The share is 1 to 100%")
                    };
                    let field = NumberField {
                        label: named.id,
                        saved: saved[key].as_u64().unwrap_or(1),
                        range,
                        unit: Unit::Plain,
                        enabled: !busy,
                        refusal,
                    };
                    if let Some(n) = field.show(ui, &mut ui_state.limit, &mut ui_state.error) {
                        *patch = Some(json!({ "cache": { key: n } }));
                    }
                    ui.label(unit);
                });
                ui.end_row();
                if let Some(limit) = held["limit_mb"].as_u64() {
                    let now = format!("{} GB now", about_gb(limit));
                    ui.label("");
                    ui.label(dim(match held["chosen_mb"].as_u64() {
                        Some(chosen) if limit < chosen => format!(
                            "{now}, keeping {}% of the disk free",
                            held["keep_free_percent"]
                        ),
                        _ => now,
                    }));
                    ui.end_row();
                } else {
                    ui.label("");
                    ui.label(dim("The disk's size can't be read, so nothing is removed"));
                    ui.end_row();
                }
                let mut disk = state["preferences"]["thumbnails"]["disk_cache"] == true;
                label(ui, "Thumbnails");
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut disk, "Keep them on disk"))
                    .changed()
                {
                    *patch = Some(json!({ "thumbnails": { "disk_cache": disk } }));
                }
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
                        );
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
                    let ask = format!("ask_{service}");
                    let mut on = saved[&ask] != false;
                    label(ui, "Before Sending");
                    if ui
                        .add_enabled(!busy, egui::Checkbox::new(&mut on, "Ask"))
                        .changed()
                    {
                        *patch = Some(json!({ "services": { ask: on } }));
                    }
                    ui.end_row();
                });
            }
            if state["services"]["xai"]["keychain"] == false {
                dialog::note(ui, "No keychain answers here: use the variables");
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
        "Proxies and Render Cache",
        "Copies the viewer plays for speed; exports read the originals",
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
                folder(ui, Folder::Files, state, busy, ui_state, folders, patch);
                let mut auto = proxies["auto"] == true;
                label(ui, "Automatic");
                if ui
                    .add_enabled(
                        !busy,
                        egui::Checkbox::new(&mut auto, "Make proxies by themselves, for:"),
                    )
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
                        refusal: "The bitrate is at least 1 Mb/s",
                    };
                    if let Some(n) = field.show(ui, &mut ui_state.mbps, &mut ui_state.error) {
                        *patch = Some(json!({ "proxies": { "above_mbps": n } }));
                    }
                    ui.label("Mb/s");
                });
                ui.end_row();
            });
            ui.add_space(10.0);
            ui.label(RichText::new("Render Cache").strong());
            render_cache(ui, state, busy, ui_state, folders, patch);
        },
    );
}

fn render_cache(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    folders: &Folders,
    patch: &mut Option<Value>,
) {
    let saved = &state["preferences"]["render_cache"];
    grid(ui, "preferences-render-cache", |ui| {
        let named = label(ui, "Background Render");
        ui.horizontal(|ui| {
            let mut on = saved["background"] == true;
            if ui
                .add_enabled(!busy, egui::Checkbox::new(&mut on, "After"))
                .changed()
            {
                *patch = Some(json!({ "render_cache": { "background": on } }));
            }
            let field = NumberField {
                label: named.id,
                saved: saved["after_secs"].as_u64().unwrap_or(5),
                range: 1..=3600,
                unit: Unit::Plain,
                enabled: !busy && on,
                refusal: "The time is 1 to 3600 seconds",
            };
            if let Some(n) = field.show(ui, &mut ui_state.after_secs, &mut ui_state.error) {
                *patch = Some(json!({ "render_cache": { "after_secs": n } }));
            }
            ui.label("seconds without activity");
        });
        ui.end_row();
        label(ui, "User Mode");
        ui.horizontal(|ui| {
            for (key, text) in [
                ("transitions", "Transitions"),
                ("composites", "Composites"),
                ("ai", "AI"),
            ] {
                let mut on = saved[key] == true;
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut on, text))
                    .changed()
                {
                    *patch = Some(json!({ "render_cache": { key: on } }));
                }
            }
        });
        ui.end_row();
    });
    let current = saved["codec"].as_str().unwrap_or("auto");
    let own = !Folder::RenderCache.saved(&state["preferences"]).is_empty();
    dialog::advanced(
        ui,
        "render-cache-advanced",
        current != "auto" || own,
        |ui| {
            grid(ui, "preferences-render-cache-advanced", |ui| {
                cache_codec(ui, state, busy, patch);
                folder(
                    ui,
                    Folder::RenderCache,
                    state,
                    busy,
                    ui_state,
                    folders,
                    patch,
                );
            });
        },
    );
}

fn cache_codec(ui: &mut egui::Ui, state: &Value, busy: bool, patch: &mut Option<Value>) {
    let current = state["preferences"]["render_cache"]["codec"]
        .as_str()
        .unwrap_or("auto");
    let choices = state["cache_codecs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let name = |c: &Value| c["name"].as_str().unwrap_or_default().to_string();
    let selected = choices.iter().find(|c| c["codec"] == current);
    label(ui, "Codec");
    ui.add_enabled_ui(!busy, |ui| {
        egui::ComboBox::from_id_salt("render-cache-codec")
            .selected_text(selected.map_or(current.to_string(), name))
            .show_ui(ui, |ui| {
                for c in &choices {
                    if ui
                        .selectable_label(c["codec"] == current, name(c))
                        .clicked()
                    {
                        *patch = Some(json!({ "render_cache": { "codec": c["codec"] } }));
                    }
                }
            });
    });
    ui.end_row();
    let line = match (current, selected.and_then(|c| c["uses"].as_str())) {
        ("auto", _) => None,
        (_, Some(uses)) => Some(dim(format!("Made by {}", by(uses)))),
        (_, None) => Some(
            RichText::new("Nothing here makes it: the cache stays empty")
                .color(ui.visuals().warn_fg_color),
        ),
    };
    if let Some(line) = line {
        ui.label("");
        ui.add(egui::Label::new(line).wrap());
        ui.end_row();
    }
}

fn by(uses: &str) -> &str {
    uses.split_once(", ").map_or(uses, |(_, by)| by)
}

fn folder(
    ui: &mut egui::Ui,
    which: Folder,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    folders: &Folders,
    patch: &mut Option<Value>,
) {
    let saved = which.saved(&state["preferences"]);
    let own = !Folder::RenderCache.saved(&state["preferences"]).is_empty();
    let (hint, title, every) = match which {
        Folder::Files if own => (
            "Project Folder",
            "Folder",
            "Every project's proxies are here",
        ),
        Folder::Files => (
            "Project Folder",
            "Folder",
            "Every project's proxies and render cache are here",
        ),
        Folder::RenderCache => (
            "Folder Above",
            "Render Cache Folder",
            "Every project's render cache is here",
        ),
    };
    let label = label(ui, "Folder");
    let text = &mut ui_state.folders[which as usize];
    ui.horizontal(|ui| {
        let r = ui
            .add_enabled(
                !busy,
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .desired_width(230.0),
            )
            .labelled_by(label.id);
        let typed = text.trim();
        if r.lost_focus() && !busy && typed != saved {
            *patch = Some(which.patch(Path::new(typed)));
        }
        let choose = ui.add_enabled(
            !busy && ui_state.choosing_folder.is_none(),
            egui::Button::new("Choose…"),
        );
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
            let picked = Picked::ask(ui.ctx(), move || dialogs.pick_folder(title, &start));
            ui_state.choosing_folder = Some((which, picked));
        }
        if !saved.is_empty() && ui.add_enabled(!busy, egui::Button::new("Reset")).clicked() {
            *patch = Some(which.patch(Path::new("")));
        }
    });
    ui.end_row();
    if which == Folder::RenderCache && saved.is_empty() {
        return;
    }
    let shown = match folders.project {
        _ if Path::new(saved).is_absolute() => every.to_string(),
        Some(dir) if saved.is_empty() => format!("This project's: {}", dir.display()),
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
                        .clicked()
                    {
                        *patch = Some(json!({ "proxies": { "codec": c["codec"] } }));
                    }
                }
            });
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
