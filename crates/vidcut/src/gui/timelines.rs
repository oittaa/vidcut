use eframe::egui::{self, RichText, Sense};
use serde_json::{Value, json};
use vidcut_core::Project;
use vidcut_core::model::TimelineSettings;
use vidcut_core::ops::{AUDIO_TRACKS, VIDEO_TRACKS};
use vidcut_core::time::format_timecode;

use super::dialog;
use super::project::Format;
use super::workspace::Workspace;

struct NewTimeline {
    name: String,
    bin: String,
    video: u32,
    audio: u32,
    use_project: bool,
    format: Format,
}

struct EditTimeline {
    id: String,
    use_project: bool,
    format: Format,
    was: (bool, Format),
}

#[derive(Default)]
pub(super) struct TimelinesUi {
    new: Option<NewTimeline>,
    settings: Option<EditTimeline>,
    rename: Option<(String, String)>,
    delete: Option<String>,
}

fn has_clips(p: &Project, id: &str) -> bool {
    p.sheets()
        .any(|(t, s)| t.id == id && s.tracks.iter().any(|t| !t.clips.is_empty()))
}

fn length(p: &Project, id: &str) -> String {
    let Some((t, s)) = p.sheets().find(|(t, _)| t.id == id) else {
        return String::new();
    };
    let f = t.own.unwrap_or_else(|| TimelineSettings::of(&p.settings));
    let frames = (s.tracks.iter())
        .flat_map(|t| t.clips.iter().map(|c| c.end()))
        .max()
        .unwrap_or(0);
    format_timecode(frames, f.fps, 0, f.counts_drop_frame())
}

fn name_of(p: &Project, id: &str) -> Option<String> {
    let t = p.timelines.iter().find(|t| t.id == id)?;
    Some(t.name.clone())
}

impl Workspace {
    pub(super) fn open_new_timeline(&mut self) {
        let Some(p) = self.project() else { return };
        let own = TimelineSettings::of(&p.settings);
        self.timelines_ui.new = Some(NewTimeline {
            name: p.next_timeline_name(),
            bin: self.snap.pool.bin.clone(),
            video: 1,
            audio: 1,
            use_project: true,
            format: Format::of_timeline(p, own, None),
        });
    }

    pub(super) fn open_timeline_settings(&mut self, id: &str) {
        let Some(p) = self.project() else { return };
        let Some(t) = p.timelines.iter().find(|t| t.id == id) else {
            return;
        };
        let own = t.own.unwrap_or_else(|| TimelineSettings::of(&p.settings));
        let format = Format::of_timeline(p, own, Some(t.start_tc));
        let use_project = t.own.is_none();
        self.timelines_ui.settings = Some(EditTimeline {
            id: id.to_string(),
            use_project,
            format: format.clone(),
            was: (use_project, format),
        });
    }

    pub(super) fn timeline_menu(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(name) = self.project().and_then(|p| name_of(p, id)) else {
            return;
        };
        if ui.button("Open").clicked() {
            self.send("timeline.open", json!({ "timeline": id }));
            ui.close();
        }
        if ui.button("Duplicate Timeline").clicked() {
            self.send("timeline.duplicate", json!({ "timeline": id }));
            ui.close();
        }
        if ui.button("Rename…").clicked() {
            self.timelines_ui.rename = Some((id.to_string(), name));
            ui.close();
        }
        if ui.button("Timeline Settings…").clicked() {
            self.open_timeline_settings(id);
            ui.close();
        }
        ui.separator();
        if ui.button("Delete Timeline…").clicked() {
            self.timelines_ui.delete = Some(id.to_string());
            ui.close();
        }
    }

    pub(super) fn pooled_timelines(&mut self, ui: &mut egui::Ui, p: &Project, bin: &str) -> bool {
        let needle = self.search.to_lowercase();
        let shown: Vec<(String, String)> = (p.timelines.iter())
            .filter(|t| match needle.is_empty() {
                true => t.bin == bin,
                false => t.name.to_lowercase().contains(&needle),
            })
            .map(|t| (t.id.clone(), t.name.clone()))
            .collect();
        if shown.is_empty() {
            return false;
        }
        let open = p.open_id().map(str::to_string);
        ui.horizontal_wrapped(|ui| {
            for (id, name) in shown {
                let on = open.as_deref() == Some(id.as_str()) && p.compound_path.is_empty();
                let text = format!(
                    "{}  {name}  {}",
                    egui_phosphor::regular::FILM_STRIP,
                    length(p, &id)
                );
                let r = ui.selectable_label(on, text).interact(Sense::click());
                if r.double_clicked() && !on {
                    self.send("timeline.open", json!({ "timeline": id }));
                }
                r.context_menu(|ui| self.timeline_menu(ui, &id));
            }
        });
        ui.separator();
        true
    }

    pub(super) fn timeline_selection(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let Some(p) = self.project() else {
            return ui.label(RichText::new("Timeline").strong());
        };
        let open = p.open_id().map(str::to_string);
        let recent: Vec<(String, String)> = (p.recent.iter())
            .take(vidcut_core::ops::timelines::RECENT_SHOWN)
            .filter_map(|id| Some((id.clone(), name_of(p, id)?)))
            .collect();
        let title = p
            .open_compound()
            .map(|c| c.name.clone())
            .or_else(|| open.as_deref().and_then(|id| name_of(p, id)))
            .unwrap_or_else(|| "No Timeline".into());
        ui.menu_button(RichText::new(title).strong(), |ui| {
            for (id, name) in recent {
                let on = open.as_deref() == Some(id.as_str());
                if ui.selectable_label(on, name).clicked() {
                    if !on {
                        self.send("timeline.open", json!({ "timeline": id }));
                    }
                    ui.close();
                }
            }
        })
        .response
    }

