use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, Rect, RichText, Sense, Stroke,
};
use eframe::egui::{pos2, vec2};
use serde_json::json;
use vidcut_core::model::Project;
use vidcut_core::multicam::{SyncRow, sync_rows};
use vidcut_core::request::ViewerMode;

use super::export::Ask;
use super::pool::FULL;
use super::theme;
use super::timeline::paint::wave_columns;
use super::title_browser::LeftTab;
use super::viewer::{ACTIVE_ANGLE, GridCell};
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

const PX_PER_SECOND: f32 = 90.0;
const RULER: f32 = 20.0;
const NUMBER: f32 = 26.0;
const MOST_ROW: f32 = 60.0;
const LEAST_ROW: f32 = 10.0;
const NUMBER_KEYS: [Key; 9] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

pub(super) struct Film<'a> {
    pub media: &'a str,
    pub frames: i64,
    pub head: i64,
    pub x: f32,
    pub per_frame: f32,
}

#[derive(Debug, Default)]
pub(super) struct SyncBin {
    past: Option<i64>,
    reached_end: bool,
    outlined: Option<String>,
    grab: Option<(i64, f32)>,
    edit: Option<Ask>,
}

impl Workspace {
    pub(super) fn sync_grid_on(&self) -> bool {
        self.left == LeftTab::SyncBin && self.snap.viewer == ViewerMode::Timeline
    }

    fn sync_at(&self) -> i64 {
        self.sync_bin.past.unwrap_or(self.snap.timeline.position)
    }

    fn sync_rows_at(&self, p: &Project, at: i64) -> vidcut_core::Result<Vec<SyncRow>> {
        sync_rows(p, &self.snap.pool.bin, at)
    }

    pub(super) fn sync_tick(&mut self) {
        if self.left != LeftTab::SyncBin {
            self.sync_bin.past = None;
            self.sync_bin.outlined = None;
        }
        if self.sync_bin.past.is_some() {
            let timeline = &self.snap.timeline;
            if timeline.position + 1 >= timeline.frames {
                self.sync_bin.reached_end = true;
            } else if self.sync_bin.reached_end {
                self.sync_bin.past = None;
            }
        }
        let Some(result) = self.sync_bin.edit.as_ref().and_then(Ask::take) else {
            return;
        };
        self.sync_bin.edit = None;
        match result {
            Ok(r) => {
                let stays = self.sync_bin.past == r.result["playhead"].as_i64();
                if !stays {
                    self.sync_bin.past = None;
                }
                self.sync_bin.reached_end = false;
                self.show_timeline();
            }
            Err(e) => self.say(e.message),
        }
    }

    pub(super) fn sync_overwrite(&mut self, ctx: &egui::Context) {
        let Some((media, _)) = self.snap.source_at() else {
            self.say("Select a Camera First");
            return;
        };
        let mut params = json!({ "media": media, "at": self.sync_at() });
        if self.project().is_some_and(|p| p.group_of(media).is_some()) {
            params["group"] = json!(true);
        }
        self.only_toggles(&mut params);
        self.sync_bin.edit = Some(Ask::send(&self.live, ctx, "edit.source_overwrite", params));
    }

    fn pick_camera(&mut self, row: &SyncRow) {
        if row.frame.is_none() {
            self.sync_bin.outlined = Some(row.media.clone());
            return;
        }
        self.sync_bin.outlined = None;
        let at = self.sync_at();
        self.send("multicam.pick", json!({ "media": row.media, "at": at }));
        self.send("viewer.load", json!({ "media": row.media, "at": "in" }));
    }

