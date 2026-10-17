use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_core::preferences::Proxies;

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
}

const LABEL: f32 = 170.0;

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

fn card(ui: &mut egui::Ui, title: &str, about: &str, add: impl FnOnce(&mut egui::Ui)) {
    let c = theme::colors();
    egui::Frame::new()
        .fill(c.faint)
        .stroke(egui::Stroke::new(1.0, c.line))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(16.0));
            ui.label(RichText::new(about).color(c.text_dim));
            ui.add_space(8.0);
            add(ui);
        });
    ui.add_space(10.0);
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

    pub(super) fn preferences_window(&mut self, ctx: &egui::Context) {
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
        let height = ctx.content_rect().height() * 0.85;
        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 640.0, height))
            .open(&mut open)
            .show(ctx, |ui| {
                let Some(state) = ui_state.state.clone() else {
                    match &ui_state.error {
                        Some(e) => ui.label(RichText::new(e).color(theme::colors().error)),
                        None => ui.spinner(),
                    };
                    return;
                };
                let busy = ui_state.ask.is_some();
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .min_scrolled_height(height)
                    .show(ui, |ui| {
                        ui.set_width(620.0);
                        appearance(ui, &state, busy, ui_state, &dialogs, &mut patch);
                        gpu(ui, &state, busy, display.as_deref(), &mut patch);
                        memory(ui, &state, busy, ui_state, &mut patch);
                        sound(ui, &state, busy, ui_state, &mut patch);
                        thumbnails(ui, &state, busy, ui_state, &mut patch);
                        let folders = Folders {
                            dialogs: &dialogs,
                            project: project.as_deref(),
                        };
                        proxies(ui, &state, busy, ui_state, &folders, &mut patch);
                        if let Some(e) = &ui_state.error {
                            ui.label(RichText::new(e).color(theme::colors().error));
                        }
                        let path = state["path"].as_str().unwrap_or_default();
                        ui.horizontal(|ui| {
                            if ui
                                .button("Show in Folder")
                                .on_hover_text("The preferences file, which you can also edit")
                                .clicked()
                            {
                                super::file_manager::show(std::path::Path::new(path));
                            }
                            ui.label(dim(path).small());
                        });
                    });
            });
        ui_state.open = open;
        if let Some(p) = patch {
            ui_state.ask = Some(Ask::send(&self.live, ctx, "preferences.set", p));
        }
    }
}