    pub(super) fn timeline_windows(&mut self, ctx: &egui::Context) {
        self.new_timeline_window(ctx);
        self.timeline_settings_window(ctx);
        self.rename_timeline_window(ctx);
        self.delete_timeline_window(ctx);
    }

    fn new_timeline_window(&mut self, ctx: &egui::Context) {
        let Some(mut n) = self.timelines_ui.new.take() else {
            return;
        };
        let (mut open, mut create, mut cancel) = (true, false, false);
        dialog::window(ctx, "New Timeline", &mut open, |ui| {
            n.format.edit_start(ui);
            dialog::form(ui, "new-timeline", |ui| {
                dialog::row(ui, "Timeline Name", |ui| {
                    ui.text_edit_singleline(&mut n.name);
                });
                dialog::row(ui, "Video Tracks", |ui| {
                    ui.add(egui::DragValue::new(&mut n.video).range(1..=VIDEO_TRACKS));
                });
                dialog::row(ui, "Audio Tracks", |ui| {
                    ui.add(egui::DragValue::new(&mut n.audio).range(0..=AUDIO_TRACKS));
                });
            });
            ui.checkbox(&mut n.use_project, "Use Project Settings");
            ui.add_enabled_ui(!n.use_project, |ui| {
                n.format.edit(ui, None);
                n.format.edit_colors(ui);
            });
            let ok = n.format.start().is_some()
                && !n.name.trim().is_empty()
                && (n.use_project || n.format.fps().is_some());
            dialog::footer(ui, |ui| {
                create = dialog::main_button(ui, "Create", ok);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if create {
            let mut params = json!({
                "name": n.name.trim(),
                "bin": n.bin,
                "start_tc": n.format.start(),
                "video_tracks": n.video,
                "audio_tracks": n.audio,
            });
            if let Some(own) = n.format.own().filter(|_| !n.use_project) {
                params["own"] = own;
            }
            self.send("timeline.new", params);
        } else if open && !cancel {
            self.timelines_ui.new = Some(n);
        }
    }

    fn timeline_settings_window(&mut self, ctx: &egui::Context) {
        let Some(mut e) = self.timelines_ui.settings.take() else {
            return;
        };
        let Some(p) = self.project() else { return };
        let Some(name) = name_of(p, &e.id) else {
            return;
        };
        let locked = has_clips(p, &e.id);
        let (mut open, mut apply, mut cancel) = (true, None, false);
        dialog::window(ctx, "Timeline Settings", &mut open, |ui| {
            ui.label(RichText::new(&name).strong());
            e.format.edit_start(ui);
            if ui
                .checkbox(&mut e.use_project, "Use Project Settings")
                .changed()
            {
                let project = TimelineSettings::of(&p.settings);
                e.format = Format::of_timeline(p, project, None).with_start_of(&e.format);
            }
            ui.add_enabled_ui(!e.use_project, |ui| {
                e.format
                    .edit(ui, locked.then_some("only while the timeline is empty"));
                e.format.edit_colors(ui);
            });
            let changes = changes(&e);
            let ok = e.format.start().is_some()
                && e.format.fps().is_some()
                && changes.as_object().is_some_and(|c| c.len() > 1);
            dialog::footer(ui, |ui| {
                if dialog::main_button(ui, "Apply", ok) {
                    apply = Some(changes);
                }
                cancel = ui.button("Cancel").clicked();
            });
        });
        if let Some(changes) = apply {
            self.send("timeline.set", changes);
        } else if open && !cancel {
            self.timelines_ui.settings = Some(e);
        }
    }

    fn rename_timeline_window(&mut self, ctx: &egui::Context) {
        let Some((id, mut name)) = self.timelines_ui.rename.take() else {
            return;
        };
        let (mut open, mut done, mut cancel) = (true, false, false);
        dialog::window(ctx, "Rename Timeline", &mut open, |ui| {
            dialog::form(ui, "rename-timeline", |ui| {
                dialog::row(ui, "Timeline Name", |ui| {
                    let r = ui.text_edit_singleline(&mut name);
                    done = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                });
            });
            dialog::footer(ui, |ui| {
                done |= dialog::main_button(ui, "Rename", !name.trim().is_empty());
                cancel = ui.button("Cancel").clicked();
            });
        });
        if done && !name.trim().is_empty() {
            let params = json!({ "timeline": id, "name": name.trim() });
            self.send("timeline.rename", params);
        } else if open && !cancel {
            self.timelines_ui.rename = Some((id, name));
        }
    }

    fn delete_timeline_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.timelines_ui.delete.take() else {
            return;
        };
        let Some(name) = self.project().and_then(|p| name_of(p, &id)) else {
            return;
        };
        let (mut open, mut delete, mut cancel) = (true, false, false);
        dialog::window(ctx, "Delete Timeline", &mut open, |ui| {
            ui.label(format!("Delete {name}?"));
            dialog::footer(ui, |ui| {
                delete = dialog::main_button(ui, "Delete", true);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if delete {
            self.send("timeline.delete", json!({ "timeline": id }));
        } else if open && !cancel {
            self.timelines_ui.delete = Some(id);
        }
    }
}

fn changes(e: &EditTimeline) -> Value {
    let mut set = serde_json::Map::new();
    set.insert("timeline".into(), json!(e.id));
    let (was_project, was) = &e.was;
    if !e.format.same_start(was) {
        set.insert("start_tc".into(), json!(e.format.start()));
    }
    if e.use_project != *was_project {
        set.insert("use_project".into(), json!(e.use_project));
    }
    let own = e.format.own_changes(was);
    if !e.use_project && own.as_object().is_some_and(|o| !o.is_empty()) {
        set.insert("own".into(), own);
    }
    Value::Object(set)
}