    pub(super) fn sync_keys(&mut self, ctx: &egui::Context) {
        if self.left != LeftTab::SyncBin {
            return;
        }
        if self.snap.viewer == ViewerMode::SourceClip
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.show_timeline();
            return;
        }
        let number = ctx.input_mut(|i| {
            NUMBER_KEYS
                .iter()
                .position(|k| i.consume_key(egui::Modifiers::NONE, *k))
        });
        let (Some(number), Some(p)) = (number, self.project_shared()) else {
            return;
        };
        let rows = self.sync_rows_at(&p, self.sync_at());
        match rows {
            Ok(rows) => {
                if let Some(row) = rows.get(number) {
                    self.pick_camera(row);
                }
            }
            Err(e) => self.say(e.message),
        }
    }

    pub(super) fn sync_close_button(&mut self, ui: &mut egui::Ui, picture: Rect) {
        if self.left != LeftTab::SyncBin || self.snap.viewer != ViewerMode::SourceClip {
            return;
        }
        let at = Rect::from_min_size(picture.left_top() + vec2(8.0, 8.0), vec2(24.0, 24.0));
        let close = ui
            .put(at, egui::Button::new(egui_phosphor::regular::X_CIRCLE))
            .on_hover_text("Close Camera");
        if close.clicked() {
            self.show_timeline();
        }
    }

    pub(super) fn sync_grid(&mut self, ui: &mut egui::Ui, area: Rect) {
        let Some(p) = self.shown_project() else {
            return;
        };
        let timeline = &self.snap.timeline;
        let speed = if timeline.playing {
            timeline.speed
        } else {
            0.0
        };
        let rows = self
            .sync_rows_at(&p, self.sync_at())
            .map_err(|e| e.message)
            .and_then(|rows| {
                if rows.is_empty() {
                    Err("No Files in This Bin".to_string())
                } else {
                    Ok(rows)
                }
            });
        match rows {
            Ok(rows) => {
                let cells: Vec<GridCell> = rows
                    .iter()
                    .map(|row| GridCell {
                        number: Some(row.number),
                        label: Some(format!("Camera {}", row.camera)),
                        media: row.media.clone(),
                        frame: row.frame,
                        active: row.active,
                        outlined: self.sync_bin.outlined.as_deref() == Some(row.media.as_str()),
                    })
                    .collect();
                let picked = self.camera_grid(ui, area, &p, &cells, speed);
                if let Some(row) = picked.and_then(|m| rows.iter().find(|r| r.media == m)) {
                    self.pick_camera(row);
                }
            }
            Err(why) => {
                ui.allocate_rect(area, Sense::hover());
                ui.painter_at(area).text(
                    area.center(),
                    Align2::CENTER_CENTER,
                    why,
                    FontId::proportional(14.0),
                    theme::colors().text_dim,
                );
                if let Some(cells) = &mut self.cells {
                    cells.close();
                }
            }
        }
        ui.add_space(4.0);
        self.clip_edge_hint(ui);
        self.transport(ui, PlayerId::Timeline, None);
    }

    pub(super) fn sync_bin_panel(&mut self, ui: &mut egui::Ui) {
        self.hover = None;
        self.pool_focus = false;
        let Some(p) = self.project_shared() else {
            ui.label(dim("Open a project to use the Sync Bin"));
            return;
        };
        let at = self.sync_at();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sync Bin").strong());
            ui.label(dim(self.tc(at)));
            ui.label(dim(self.snap.pool.bin.clone()));
        });
        ui.separator();
        let height = (ui.available_height() - self.pool_buttons).max(60.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        match self.sync_rows_at(&p, at) {
            Ok(rows) if rows.is_empty() => {
                inside.label(dim("No Files in This Bin"));
            }
            Ok(rows) => self.sync_strips(&mut inside, rect, &p, &rows, at),
            Err(e) => {
                inside.label(RichText::new(e.message).color(theme::colors().error));
            }
        }
        let top = ui.cursor().top();
        ui.separator();
        self.edit_buttons(ui);
        let buttons = ui.cursor().top() - top;
        if (buttons - self.pool_buttons).abs() > 0.5 {
            self.pool_buttons = buttons;
            ui.ctx().request_repaint();
        }
    }

    fn sync_strips(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        p: &Project,
        rows: &[SyncRow],
        at: i64,
    ) {
        let per_frame = PX_PER_SECOND / p.settings.fps.as_f64().max(1.0) as f32;
        let band = Rect::from_min_max(pos2(rect.left() + NUMBER, rect.top()), rect.max);
        let mid = band.center().x;
        let x = |t: i64| mid + (t - at) as f32 * per_frame;
        let painter = ui.painter_at(rect);
        let ruler = Rect::from_min_size(band.min, vec2(band.width(), RULER));
        self.sync_ruler(&painter, ruler, p, at, per_frame);
        let row_height = ((rect.height() - RULER) / rows.len() as f32).clamp(LEAST_ROW, MOST_ROW);
        let mut picked = None;
        for (i, row) in rows.iter().enumerate() {
            let strip = Rect::from_min_size(
                pos2(band.left(), ruler.bottom() + i as f32 * row_height),
                vec2(band.width(), row_height - 3.0),
            );
            painter.rect_filled(strip, CornerRadius::same(2), theme::colors().track);
            let number = Rect::from_min_size(
                pos2(rect.left(), strip.top()),
                vec2(NUMBER - 4.0, strip.height()),
            );
            let color = if row.active {
                ACTIVE_ANGLE
            } else {
                theme::colors().text
            };
            painter.text(
                number.center(),
                Align2::CENTER_CENTER,
                row.number.to_string(),
                FontId::proportional(strip.height().clamp(8.0, 13.0)),
                color,
            );
            let clicked = ui
                .interact(
                    number,
                    ui.scope_id().with(("sync-camera", i)),
                    Sense::click(),
                )
                .clicked();
            if clicked {
                picked = Some(row.clone());
            }
            for clip in &row.clips {
                let Some(start) = clip.start else {
                    continue;
                };
                let film = Film {
                    media: &clip.media,
                    frames: clip.frames,
                    head: clip.head,
                    x: x(start),
                    per_frame,
                };
                self.sync_film(ui, strip, p, &film);
            }
        }
        let bottom = (ruler.bottom() + rows.len() as f32 * row_height).min(rect.bottom());
        painter.line_segment(
            [pos2(mid, band.top()), pos2(mid, bottom)],
            Stroke::new(1.5, ACTIVE_ANGLE),
        );
        let area = Rect::from_min_max(band.min, pos2(band.right(), bottom));
        let drag = ui.interact(area, ui.scope_id().with("sync-strips"), Sense::drag());
        if drag.drag_started()
            && let Some(pos) = drag.interact_pointer_pos()
        {
            self.sync_bin.grab = Some((at, pos.x));
        }
        if let (true, Some((from, press)), Some(pos)) = (
            drag.dragged(),
            self.sync_bin.grab,
            drag.interact_pointer_pos(),
        ) {
            let to = from - ((pos.x - press) / per_frame).round() as i64;
            self.sync_move(to, rows);
        }
        if drag.drag_stopped() {
            self.sync_bin.grab = None;
        }
        if let Some(row) = picked {
            self.pick_camera(&row);
        }
    }

    fn sync_move(&mut self, to: i64, rows: &[SyncRow]) {
        let last = (self.snap.timeline.frames - 1).max(0);
        let media_end = rows
            .iter()
            .flat_map(|r| &r.clips)
            .filter_map(|c| c.start.map(|s| s + c.frames))
            .max()
            .unwrap_or(0);
        let to = to.clamp(0, media_end.max(last));
        if to > last {
            self.sync_bin.past = Some(to);
            self.sync_bin.reached_end = false;
            self.seek(PlayerId::Timeline, last, true);
        } else {
            self.sync_bin.past = None;
            self.seek(PlayerId::Timeline, to, true);
        }
    }

    fn sync_ruler(
        &self,
        painter: &egui::Painter,
        ruler: Rect,
        p: &Project,
        at: i64,
        per_frame: f32,
    ) {
        let second = p.settings.fps.as_f64().round().max(1.0) as i64;
        let start_tc = p.settings.start_tc;
        let left = at + ((ruler.left() - ruler.center().x) / per_frame) as i64;
        let right = at + ((ruler.right() - ruler.center().x) / per_frame) as i64;
        let first = (start_tc + left.max(0)).div_euclid(second) + 1;
        let x = |t: i64| ruler.center().x + (t - at) as f32 * per_frame;
        for n in first.. {
            let t = n * second - start_tc;
            if t > right {
                break;
            }
            let tall = n % 2 == 0;
            let top = if tall {
                ruler.top() + 11.0
            } else {
                ruler.top() + 15.0
            };
            painter.line_segment(
                [pos2(x(t), top), pos2(x(t), ruler.bottom())],
                Stroke::new(1.0, theme::colors().text_dim),
            );
            if tall {
                painter.text(
                    pos2(x(t), ruler.top()),
                    Align2::CENTER_TOP,
                    p.settings.timecode(t),
                    FontId::proportional(10.0),
                    theme::colors().text_dim,
                );
            }
        }
    }

    pub(super) fn sync_film(&self, ui: &egui::Ui, strip: Rect, p: &Project, film: &Film<'_>) {
        let Some(m) = p.media(film.media) else {
            return;
        };
        let span = Rect::from_min_max(
            pos2(film.x.max(strip.left()), strip.top()),
            pos2(
                (film.x + film.frames as f32 * film.per_frame).min(strip.right()),
                strip.bottom(),
            ),
        );
        if span.width() <= 0.0 {
            return;
        }
        let clipped = ui.painter_at(span);
        if m.info.video.is_none() {
            clipped.rect_filled(span, CornerRadius::ZERO, theme::colors().audio);
            if let Some(w) = self.snap.waveforms.get(&m.id) {
                let origin = m.info.audio.first().map_or(0.0, |a| a.first_pts);
                let secs = p.settings.fps.frames_to_secs(1);
                let whole = Rect::from_min_max(pos2(film.x, strip.top()), span.max);
                wave_columns(&clipped, whole, |x| {
                    let frames = f64::from((x - film.x) / film.per_frame);
                    let t = origin + (film.head as f64 + frames) * secs;
                    w.range(t, t + secs / f64::from(film.per_frame))
                });
            }
            return;
        }
        clipped.rect_filled(span, CornerRadius::ZERO, theme::colors().panel_dark);
        let aspect = p.settings.width as f32 / p.settings.height.max(1) as f32;
        let wide = strip.height() * aspect;
        let step = ((wide / film.per_frame).ceil() as i64).max(1);
        let hidden = ((span.left() - film.x) / film.per_frame) as i64;
        let mut frame = hidden.max(0) / step * step;
        while frame < film.frames {
            let left = film.x + frame as f32 * film.per_frame;
            if left >= span.right() {
                break;
            }
            let tile = Rect::from_min_size(pos2(left, strip.top()), vec2(wide, strip.height()));
            let source = film.head + frame;
            if let Some(shown) = self.pool_picture(ui.ctx(), m, source, tile.height(), step) {
                clipped.image(shown, tile, FULL, Color32::WHITE);
            }
            frame += step;
        }
    }
}
