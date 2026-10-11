//! The Media Pool (spec §3): thumbnails with hover scrubbing, a list view,
//! search, and the edit-function buttons below it (spec §2, §7).

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, vec2};
use serde_json::json;
use vidcut_core::model::Media;
use vidcut_core::request::ViewerMode;

use super::theme;
use super::workspace::{PoolView, Workspace, dim, fit};

const TILE: egui::Vec2 = egui::Vec2::new(176.0, 99.0);

impl Workspace {
    pub(super) fn pool_panel(&mut self, ui: &mut egui::Ui) {
        self.hover = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Media Pool").strong());
            ui.selectable_value(&mut self.pool_view, PoolView::Thumbnails, "Grid")
                .on_hover_text("Thumbnail view");
            ui.selectable_value(&mut self.pool_view, PoolView::List, "List")
                .on_hover_text("List view");
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.separator();
        let media: Vec<Media> = self
            .project()
            .map(|p| {
                p.media
                    .iter()
                    .filter(|m| {
                        self.search.is_empty()
                            || m.name.to_lowercase().contains(&self.search.to_lowercase())
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let buttons = 84.0;
        let height = (ui.available_height() - buttons).max(60.0);
        egui::ScrollArea::vertical()
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if media.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(dim(if self.search.is_empty() {
                            "No media yet. Import with Ctrl+I, or `vidcut media add FILE…`"
                        } else {
                            "Nothing matches"
                        }));
                        if self.search.is_empty() && ui.button("Import Media…").clicked() {
                            self.import_media();
                        }
                    });
                    return;
                }
                match self.pool_view {
                    PoolView::Thumbnails => {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                            for m in &media {
                                self.tile(ui, m);
                            }
                        });
                    }
                    PoolView::List => self.list(ui, &media),
                }
            });
        ui.separator();
        self.edit_buttons(ui);
    }

    /// One thumbnail with hover scrubbing and source marks (p. 664, 691).
    fn tile(&mut self, ui: &mut egui::Ui, m: &Media) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(TILE.x, TILE.y + 30.0), Sense::click_and_drag());
        let picture = Rect::from_min_size(rect.min, TILE);
        let painter = ui.painter_at(rect);
        let selected = self.snap.selection.media.contains(&m.id);
        let open = self.snap.source_at().map(|(id, _)| id) == Some(m.id.as_str());
        painter.rect_filled(picture, CornerRadius::same(3), theme::TRACK);
        let frames = self.project().map_or(0, |p| p.media_frames(m));
        let hovered_frame = resp.hover_pos().map(|pos| {
            let f = ((pos.x - picture.left()) / picture.width()).clamp(0.0, 1.0);
            ((f64::from(f) * frames as f64) as i64).min((frames - 1).max(0))
        });
        if let Some(f) = hovered_frame {
            self.hover = Some((m.id.clone(), f));
        }
        match self.thumbs.get(&m.id) {
            Some(t) if !t.textures.is_empty() => {
                let n = t.textures.len();
                let i = hovered_frame.map_or(0, |f| {
                    ((f as f64 / frames.max(1) as f64) * n as f64).floor() as usize
                });
                let tex = &t.textures[i.min(n - 1)];
                let size = tex.size_vec2();
                let r = fit(picture, size.x / size.y.max(1.0));
                painter.image(
                    tex.id(),
                    r,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            _ => {
                let text = if m.info.video.is_none() {
                    "♪ audio"
                } else {
                    "…"
                };
                painter.text(
                    picture.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(16.0),
                    theme::TEXT_DIM,
                );
            }
        }
        // Source marks along the bottom edge, hover position as a line.
        if frames > 0 {
            let x = |f: i64| picture.left() + picture.width() * (f as f32 / frames as f32);
            if !m.marks.is_empty() {
                let a = x(m.marks.mark_in.unwrap_or(0));
                let b = x(m.marks.mark_out.unwrap_or(frames));
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(a, picture.bottom() - 4.0),
                        Pos2::new(b, picture.bottom()),
                    ),
                    CornerRadius::ZERO,
                    Color32::WHITE,
                );
            }
            if let Some(f) = hovered_frame {
                painter.line_segment(
                    [
                        Pos2::new(x(f), picture.top()),
                        Pos2::new(x(f), picture.bottom()),
                    ],
                    Stroke::new(1.0, theme::PLAYHEAD),
                );
            }
        }
        let border = if selected {
            Stroke::new(2.0, theme::ACCENT)
        } else if open {
            Stroke::new(1.0, theme::TEXT_DIM)
        } else {
            Stroke::NONE
        };
        painter.rect_stroke(
            picture,
            CornerRadius::same(3),
            border,
            egui::StrokeKind::Inside,
        );
        let label = |pos: Pos2, text: String, color: Color32| {
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::proportional(12.0),
                color,
            )
        };
        label(
            Pos2::new(picture.left(), picture.bottom() + 2.0),
            elide(&m.name, 28),
            theme::TEXT,
        );
        label(
            Pos2::new(picture.left(), picture.bottom() + 16.0),
            match hovered_frame {
                Some(f) => self.src_tc(f),
                None => self.src_tc(frames),
            },
            theme::TEXT_DIM,
        );
        let resp = resp.on_hover_text(format!(
            "{}\n{}\nI/O while hovering marks the clip; double-click opens it",
            m.name,
            m.path.display()
        ));
        self.media_interaction(&resp, m);
    }

    fn list(&mut self, ui: &mut egui::Ui, media: &[Media]) {
        egui::Grid::new("pool-list")
            .striped(true)
            .num_columns(6)
            .spacing(vec2(12.0, 4.0))
            .show(ui, |ui| {
                for h in ["Name", "Duration", "Resolution", "FPS", "Codec", "Audio"] {
                    ui.label(dim(h));
                }
                ui.end_row();
                for m in media {
                    let selected = self.snap.selection.media.contains(&m.id);
                    let resp = ui
                        .selectable_label(selected, elide(&m.name, 40))
                        .interact(Sense::drag());
                    self.media_interaction(&resp, m);
                    let frames = self.project().map_or(0, |p| p.media_frames(m));
                    ui.label(self.src_tc(frames));
                    match &m.info.video {
                        Some(v) => {
                            ui.label(format!("{}x{}", v.width, v.height));
                            ui.label(v.fps.to_string());
                            ui.label(&v.codec);
                        }
                        None => {
                            ui.label("-");
                            ui.label("-");
                            ui.label("-");
                        }
                    }
                    ui.label(match m.info.audio.first() {
                        Some(a) => format!("{} {} ch", a.codec, a.channels),
                        None => "none".into(),
                    });
                    ui.end_row();
                }
            });
    }

    /// Click selects (and shows the clip in a source viewer), double-click
    /// opens it as Source Clip, dragging drops it on the timeline (spec §3).
    fn media_interaction(&mut self, resp: &egui::Response, m: &Media) {
        if resp.drag_started()
            && let Some(press) = resp.ctx.input(|i| i.pointer.press_origin())
        {
            let frames = self.project().map_or(0, |p| p.media_frames(m));
            let range = (
                m.marks.mark_in.unwrap_or(0),
                m.marks.mark_out.unwrap_or(frames),
            );
            self.start_media_drag(&m.id, range, press);
            return;
        }
        if resp.double_clicked() {
            self.send("viewer.load", json!({ "media": m.id }));
            return;
        }
        if resp.clicked() {
            let mods = resp.ctx.input(|i| i.modifiers);
            let mut selection = self.snap.selection.media.clone();
            if mods.command {
                if let Some(i) = selection.iter().position(|x| x == &m.id) {
                    selection.remove(i);
                } else {
                    selection.push(m.id.clone());
                }
            } else if mods.shift
                && let (Some(p), Some(last)) = (self.project(), selection.last())
            {
                let ids: Vec<&str> = p.media.iter().map(|x| x.id.as_str()).collect();
                let (a, b) = (
                    ids.iter().position(|x| x == last).unwrap_or(0),
                    ids.iter().position(|x| *x == m.id).unwrap_or(0),
                );
                selection = ids[a.min(b)..=a.max(b)]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            } else {
                selection = vec![m.id.clone()];
            }
            self.send("selection.set", json!({ "media": selection }));
            if !mods.command && !mods.shift {
                match self.snap.viewer {
                    ViewerMode::SourceClip => self.send("viewer.load", json!({ "media": m.id })),
                    ViewerMode::SourceTape => {
                        if let Some(e) = self.snap.tape.iter().find(|e| e.media == m.id) {
                            self.send(
                                "player.seek",
                                json!({ "at": e.start, "viewer": "source_tape" }),
                            );
                        }
                    }
                    ViewerMode::Timeline => {}
                }
            }
        }
        resp.context_menu(|ui| {
            if ui.button("Open in Viewer").clicked() {
                self.send("viewer.load", json!({ "media": m.id }));
                ui.close();
            }
            if ui.button("Append to Timeline").clicked() {
                let frames = self.project().map_or(0, |p| p.media_frames(m));
                let (a, b) = (
                    m.marks.mark_in.unwrap_or(0),
                    m.marks.mark_out.unwrap_or(frames),
                );
                self.send(
                    "edit.append",
                    json!({ "media": m.id, "range": format!("{a}f..{b}f") }),
                );
                ui.close();
            }
            if ui
                .add_enabled(!m.marks.is_empty(), egui::Button::new("Clear In and Out"))
                .clicked()
            {
                self.send("media.marks.clear", json!({ "media": m.id }));
                ui.close();
            }
            ui.separator();
            let used = self.project().is_some_and(|p| {
                p.tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .any(|c| c.media == m.id)
            });
            let label = if used {
                "Remove from Pool (and its clips)"
            } else {
                "Remove from Pool"
            };
            if ui.button(label).clicked() {
                self.send("media.remove", json!({ "media": m.id, "force": used }));
                ui.close();
            }
        });
    }

    /// The edit-function buttons under the pool (spec §2, §7.2).
    fn edit_buttons(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let buttons = [
                (
                    "Smart Insert",
                    "edit.smart_insert",
                    "F9: insert at the edit nearest the playhead",
                ),
                ("Append", "edit.append", "Shift+F12: after the last clip"),
                (
                    "Ripple Overwrite",
                    "edit.ripple_overwrite",
                    "Shift+F10: replace the clip under the playhead (or the In–Out range)",
                ),
            ];
            for (label, method, tip) in buttons {
                if ui.button(label).on_hover_text(tip).clicked() {
                    self.edit_function(method);
                }
            }
            ui.add_enabled(false, egui::Button::new("Place On Top"))
                .on_disabled_hover_text("F12: needs multi-track editing (M6)");
            ui.toggle_value(&mut self.video_only, "Video Only")
                .on_hover_text("Edits take only the picture (p. 694)");
        });
        let sources = self.edit_sources();
        let text = match sources.len() {
            0 => "No source: open a clip or select pool media".to_string(),
            1 => {
                let s = &sources[0];
                let media = s["media"].as_str().unwrap_or_default();
                let name = self
                    .project()
                    .and_then(|p| p.media(media))
                    .map_or(media.to_string(), |m| m.name.clone());
                format!("Source: {} ({})", elide(&name, 30), range_text(self, s))
            }
            n => format!("Source: {n} selected clips"),
        };
        ui.label(dim(text).small());
    }
}

fn range_text(app: &Workspace, source: &serde_json::Value) -> String {
    let r = source["range"].as_str().unwrap_or_default();
    let mut parts = r
        .split("..")
        .map(|x| x.trim_end_matches('f').parse::<i64>().unwrap_or(0));
    let (a, b) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    format!("{} long", app.src_tc(b - a))
}

/// Shortens `s` to `n` characters with an ellipsis.
pub(super) fn elide(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
