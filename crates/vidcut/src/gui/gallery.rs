use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, ColorImage, Rect, RichText, Sense, TextureOptions, pos2, vec2};
use serde_json::{Value, json};
use vidcut_av::thumbs::Image;
use vidcut_core::lru::Lru;
use vidcut_core::model::{ApplyKeys, Project, Settings};
use vidcut_core::ops::gallery::numbered;
use vidcut_core::ops::grade::{GradeOf, PRESERVED_NODES};
use vidcut_core::request::GradeLevel;

use super::dialog;
use super::theme;
use super::workspace::{Workspace, dim};
use crate::threads::Work;

pub(super) const STILLS_KEPT: usize = 96;
const PICTURE: egui::Vec2 = vec2(112.0, 63.0);
const SLOT: egui::Vec2 = vec2(56.0, 32.0);
const CELL_GAP: f32 = 8.0;
const SLOT_GAP: f32 = 4.0;
const ALBUMS_WIDTH: f32 = 120.0;

type Loaded = (PathBuf, Option<Image>);

type Texture = (egui::TextureId, egui::Vec2);

#[derive(Clone)]
struct Dragged(Vec<String>);

struct Renaming {
    album: String,
    text: String,
    fresh: bool,
}

struct Labeling {
    still: String,
    text: String,
    fresh: bool,
}

pub(super) struct Gallery {
    kept: Lru<PathBuf, Option<egui::TextureHandle>, STILLS_KEPT>,
    loading: Option<PathBuf>,
    sender: Sender<Loaded>,
    results: Receiver<Loaded>,
    album: Option<String>,
    pub(super) albums: bool,
    pub(super) memories: bool,
    renaming: Option<Renaming>,
    labeling: Option<Labeling>,
    removing: Option<String>,
}

impl Default for Gallery {
    fn default() -> Self {
        let (sender, results) = channel();
        Self {
            kept: Lru::default(),
            loading: None,
            sender,
            results,
            album: None,
            albums: false,
            memories: false,
            renaming: None,
            labeling: None,
            removing: None,
        }
    }
}

impl Gallery {
    fn take_loaded(&mut self, ctx: &egui::Context) {
        while let Ok((path, image)) = self.results.try_recv() {
            let texture = image.map(|i| {
                let size = [i.width as usize, i.height as usize];
                let picture = ColorImage::from_rgba_unmultiplied(size, &i.rgba);
                ctx.load_texture("gallery-still", picture, TextureOptions::LINEAR)
            });
            if self.loading.as_ref() == Some(&path) {
                self.loading = None;
            }
            self.kept.insert(path, texture);
        }
    }

    fn texture(&mut self, ctx: &egui::Context, path: &Path, height: u32) -> Option<Texture> {
        if self.kept.contains(path) {
            let t = self.kept.get(path)?.as_ref()?;
            return Some((t.id(), t.size_vec2()));
        }
        if self.loading.is_some() {
            return None;
        }
        self.loading = Some(path.to_path_buf());
        let (path, sender, ctx) = (path.to_path_buf(), self.sender.clone(), ctx.clone());
        let started = crate::threads::spawn("vidcut-still", Work::Job, move || {
            let image = vidcut_av::export::still::exr_rgba(&path, height).ok();
            let _ = sender.send((path, image));
            ctx.request_repaint();
        });
        if started.is_err() {
            self.loading = None;
        }
        None
    }
}

enum Act {
    Pick(String, bool),
    Apply(String),
    Append(String),
    Delete(Vec<String>),
    Move(Vec<String>, String),
    Albums(bool),
    Open(String),
    AddAlbum,
    StartRename(String, String),
    Rename(String, String),
    StopRename,
    Remove(String),
    Memories(bool),
    ToMemory(String, char),
    ApplyMemory(char),
    ClearMemory(char),
    StartLabel(String, String),
    Label(String, String),
    StopLabel,
}

impl Workspace {
    pub(super) fn shown_album(&self) -> Option<String> {
        let p = self.project()?;
        let kept = (self.gallery.album.as_deref()).and_then(|id| p.album(id));
        kept.or(p.albums.first()).map(|a| a.id.clone())
    }