fn appearance(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    dialogs: &std::sync::Arc<dyn super::export::Dialogs>,
    patch: &mut Option<Value>,
) {
    card(ui, "Appearance", "How vidcut looks.", |ui| {
        grid(ui, "preferences-appearance", |ui| {
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
            ui.label("Theme");
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
                    .on_hover_text(
                        "Choose a theme file (.toml). It is copied into the themes folder and \
                         shown",
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
        });
    });
}

fn gpu(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    display: Option<&str>,
    patch: &mut Option<Value>,
) {
    card(
        ui,
        "GPU",
        "The graphics card decodes, scales and encodes video in the viewer, exports and proxies.",
        |ui| {
            grid(ui, "preferences-gpu", |ui| {
                let mut on = state["preferences"]["gpu"]["acceleration"] != false;
                ui.label("Acceleration");
                if ui
                    .add_enabled(
                        !busy,
                        egui::Checkbox::new(&mut on, "Decode, scale and encode on the GPU"),
                    )
                    .on_hover_text(
                        "Turn off to do everything on the CPU, if a graphics driver causes \
                         trouble",
                    )
                    .changed()
                {
                    *patch = Some(json!({ "gpu": { "acceleration": on } }));
                }
                ui.end_row();
                let mut decoding = state["preferences"]["gpu"]["decoding"] != false;
                ui.label("Decoding");
                if ui
                    .add_enabled(
                        !busy && on,
                        egui::Checkbox::new(&mut decoding, "Decode on the GPU too"),
                    )
                    .on_hover_text(
                        "Decode H.264, HEVC, AV1 and VP9 on the graphics card in the viewer, \
                         exports and proxies, and all but H.264 in the timeline's pictures. Turn \
                         off where a small GPU decodes more slowly than the CPU: the CPU decodes \
                         and scales, and the GPU only encodes",
                    )
                    .changed()
                {
                    *patch = Some(json!({ "gpu": { "decoding": decoding } }));
                }
                ui.end_row();
                ui.label("GPU in use");
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
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut pick, String::new(), &automatic);
                for g in gpus {
                    let id = g["id"].as_str().unwrap_or_default();
                    ui.selectable_value(&mut pick, id.to_string(), name(g));
                }
            })
            .response
            .on_hover_text(
                "The graphics card that decodes and encodes. Automatic: NVIDIA's when there is \
                 one, else the first other. A card is known by its model and its slot, so the \
                 choice holds on every system this computer starts",
            );
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
    card(
        ui,
        "Memory",
        "Decoded pictures kept around the playhead, so stepping and playing back wait less.",
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
                    Some(m) if gpu_decodes => {
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
                        "Viewer pictures",
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
                    let label = ui.label(name).on_hover_text(tip);
                    ui.horizontal(|ui| {
                        let custom = saved != 0;
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
                            ui.add(egui::Label::new(dim(auto)).wrap());
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
    card(ui, "Sound", "Where the viewer's sound plays.", |ui| {
        grid(ui, "preferences-sound", |ui| {
            let chosen = state["preferences"]["player"]["audio_device"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            ui.label("Output");
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
                                "The system's default follows the system. A chosen device \
                                 that isn't connected plays on the default",
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

fn thumbnails(
    ui: &mut egui::Ui,
    state: &Value,
    busy: bool,
    ui_state: &mut PreferencesUi,
    patch: &mut Option<Value>,
) {
    let thumbs = &state["preferences"]["thumbnails"];
    let cache = &state["thumbnail_cache"];
    card(
        ui,
        "Thumbnails",
        "The Media Pool's thumbnails and the pictures in the timeline's clips.",
        |ui| {
            grid(ui, "preferences-thumbnails", |ui| {
                let mut disk = thumbs["disk_cache"] == true;
                ui.label("Disk cache");
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut disk, "Keep them on disk"))
                    .on_hover_text(format!(
                        "Projects open with their pictures shown, without decoding every clip \
                         again ({}). Turning this off empties the cache.",
                        cache["path"].as_str().unwrap_or_default()
                    ))
                    .changed()
                {
                    *patch = Some(json!({ "thumbnails": { "disk_cache": disk } }));
                }
                ui.end_row();
                let label = ui.label("Size limit");
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
                    )));
                });
                ui.end_row();
            });
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
    card(
        ui,
        "Proxies",
        "Smaller copies the viewer plays instead of heavy media. Exports always read the \
         originals.",
        |ui| {
            grid(ui, "preferences-proxies", |ui| {
                let label = ui.label("Size");
                ui.horizontal(|ui| {
                    let saved = proxies["size"].as_u64().unwrap_or(960);
                    let field = NumberField {
                        label: label.id,
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
                ui.label("Automatic");
                if ui
                    .add_enabled(
                        !busy,
                        egui::Checkbox::new(&mut auto, "Make proxies by themselves, for:"),
                    )
                    .on_hover_text(
                        "While Proxies is on in the Media Pool: for the media you import that \
                         these rules pick, and for the pool's when you turn Proxies on",
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
                        tip: "Files averaging more get a proxy: camera and editing formats the \
                              GPU can't decode",
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
    let label = ui.label("Folder");
    ui.horizontal(|ui| {
        let r = ui
            .add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut ui_state.folder).desired_width(280.0),
            )
            .labelled_by(label.id)
            .on_hover_text(
                "Where new proxies go: a folder in the project's folder, or an absolute path. \
                 Every project shares an absolute one, so a file used in several projects gets \
                 one proxy. Proxies already made stay where they are",
            );
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
        _ if Path::new(saved).is_absolute() => "Every project's new proxies go here".to_string(),
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
    ui.label("Codec");
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
            .on_hover_text(
                "Automatic: the first of DNxHR LB, ProRes Proxy, AV1, HEVC and H.264 that the GPU \
                 makes and plays. Else the first the GPU plays, else the first it makes, else \
                 DNxHR LB in software. ProRes Proxy and DNxHR LB make every frame a keyframe: a \
                 jump decodes one frame instead of up to 60, but the files are many times \
                 larger. A Mac's GPU and NVIDIA's make ProRes Proxy and show it from GPU memory. \
                 AV1 files are the smallest. Made in software (SVT-AV1), they take longer than \
                 H.264 with x264",
            );
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
