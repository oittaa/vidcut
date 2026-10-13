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
    /// The number fields as typed: the thumbnail cache limit and the
    /// timeline's picture memory (MB), the proxy rules' picture size and
    /// bitrate, and the proxy size.
    limit: String,
    memory: String,
    /// The player's memory and video memory (MB, 0: automatic).
    player_memory: String,
    video_memory: String,
    larger_w: String,
    larger_h: String,
    mbps: String,
    size: String,
    /// The system's audio devices, listed on a thread as the window opens
    /// (it can take a while), or why they couldn't be.
    audio_outputs: Option<Result<Vec<vidcut_av::player::OutputDevice>, String>>,
    listing:
        Option<std::sync::mpsc::Receiver<Result<Vec<vidcut_av::player::OutputDevice>, String>>>,
}

/// A number typed into a field: digits only, sent on Enter or when the
/// field loses focus.
struct NumberField<'a> {
    /// The label it is read by (accessibility, tests).
    label: egui::Id,
    /// The value in the preferences now.
    saved: u64,
    range: std::ops::RangeInclusive<u64>,
    enabled: bool,
    tip: &'a str,
    /// Shown when the number is outside `range`.
    refusal: &'a str,
}

impl NumberField<'_> {
    /// The number once it is sent and differs from `saved`. A number outside
    /// `range` is refused with `refusal`, and the field goes back to `saved`.
    fn show(self, ui: &mut egui::Ui, text: &mut String, error: &mut Option<String>) -> Option<u64> {
        let r = ui
            .add_enabled(
                self.enabled,
                egui::TextEdit::singleline(text).desired_width(56.0),
            )
            .labelled_by(self.label)
            .on_hover_text(self.tip);
        if r.changed() {
            text.retain(|c| c.is_ascii_digit());
        }
        if !r.lost_focus() {
            return None;
        }
        // egui reports a focus given up mid-frame (Enter, a click elsewhere)
        // in that frame and the next one: the number goes once.
        let frame = ui.ctx().cumulative_frame_nr();
        let key = r.id.with("lost focus");
        let before = ui.data_mut(|d| d.get_temp::<u64>(key));
        ui.data_mut(|d| d.insert_temp(key, frame));
        if before.is_some_and(|b| b + 1 == frame) {
            return None;
        }
        match text.parse::<u64>() {
            Ok(n) if self.range.contains(&n) => (n != self.saved).then_some(n),
            _ => {
                *error = Some(self.refusal.to_string());
                *text = self.saved.to_string();
                None
            }
        }
    }
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
                    ui_state.memory = n(&p["thumbnails"]["timeline_memory_mb"]);
                    ui_state.player_memory = n(&p["player"]["memory_mb"]);
                    ui_state.video_memory = n(&p["player"]["video_memory_mb"]);
                    // The timeline's pictures follow at once.
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
                let proxies = &state["preferences"]["proxies"];
                let cache = &state["thumbnail_cache"];
                let busy = ui_state.ask.is_some();
                ui.label(RichText::new("GPU").strong());
                egui::Grid::new("preferences-gpu")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let mut on = state["preferences"]["gpu"]["acceleration"] != false;
                        ui.label("Acceleration");
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Checkbox::new(&mut on, "Decode, scale and encode on the GPU"),
                            )
                            .on_hover_text(
                                "Use the graphics card for the viewer, exports and proxies. \
                                 Turn off to do everything on the CPU, if a graphics driver \
                                 causes trouble",
                            )
                            .changed()
                        {
                            patch = Some(json!({ "gpu": { "acceleration": on } }));
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
                                "Decode H.264, HEVC, AV1 and VP9 on the graphics card in the \
                                 viewer, exports and proxies, and all but H.264 in the \
                                 timeline's pictures. Turn off where a small GPU decodes more \
                                 slowly than the CPU: the CPU decodes and scales, and the GPU \
                                 only encodes",
                            )
                            .changed()
                        {
                            patch = Some(json!({ "gpu": { "decoding": decoding } }));
                        }
                        ui.end_row();
                        // The GPU that decodes and encodes: none to choose
                        // where none is listed (macOS).
                        let gpus = state["gpus"].as_array().cloned().unwrap_or_default();
                        if !gpus.is_empty() {
                            let name = |g: &Value| g["name"].as_str().unwrap_or_default().to_string();
                            let automatic = match state["gpu_automatic"].is_object() {
                                true => format!("Automatic ({})", name(&state["gpu_automatic"])),
                                false => "Automatic".to_string(),
                            };
                            let chosen = state["preferences"]["gpu"]["device"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string();
                            // A card moved to another slot is found by its
                            // model: picking it again saves its new slot.
                            let found = state["gpu_chosen"]["id"].as_str().map(String::from);
                            let shown = match &found {
                                _ if chosen.is_empty() => automatic.clone(),
                                Some(_) => name(&state["gpu_chosen"]),
                                None => format!("{chosen} (not here: automatic)"),
                            };
                            let current = found.unwrap_or_else(|| chosen.clone());
                            let mut pick = current.clone();
                            ui.label("GPU");
                            ui.add_enabled_ui(!busy && on, |ui| {
                                egui::ComboBox::from_id_salt("preferences-gpu-device")
                                    .selected_text(shown)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut pick, String::new(), &automatic);
                                        for g in &gpus {
                                            let id = g["id"].as_str().unwrap_or_default();
                                            ui.selectable_value(&mut pick, id.to_string(), name(g));
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "The graphics card that decodes and encodes, in the                                          viewer, exports and proxies. Automatic: NVIDIA's when                                          there is one, else the first other. A card is known by                                          its model and its slot, so the choice holds on every                                          system this computer starts",
                                    );
                            });
                            if pick != current {
                                patch = Some(json!({ "gpu": { "device": pick } }));
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.label(RichText::new("Player").strong());
                egui::Grid::new("preferences-player")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let player = &state["preferences"]["player"];
                        let auto_mb = state["player_memory_auto_mb"].as_u64().unwrap_or(1024);
                        for (key, name, saved, text, tip) in [
                            (
                                "memory_mb",
                                "Memory",
                                player["memory_mb"].as_u64().unwrap_or(0),
                                &mut ui_state.player_memory,
                                format!(
                                    "Decoded pictures kept around the playhead, so steps and \
                                     playing back wait less. 0: automatic, the larger of 1 GB \
                                     and half the computer's memory ({auto_mb} MB here)"
                                ),
                            ),
                            (
                                "video_memory_mb",
                                "Video memory",
                                player["video_memory_mb"].as_u64().unwrap_or(0),
                                &mut ui_state.video_memory,
                                "Pictures the GPU decoded, kept on the GPU. 0: automatic, \
                                 half the GPU's memory"
                                    .to_string(),
                            ),
                        ] {
                            let label = ui.label(name);
                            ui.horizontal(|ui| {
                                let field = NumberField {
                                    label: label.id,
                                    saved,
                                    range: 0..=100_000_000,
                                    enabled: !busy,
                                    tip: &tip,
                                    refusal: "The player's memory is 0 (automatic) or at least \
                                              64 MB",
                                };
                                if let Some(mb) = field.show(ui, text, &mut ui_state.error) {
                                    patch = Some(json!({ "player": { key: mb } }));
                                }
                                ui.label("MB (0: automatic)");
                            });
                            ui.end_row();
                        }
                        if let Some(listed) =
                            ui_state.listing.as_ref().and_then(|r| r.try_recv().ok())
                        {
                            ui_state.audio_outputs = Some(listed);
                            ui_state.listing = None;
                        }
                        let chosen = player["audio_device"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        ui.label("Sound");
                        match &ui_state.audio_outputs {
                            None => {
                                ui.spinner();
                            }
                            Some(Err(e)) => {
                                ui.label(RichText::new(e).color(theme::ERROR));
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
                                                ui.selectable_value(
                                                    &mut pick,
                                                    d.id.clone(),
                                                    &d.name,
                                                );
                                            }
                                        })
                                        .response
                                        .on_hover_text(
                                            "The device the player plays sound on. The system's \
                                             default follows the system; a chosen device that \
                                             isn't connected plays on the default",
                                        );
                                });
                                if pick != chosen {
                                    patch = Some(json!({ "player": { "audio_device": pick } }));
                                }
                            }
                        }
                        ui.end_row();
                    });
                ui.add_space(8.0);
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
                        ui.horizontal(|ui| {
                            let field = NumberField {
                                label: label.id,
                                saved: thumbs["cache_limit_mb"].as_u64().unwrap_or(1),
                                range: 1..=1_000_000,
                                enabled: !busy && disk,
                                tip: "Above it, the least recently used thumbnails go first",
                                refusal: "The size limit is 1 to 1000000 MB",
                            };
                            if let Some(mb) =
                                field.show(ui, &mut ui_state.limit, &mut ui_state.error)
                            {
                                patch = Some(json!({ "thumbnails": { "cache_limit_mb": mb } }));
                            }
                            ui.label("MB");
                        });
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
                        let label = ui.label("Timeline memory");
                        ui.horizontal(|ui| {
                            let field = NumberField {
                                label: label.id,
                                saved: thumbs["timeline_memory_mb"].as_u64().unwrap_or(16),
                                range: 16..=1_000_000,
                                enabled: !busy,
                                tip: "The timeline's filmstrip pictures kept in memory: above \
                                      it, those shown least recently go first",
                                refusal: "The timeline memory is 16 to 1000000 MB",
                            };
                            if let Some(mb) =
                                field.show(ui, &mut ui_state.memory, &mut ui_state.error)
                            {
                                patch = Some(json!({ "thumbnails": { "timeline_memory_mb": mb } }));
                            }
                            ui.label("MB");
                        });
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.label(RichText::new("Proxies").strong());
                egui::Grid::new("preferences-proxies")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let label = ui.label("Size");
                        ui.horizontal(|ui| {
                            let saved = proxies["size"].as_u64().unwrap_or(960);
                            let field = NumberField {
                                label: label.id,
                                saved,
                                range: 16..=7680,
                                enabled: !busy,
                                tip: "The longest side of a proxy, automatic or not",
                                refusal: "The proxy size is even, from 16 to 7680",
                            };
                            if let Some(n) = field.show(ui, &mut ui_state.size, &mut ui_state.error)
                            {
                                if n.is_multiple_of(2) {
                                    patch = Some(json!({ "proxies": { "size": n } }));
                                } else {
                                    ui_state.error =
                                        Some("The proxy size is even, from 16 to 7680".into());
                                    ui_state.size = saved.to_string();
                                }
                            }
                            ui.label("px, longest side");
                        });
                        ui.end_row();
                        // Each choice says what it makes proxies with here.
                        ui.label("Codec");
                        let current = proxies["codec"].as_str().unwrap_or("auto");
                        let choices = state["proxy_codecs"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        let text = |c: &Value| {
                            let uses = c["uses"].as_str().unwrap_or("nothing that works here");
                            match c["codec"].as_str().unwrap_or_default() {
                                "auto" => format!("Automatic: {uses}"),
                                "av1" => format!("AV1: {}", uses.trim_start_matches("AV1, ")),
                                "h264" => format!("H.264: {}", uses.trim_start_matches("H.264, ")),
                                other => format!("{other}: {uses}"),
                            }
                        };
                        let shown = choices
                            .iter()
                            .find(|c| c["codec"] == current)
                            .map(text)
                            .unwrap_or_else(|| current.to_string());
                        ui.add_enabled_ui(!busy, |ui| {
                            egui::ComboBox::from_id_salt("proxy-codec")
                                .selected_text(shown)
                                .show_ui(ui, |ui| {
                                    for c in &choices {
                                        if ui
                                            .selectable_label(c["codec"] == current, text(c))
                                            .clicked()
                                        {
                                            patch =
                                                Some(json!({ "proxies": { "codec": c["codec"] } }));
                                        }
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Automatic: AV1 on a hardware encoder when there is one, \
                                     else H.264 with x264, which stays quick on old computers. \
                                     AV1 files are smaller; making them in software (SVT-AV1) is \
                                     slower than x264",
                                );
                        });
                        ui.end_row();
                        let mut auto = proxies["auto"] == true;
                        ui.label("Automatic");
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Checkbox::new(&mut auto, "Make proxies by themselves"),
                            )
                            .on_hover_text(
                                "While Proxies is on in the Media Pool: for the media you import \
                                 that the rules below pick, and for the pool's when you turn \
                                 Proxies on",
                            )
                            .changed()
                        {
                            patch = Some(json!({ "proxies": { "auto": auto } }));
                        }
                        ui.end_row();
                        let mut seq = proxies["image_sequences"] == true;
                        ui.label("For");
                        if ui
                            .add_enabled(
                                !busy && auto,
                                egui::Checkbox::new(&mut seq, "Image sequences"),
                            )
                            .on_hover_text("Every picture is decoded on the CPU as it plays")
                            .changed()
                        {
                            patch = Some(json!({ "proxies": { "image_sequences": seq } }));
                        }
                        ui.end_row();
                        ui.label("");
                        ui.horizontal(|ui| {
                            let label = ui.label("Videos larger than");
                            let saved = |i: usize| proxies["larger_than"][i].as_u64().unwrap_or(1);
                            let field = |i: usize| NumberField {
                                label: label.id,
                                saved: saved(i),
                                range: 1..=100_000,
                                enabled: !busy && auto,
                                tip: "Pictures with more pixels than this get a proxy",
                                refusal: "The size needs a width and height of at least 1",
                            };
                            let w = field(0).show(ui, &mut ui_state.larger_w, &mut ui_state.error);
                            ui.label("×");
                            let h = field(1).show(ui, &mut ui_state.larger_h, &mut ui_state.error);
                            if w.is_some() || h.is_some() {
                                let size = [w.unwrap_or(saved(0)), h.unwrap_or(saved(1))];
                                patch = Some(json!({ "proxies": { "larger_than": size } }));
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        ui.horizontal(|ui| {
                            let label = ui.label("Videos above");
                            let field = NumberField {
                                label: label.id,
                                saved: proxies["above_mbps"].as_u64().unwrap_or(1),
                                range: 1..=100_000,
                                enabled: !busy && auto,
                                tip: "Files averaging more get a proxy: camera and editing \
                                      formats the GPU can't decode",
                                refusal: "The bitrate is at least 1 Mb/s",
                            };
                            if let Some(n) = field.show(ui, &mut ui_state.mbps, &mut ui_state.error)
                            {
                                patch = Some(json!({ "proxies": { "above_mbps": n } }));
                            }
                            ui.label("Mb/s");
                        });
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