    pub(super) fn set_gallery(
        &mut self,
        album: Option<String>,
        albums: Option<bool>,
        memories: Option<bool>,
    ) -> Result<(), String> {
        if let Some(id) = album {
            let known = self.project().is_some_and(|p| p.album(&id).is_some());
            if !known {
                return Err(format!("No album {id}"));
            }
            self.gallery.album = Some(id);
        }
        self.gallery.albums = albums.unwrap_or(self.gallery.albums);
        self.gallery.memories = memories.unwrap_or(self.gallery.memories);
        Ok(())
    }

    pub(super) fn gallery_panel(&mut self, ui: &mut egui::Ui) {
        let mut act = None;
        ui.horizontal(|ui| {
            let on = self.gallery.albums;
            let list = egui::Button::selectable(on, egui_phosphor::regular::FOLDERS);
            if ui.add(list).on_hover_text("Albums").clicked() {
                act = Some(Act::Albums(!on));
            }
            let on = self.gallery.memories;
            let bank = egui::Button::selectable(on, egui_phosphor::regular::SQUARES_FOUR);
            if ui.add(bank).on_hover_text("Memories").clicked() {
                act = Some(Act::Memories(!on));
            }
            ui.label(RichText::new("Gallery").strong());
        });
        let Some(p) = self.project_shared() else {
            return;
        };
        let area = ui.available_rect_before_wrap();
        let back = ui.interact(area, ui.make_persistent_id("gallery-area"), Sense::click());
        let mut set = None;
        back.context_menu(|ui| set = gallery_menu(ui, &p.settings));
        if let Some(params) = set {
            self.send("project.set", params);
        }
        self.gallery.take_loaded(ui.ctx());
        let shown = self.shown_album();
        ui.horizontal_top(|ui| {
            if self.gallery.albums {
                self.album_list(ui, &p, shown.as_deref(), &mut act);
                ui.separator();
            }
            ui.vertical(|ui| {
                if self.gallery.memories {
                    self.memory_bank(ui, &p, &mut act);
                    ui.separator();
                }
                self.still_grid(ui, &p, shown.as_deref(), &mut act);
            });
        });
        self.remove_album_window(ui.ctx(), &p);
        self.gallery_act(act);
    }

