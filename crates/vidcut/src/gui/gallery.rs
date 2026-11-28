use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, ColorImage, Rect, RichText, Sense, TextureOptions, pos2, vec2};
use serde_json::json;
use vidcut_av::thumbs::Image;
use vidcut_core::lru::Lru;
use vidcut_core::ops::grade::GradeOf;

use super::theme;
use super::workspace::{Workspace, dim};
use crate::threads::Work;

pub(super) const STILLS_KEPT: usize = 96;
const PICTURE: egui::Vec2 = vec2(112.0, 63.0);
const CELL_GAP: f32 = 8.0;

type Loaded = (PathBuf, Option<Image>);

type Texture = (egui::TextureId, egui::Vec2);

pub(super) struct Gallery {
    kept: Lru<PathBuf, Option<egui::TextureHandle>, STILLS_KEPT>,
    loading: Option<PathBuf>,
    sender: Sender<Loaded>,
    results: Receiver<Loaded>,
}

impl Default for Gallery {
    fn default() -> Self {
        let (sender, results) = channel();
        Self {
            kept: Lru::default(),
            loading: None,
            sender,
            results,
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
}

impl Workspace {
    pub(super) fn gallery_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Gallery").strong());
        let Some(p) = self.project_shared() else {
            return;
        };
        self.gallery.take_loaded(ui.ctx());
        if p.stills.is_empty() {
            ui.label(dim("No Stills"));
            return;
        }
        let dir = self.snap.still_dir.clone();
        let selected = self.snap.selection.stills.clone();
        let height = (PICTURE.y * ui.ctx().pixels_per_point()).round().max(1.0) as u32;
        let colors = theme::colors();
        let mut act = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(CELL_GAP, CELL_GAP);
                    for s in &p.stills {
                        let chosen = selected.contains(&s.id);
                        let cell = ui.vertical(|ui| {
                            ui.set_width(PICTURE.x);
                            let (rect, picture) = ui.allocate_exact_size(PICTURE, Sense::click());
                            let painter = ui.painter_at(rect);
                            painter.rect_filled(rect, 2.0, colors.panel_dark);
                            let path = dir.join(&s.picture);
                            if ui.is_rect_visible(rect)
                                && let Some((texture, size)) =
                                    self.gallery.texture(ui.ctx(), &path, height)
                            {
                                let shown = Rect::from_center_size(rect.center(), fit(size));
                                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                painter.image(texture, shown, uv, egui::Color32::WHITE);
                            }
                            if chosen {
                                let stroke = egui::Stroke::new(2.0, colors.accent);
                                painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                            }
                            let name = match s.label.is_empty() {
                                true => s.id.as_str(),
                                false => s.label.as_str(),
                            };
                            ui.add(egui::Label::new(dim(name).small()).truncate());
                            picture
                        });
                        let picture = cell.inner;
                        if picture.clicked() {
                            let toggle = ui.input(|i| i.modifiers.command);
                            act = Some(Act::Pick(s.id.clone(), toggle));
                        }
                        if picture.middle_clicked() {
                            act = Some(Act::Apply(s.id.clone()));
                        }
                        if picture.secondary_clicked() && !chosen {
                            act = Some(Act::Pick(s.id.clone(), false));
                        }
                        picture.context_menu(|ui| {
                            if ui.button("Apply Grade").clicked() {
                                act = Some(Act::Apply(s.id.clone()));
                                ui.close();
                            }
                            if ui.button("Append Node Graph").clicked() {
                                act = Some(Act::Append(s.id.clone()));
                                ui.close();
                            }
                            if ui.button("Delete Selected").clicked() {
                                act = Some(Act::Delete(match chosen {
                                    true => selected.clone(),
                                    false => vec![s.id.clone()],
                                }));
                                ui.close();
                            }
                        });
                    }
                });
            });
        match act {
            Some(Act::Pick(id, true)) => {
                let mut stills = selected;
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
            None => {}
        }
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
        self.send("gallery.grab", json!({ "at": at }));
    }
}

fn fit(size: egui::Vec2) -> egui::Vec2 {
    let aspect = size.x / size.y.max(1.0);
    match aspect > PICTURE.x / PICTURE.y {
        true => vec2(PICTURE.x, PICTURE.x / aspect),
        false => vec2(PICTURE.y * aspect, PICTURE.y),
    }
}