    fn album_list(
        &mut self,
        ui: &mut egui::Ui,
        p: &Project,
        shown: Option<&str>,
        act: &mut Option<Act>,
    ) {
        let colors = theme::colors();
        ui.vertical(|ui| {
            ui.set_width(ALBUMS_WIDTH);
            for a in &p.albums {
                if let Some(r) = (self.gallery.renaming.as_mut()).filter(|r| r.album == a.id) {
                    let edit = egui::TextEdit::singleline(&mut r.text).desired_width(ALBUMS_WIDTH);
                    let field = ui.add(edit);
                    if r.fresh {
                        field.request_focus();
                        r.fresh = false;
                    }
                    if field.lost_focus() {
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        *act = Some(match enter {
                            true => Act::Rename(a.id.clone(), r.text.clone()),
                            false => Act::StopRename,
                        });
                    }
                    continue;
                }
                let on = shown == Some(a.id.as_str());
                let item = egui::Button::selectable(on, a.name.as_str()).truncate();
                let r = ui.add_sized([ALBUMS_WIDTH, ui.spacing().interact_size.y], item);
                if r.dnd_hover_payload::<Dragged>().is_some() {
                    let stroke = egui::Stroke::new(2.0, colors.accent);
                    let painter = ui.painter();
                    painter.rect_stroke(r.rect, 2.0, stroke, egui::StrokeKind::Inside);
                }
                if let Some(moved) = r.dnd_release_payload::<Dragged>() {
                    *act = Some(Act::Move(moved.0.clone(), a.id.clone()));
                } else if r.double_clicked() {
                    *act = Some(Act::StartRename(a.id.clone(), a.name.clone()));
                } else if (r.clicked() || r.secondary_clicked()) && !on {
                    *act = Some(Act::Open(a.id.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button("Add Still Album").clicked() {
                        *act = Some(Act::AddAlbum);
                        ui.close();
                    }
                    if ui.button("Remove Current Album").clicked() {
                        *act = Some(Act::Remove(a.id.clone()));
                        ui.close();
                    }
                });
            }
            let rest = ui.available_size().max(vec2(ALBUMS_WIDTH, 24.0));
            let below = ui.allocate_response(rest, Sense::click());
            below.context_menu(|ui| {
                if ui.button("Add Still Album").clicked() {
                    *act = Some(Act::AddAlbum);
                    ui.close();
                }
            });
        });
    }

    fn memory_bank(&mut self, ui: &mut egui::Ui, p: &Project, act: &mut Option<Act>) {
        let dir = self.snap.still_dir.clone();
        let height = (PICTURE.y * ui.ctx().pixels_per_point()).round().max(1.0) as u32;
        let colors = theme::colors();
        let font = egui::TextStyle::Small.resolve(ui.style());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(SLOT_GAP, SLOT_GAP);
            for slot in 'A'..='Z' {
                let memory = p.memory(slot);
                let (rect, r) = ui.allocate_exact_size(SLOT, Sense::click());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, egui::Color32::BLACK);
                if let Some(m) = memory
                    && ui.is_rect_visible(rect)
                    && let Some((texture, size)) =
                        self.gallery
                            .texture(ui.ctx(), &dir.join(&m.picture), height)
                {
                    let shown = Rect::from_center_size(rect.center(), fit(size, SLOT));
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    painter.image(texture, shown, uv, egui::Color32::WHITE);
                }
                let corner = rect.left_top() + vec2(3.0, 1.0);
                let letter = slot.to_string();
                painter.text(
                    corner,
                    egui::Align2::LEFT_TOP,
                    letter,
                    font.clone(),
                    colors.text,
                );
                if r.dnd_hover_payload::<Dragged>().is_some() {
                    let stroke = egui::Stroke::new(2.0, colors.accent);
                    painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                }
                if let Some(dropped) = r.dnd_release_payload::<Dragged>()
                    && let Some(first) = dropped.0.first()
                {
                    *act = Some(Act::ToMemory(first.clone(), slot));
                }
                if memory.is_none() {
                    continue;
                }
                if r.middle_clicked() {
                    *act = Some(Act::ApplyMemory(slot));
                }
                r.context_menu(|ui| {
                    if ui.button("Apply Grade").clicked() {
                        *act = Some(Act::ApplyMemory(slot));
                        ui.close();
                    }
                    if ui.button("Clear").clicked() {
                        *act = Some(Act::ClearMemory(slot));
                        ui.close();
                    }
                });
            }
        });
    }

    fn still_grid(
        &mut self,
        ui: &mut egui::Ui,
        p: &Project,
        shown: Option<&str>,
        act: &mut Option<Act>,
    ) {
        let stills = shown.map(|album| numbered(p, album)).unwrap_or_default();
        if stills.is_empty() {
            ui.label(dim("No Stills"));
            return;
        }
        let dir = self.snap.still_dir.clone();
        let selected: Vec<String> = (self.snap.selection.stills.iter())
            .filter(|id| stills.iter().any(|(s, _)| s.id == **id))
            .cloned()
            .collect();
        let height = (PICTURE.y * ui.ctx().pixels_per_point()).round().max(1.0) as u32;
        let colors = theme::colors();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(CELL_GAP, CELL_GAP);
                    for (s, number) in &stills {
                        let chosen = selected.contains(&s.id);
                        let cell = ui.vertical(|ui| {
                            ui.set_width(PICTURE.x);
                            let sense = Sense::click_and_drag();
                            let (rect, picture) = ui.allocate_exact_size(PICTURE, sense);
                            let painter = ui.painter_at(rect);
                            painter.rect_filled(rect, 2.0, colors.panel_dark);
                            let path = dir.join(&s.picture);
                            if ui.is_rect_visible(rect)
                                && let Some((texture, size)) =
                                    self.gallery.texture(ui.ctx(), &path, height)
                            {
                                let shown =
                                    Rect::from_center_size(rect.center(), fit(size, PICTURE));
                                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                painter.image(texture, shown, uv, egui::Color32::WHITE);
                            }
                            if chosen {
                                let stroke = egui::Stroke::new(2.0, colors.accent);
                                painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                            }
                            ui.add(egui::Label::new(dim(number.as_str()).small()).truncate());
                            let labeling =
                                (self.gallery.labeling.as_mut()).filter(|l| l.still == s.id);
                            match labeling {
                                Some(l) => label_field(ui, l, act),
                                None if s.label.is_empty() => {}
                                None => {
                                    let label = egui::Label::new(s.label.as_str()).truncate();
                                    ui.add(label);
                                }
                            }
                            picture
                        });
                        let picture = cell.inner;
                        let mine = match chosen {
                            true => selected.clone(),
                            false => vec![s.id.clone()],
                        };
                        picture.dnd_set_drag_payload(Dragged(mine.clone()));
                        if picture.clicked() {
                            let toggle = ui.input(|i| i.modifiers.command);
                            *act = Some(Act::Pick(s.id.clone(), toggle));
                        }
                        if picture.middle_clicked() {
                            *act = Some(Act::Apply(s.id.clone()));
                        }
                        if picture.secondary_clicked() && !chosen {
                            *act = Some(Act::Pick(s.id.clone(), false));
                        }
                        picture.context_menu(|ui| {
                            if ui.button("Apply Grade").clicked() {
                                *act = Some(Act::Apply(s.id.clone()));
                                ui.close();
                            }
                            if ui.button("Append Node Graph").clicked() {
                                *act = Some(Act::Append(s.id.clone()));
                                ui.close();
                            }
                            if ui.button("Delete Selected").clicked() {
                                *act = Some(Act::Delete(mine));
                                ui.close();
                            }
                            if ui.button("Change Label").clicked() {
                                *act = Some(Act::StartLabel(s.id.clone(), s.label.clone()));
                                ui.close();
                            }
                        });
                    }
                });
            });
    }

    fn remove_album_window(&mut self, ctx: &egui::Context, p: &Project) {
        let Some(id) = self.gallery.removing.take() else {
            return;
        };
        let Some(album) = p.album(&id) else {
            return;
        };
        let (mut open, mut delete, mut cancel) = (true, false, false);
        dialog::window(ctx, "Remove Album", &mut open, |ui| {
            ui.label(format!("Delete {} and its stills?", album.name));
            dialog::footer(ui, |ui| {
                delete = dialog::main_button(ui, "Delete", true);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if delete {
            self.send("gallery.album.remove", json!({ "album": id }));
        } else if open && !cancel {
            self.gallery.removing = Some(id);
        }
    }

    fn gallery_act(&mut self, act: Option<Act>) {
        match act {
            Some(Act::Pick(id, true)) => {
                let mut stills = self.snap.selection.stills.clone();
                match stills.iter().position(|s| *s == id) {
                    Some(i) => {
                        stills.remove(i);
                    }
                    None => stills.push(id),
                }
                self.send("selection.set", json!({ "stills": stills }));
            }
            Some(Act::Pick(id, false)) => {
                self.send("selection.set", json!({ "stills": [id] }));
            }
            Some(Act::Apply(id)) => {
                let grade = self.effects_page.grading;
                self.send("gallery.apply", json!({ "still": id, "grade": grade }));
            }
            Some(Act::Append(id)) => self.append_still(&id),
            Some(Act::Delete(stills)) => self.send("gallery.remove", json!({ "stills": stills })),
            Some(Act::Move(stills, album)) => {
                self.send("gallery.move", json!({ "stills": stills, "album": album }));
            }
            Some(Act::Albums(on)) => self.send("gui.gallery", json!({ "albums": on })),
            Some(Act::Open(album)) => self.send("gui.gallery", json!({ "album": album })),
            Some(Act::AddAlbum) => {
                let Some(next) = self.project().map(|p| format!("a{}", p.next_id.album)) else {
                    return;
                };
                self.send("gallery.album.add", json!({}));
                self.gallery.album = Some(next);
            }
            Some(Act::StartRename(album, text)) => {
                self.gallery.renaming = Some(Renaming {
                    album,
                    text,
                    fresh: true,
                });
            }
            Some(Act::Rename(album, name)) => {
                self.gallery.renaming = None;
                if !name.trim().is_empty() {
                    let params = json!({ "album": album, "name": name.trim() });
                    self.send("gallery.album.rename", params);
                }
            }
            Some(Act::StopRename) => self.gallery.renaming = None,
            Some(Act::Remove(album)) => self.gallery.removing = Some(album),
            Some(Act::Memories(on)) => self.send("gui.gallery", json!({ "memories": on })),
            Some(Act::ToMemory(still, slot)) => {
                self.send(
                    "gallery.memory.copy",
                    json!({ "still": still, "memory": slot }),
                );
            }
            Some(Act::ApplyMemory(slot)) => {
                let grade = self.effects_page.grading;
                self.send(
                    "gallery.memory.apply",
                    json!({ "memory": slot, "grade": grade }),
                );
            }
            Some(Act::ClearMemory(slot)) => {
                self.send("gallery.memory.clear", json!({ "memory": slot }));
            }
            Some(Act::StartLabel(still, text)) => {
                self.gallery.labeling = Some(Labeling {
                    still,
                    text,
                    fresh: true,
                });
            }
            Some(Act::Label(still, label)) => {
                self.gallery.labeling = None;
                self.send("gallery.label", json!({ "still": still, "label": label }));
            }
            Some(Act::StopLabel) => self.gallery.labeling = None,
            None => {}
        }
    }

    pub(super) fn save_memory(&self, slot: char) {
        let at = self.snap.timeline.position;
        self.send("gallery.grab", json!({ "at": at, "memory": slot }));
    }

    pub(super) fn load_memory(&self, slot: char) {
        let grade = self.effects_page.grading;
        let mut params = json!({ "memory": slot, "grade": grade });
        if matches!(grade, GradeLevel::Clip) {
            params["layer"] = json!(self.shown_layer());
        }
        self.send("gallery.memory.load", params);
    }

    fn append_still(&self, id: &str) {
        let grade = self.effects_page.grading;
        let mut params = json!({ "still": id, "grade": grade });
        if let Some((GradeOf::Clip(clip, layer), _, _)) = self.grade_shown() {
            params["clips"] = json!([clip]);
            params["layer"] = json!(layer);
        }
        self.send("gallery.append", params);
    }

    pub(super) fn grab_still(&self) {
        let at = self.snap.timeline.position;
        let mut params = json!({ "at": at });
        if let Some(album) = self.shown_album() {
            params["album"] = json!(album);
        }
        self.send("gallery.grab", params);
    }
}

fn label_field(ui: &mut egui::Ui, l: &mut Labeling, act: &mut Option<Act>) {
    let output = egui::TextEdit::singleline(&mut l.text)
        .desired_width(PICTURE.x)
        .show(ui);
    let field = output.response;
    if l.fresh {
        field.request_focus();
        let mut state = output.state;
        let all = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(l.text.chars().count()),
        );
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), field.id);
        l.fresh = false;
    }
    if field.lost_focus() {
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        *act = Some(match enter {
            true => Act::Label(l.still.clone(), l.text.clone()),
            false => Act::StopLabel,
        });
    }
}

fn gallery_menu(ui: &mut egui::Ui, s: &Settings) -> Option<Value> {
    let mut set = None;
    ui.menu_button("Apply Grades Using", |ui| {
        let modes = [
            (ApplyKeys::None, "No Keyframes"),
            (
                ApplyKeys::SourceTimecode,
                "Keyframes Aligning Source Timecode",
            ),
            (ApplyKeys::StartFrames, "Keyframes Aligning Start Frames"),
        ];
        for (keys, name) in modes {
            if ui.radio(s.apply_keys == keys, name).clicked() {
                set = Some(json!({ "apply_keys": keys }));
                ui.close();
            }
        }
    });
    ui.menu_button("Preserve Number Of Nodes", |ui| {
        for n in 0..=PRESERVED_NODES {
            if ui.radio(s.preserve_nodes == n, n.to_string()).clicked() {
                set = Some(json!({ "preserve_nodes": n }));
                ui.close();
            }
        }
    });
    set
}

fn fit(size: egui::Vec2, into: egui::Vec2) -> egui::Vec2 {
    let aspect = size.x / size.y.max(1.0);
    match aspect > into.x / into.y {
        true => vec2(into.x, into.x / aspect),
        false => vec2(into.y * aspect, into.y),
    }
}
