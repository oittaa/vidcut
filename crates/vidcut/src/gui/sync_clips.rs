use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, Rect, RichText, Sense, Stroke, pos2, vec2,
};
use serde_json::{Value, json};
use vidcut_core::model::{AngleSync, Project};
use vidcut_core::multicam::{camera_order, cameras, hand_start};
use vidcut_core::time::format_timecode;

use super::dialog;
use super::export::Ask;
use super::pool::elide;
use super::sync_bin::Film;
use super::theme;
use super::viewer::{ACTIVE_ANGLE, GridCell, grid_pictures};
use super::workspace::{Workspace, dim, fit};

const FAST_NUDGE: i64 = 5;
const RULER: f32 = 22.0;
const NUMBER: f32 = 26.0;
const LOCK: f32 = 24.0;
const MOST_ROW: f32 = 60.0;
const LEAST_ROW: f32 = 22.0;
const FOOTER: f32 = 56.0;

#[derive(Debug, Clone)]
struct Row {
    media: String,
    frames: i64,
    at: i64,
    locked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    Places,
    AutoSync(AngleSync),
    Save,
}

#[derive(Debug, Default)]
pub(super) struct SyncClips {
    pub(super) open: bool,
    group: Option<String>,
    bin: String,
    rows: Vec<Row>,
    selected: usize,
    sync: AngleSync,
    used: AngleSync,
    start: i64,
    at: i64,
    playing: Option<(Instant, i64)>,
    grab: Option<(usize, i64, f32)>,
    waiting: Option<(Waiting, Ask)>,
    failed: Vec<String>,
}

impl SyncClips {
    fn frames(&self) -> i64 {
        self.rows
            .iter()
            .map(|r| r.at + r.frames)
            .max()
            .unwrap_or(0)
            .max(1)
    }

    fn nudge(&mut self, by: i64) {
        if let Some(row) = self.rows.get_mut(self.selected).filter(|r| !r.locked) {
            row.at = (row.at + by).max(0);
        }
    }

    fn take_layout(&mut self, p: &Project, layout: &Value, lock: bool) {
        if self.group.is_none() {
            let ids: Vec<String> = self.rows.iter().map(|r| r.media.clone()).collect();
            let first = layout["start"].as_i64().unwrap_or(0);
            self.start = hand_start(p, &ids, self.used, first);
        }
        let unmatched = layout["unmatched"].as_array();
        let unmatched: Vec<&str> = unmatched
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        for angle in layout["angles"].as_array().into_iter().flatten() {
            let media = angle["media"].as_str().unwrap_or_default();
            if let Some(row) = self.rows.iter_mut().find(|r| r.media == media) {
                row.at = angle["at"].as_i64().unwrap_or(row.at);
                row.locked = lock && !unmatched.contains(&media);
            }
        }
    }
}

impl Workspace {
    pub(super) fn open_sync_clips(&mut self, ctx: &egui::Context) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let bin = self.snap.pool.bin.clone();
        let files = match cameras(&p, &bin) {
            Ok(files) => files,
            Err(e) => {
                self.say(e.message);
                return;
            }
        };
        if files.len() < 2 {
            self.say("A Sync Needs Two Files or More");
            return;
        }
        if let Some(g) = files.first().and_then(|(m, _)| p.group_of(&m.id)) {
            let id = g.id.clone();
            self.open_sync_group(&id);
            return;
        }
        let rows: Vec<Row> = files
            .iter()
            .map(|(m, _)| Row {
                media: m.id.clone(),
                frames: p.media_frames(m),
                at: 0,
                locked: false,
            })
            .collect();
        let media: Vec<&str> = rows.iter().map(|r| r.media.as_str()).collect();
        let params = json!({ "media": media, "sync": AngleSync::Timecode, "full_extents": true });
        let ask = Ask::send(&self.live, ctx, "multicam.layout", params);
        self.sync_clips = SyncClips {
            open: true,
            bin,
            rows,
            waiting: Some((Waiting::Places, ask)),
            ..SyncClips::default()
        };
    }

    pub(super) fn open_sync_group(&mut self, id: &str) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(g) = p.group(id) else {
            return;
        };
        let mut files: Vec<_> = g
            .angles
            .iter()
            .filter_map(|a| Some((p.media(&a.media)?, a)))
            .collect();
        files.sort_by(|(a, _), (b, _)| {
            camera_order(
                a.camera.as_deref().unwrap_or(&a.name),
                b.camera.as_deref().unwrap_or(&b.name),
            )
        });
        let rows = files
            .iter()
            .map(|(m, a)| Row {
                media: m.id.clone(),
                frames: p.media_frames(m),
                at: a.at,
                locked: a.locked,
            })
            .collect();
        self.sync_clips = SyncClips {
            open: true,
            group: Some(g.id.clone()),
            bin: g.bin.clone(),
            rows,
            start: g.start,
            ..SyncClips::default()
        };
    }

    pub(super) fn close_sync_clips(&mut self) {
        if let Some((_, ask)) = &self.sync_clips.waiting {
            ask.stop();
        }
        self.sync_clips = SyncClips::default();
    }

    pub(super) fn sync_clips_keys(&mut self, ctx: &egui::Context) {
        if !self.sync_clips.open {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if self.sync_clips.failed.is_empty() {
                self.close_sync_clips();
            } else {
                self.sync_clips.failed.clear();
            }
            return;
        }
        let nudge = ctx.input_mut(|i| {
            [
                (egui::Modifiers::NONE, Key::Comma, -1),
                (egui::Modifiers::NONE, Key::Period, 1),
                (egui::Modifiers::SHIFT, Key::Comma, -FAST_NUDGE),
                (egui::Modifiers::SHIFT, Key::Period, FAST_NUDGE),
            ]
            .into_iter()
            .find(|(modifiers, key, _)| i.consume_key(*modifiers, *key))
            .map(|(.., by)| by)
        });
        if let Some(by) = nudge {
            self.sync_clips.nudge(by);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Space)) {
            let s = &mut self.sync_clips;
            s.playing = match s.playing {
                Some(_) => None,
                None => Some((Instant::now(), s.at)),
            };
        }
    }

    fn sync_clips_answer(&mut self) {
        let Some((what, result)) = self
            .sync_clips
            .waiting
            .as_ref()
            .and_then(|(what, ask)| Some((*what, ask.take()?)))
        else {
            return;
        };
        self.sync_clips.waiting = None;
        let answer = match result {
            Ok(answer) => answer,
            Err(e) => {
                self.say(e.message);
                return;
            }
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        match what {
            Waiting::Places => self.sync_clips.take_layout(&p, &answer.result, false),
            Waiting::AutoSync(sync) => {
                self.sync_clips.used = sync;
                self.sync_clips.take_layout(&p, &answer.result, true);
                self.sync_clips.failed = answer.result["unmatched"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|m| self.media_name(m))
                    .collect();
            }
            Waiting::Save => self.close_sync_clips(),
        }
    }

    fn auto_sync(&mut self, ctx: &egui::Context) {
        let s = &self.sync_clips;
        let media: Vec<&str> = s.rows.iter().map(|r| r.media.as_str()).collect();
        let sync = s.sync;
        let params = json!({ "media": media, "sync": sync, "full_extents": true });
        let ask = Ask::send(&self.live, ctx, "multicam.layout", params);
        self.sync_clips.waiting = Some((Waiting::AutoSync(sync), ask));
    }

    fn save_sync(&mut self, ctx: &egui::Context) {
        let s = &self.sync_clips;
        let angles: Vec<Value> = s
            .rows
            .iter()
            .map(|r| json!({ "media": r.media, "at": r.at, "locked": r.locked }))
            .collect();
        let (method, params) = match &s.group {
            Some(group) => ("multicam.set", json!({ "group": group, "angles": angles })),
            None => {
                let media: Vec<&str> = s.rows.iter().map(|r| r.media.as_str()).collect();
                let mut params = json!({ "media": media, "sync": s.used, "angles": angles });
                if !s.bin.is_empty() {
                    params["bin"] = json!(s.bin);
                }
                ("multicam.create", params)
            }
        };
        let ask = Ask::send(&self.live, ctx, method, params);
        self.sync_clips.waiting = Some((Waiting::Save, ask));
    }

    pub(super) fn sync_clips_window(&mut self, ctx: &egui::Context) {
        self.sync_clips_answer();
        if !self.sync_clips.open {
            return;
        }
        let Some(p) = self.project_shared() else {
            self.close_sync_clips();
            return;
        };
        if let Some((from_time, from)) = self.sync_clips.playing {
            let end = self.sync_clips.frames();
            let fps = p.settings.fps.as_f64();
            let at = from + (from_time.elapsed().as_secs_f64() * fps) as i64;
            self.sync_clips.at = at.min(end);
            if at >= end {
                self.sync_clips.playing = None;
            }
            ctx.request_repaint();
        }
        let mut open = true;
        let mut cancel = false;
        dialog::tool_window(ctx, "Sync Clips", &mut open, [980.0, 700.0], |ui| {
            self.sync_clips_pictures(ui, &p);
            self.sync_clips_bar(ui);
            let height = (ui.available_height() - FOOTER).max(120.0);
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
            self.sync_clips_rows(ui, rect, &p);
            cancel = self.sync_clips_footer(ui);
        });
        if !open || cancel {
            self.close_sync_clips();
            return;
        }
        self.sync_clips_failed(ctx);
    }

    fn sync_clips_failed(&mut self, ctx: &egui::Context) {
        if self.sync_clips.failed.is_empty() {
            return;
        }
        let names = self.sync_clips.failed.clone();
        let mut open = true;
        let mut done = false;
        dialog::window(ctx, "Auto Sync Failed", &mut open, |ui| {
            ui.label("No match was found when attempting to align these clips.");
            for name in &names {
                ui.label(dim(name.clone()));
            }
            dialog::footer(ui, |ui| {
                done = dialog::main_button(ui, "OK", true);
            });
        });
        if !open || done {
            self.sync_clips.failed.clear();
        }
    }

    fn sync_clips_pictures(&mut self, ui: &mut egui::Ui, p: &Project) {
        let height = (ui.available_height() * 0.45).max(160.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(12, 12, 13));
        let middle = rect.center().x;
        let title = rect.top() + 2.0;
        painter.text(
            pos2((rect.left() + middle) / 2.0, title),
            Align2::CENTER_TOP,
            "Sync View",
            FontId::proportional(12.0),
            theme::colors().text_dim,
        );
        let s = &self.sync_clips;
        let selected = s.rows.get(s.selected).map(|r| r.media.clone());
        if let Some(media) = &selected {
            painter.text(
                pos2((middle + rect.right()) / 2.0, title),
                Align2::CENTER_TOP,
                elide(&self.media_name(media), 60),
                FontId::proportional(12.0),
                theme::colors().text_dim,
            );
        }
        let top = rect.top() + 18.0;
        let left = Rect::from_min_max(pos2(rect.left(), top), pos2(middle, rect.bottom()));
        let right = Rect::from_min_max(pos2(middle, top), rect.max);
        let aspect = p.settings.width as f32 / p.settings.height.max(1) as f32;
        let frame_of = |r: &Row| {
            let frame = s.at - r.at;
            (0..r.frames).contains(&frame).then_some(frame)
        };
        let mut cells: Vec<GridCell> = s
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| GridCell {
                number: Some(i + 1),
                media: r.media.clone(),
                frame: frame_of(r),
                active: i == s.selected,
                outlined: false,
            })
            .collect();
        let mut pictures = grid_pictures(left, p, cells.len());
        if let Some(r) = s.rows.get(s.selected) {
            cells.push(GridCell {
                number: None,
                media: r.media.clone(),
                frame: frame_of(r),
                active: false,
                outlined: false,
            });
            pictures.push(fit(right.shrink(6.0), aspect));
        }
        let speed = if s.playing.is_some() { 1.0 } else { 0.0 };
        let picked = self.camera_cells(ui, &pictures, p, &cells, speed);
        let s = &mut self.sync_clips;
        if let Some(i) = picked.and_then(|m| s.rows.iter().position(|r| r.media == m)) {
            s.selected = i;
        }
    }

    fn sync_clips_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let waiting = self.sync_clips.waiting.is_some();
        ui.horizontal(|ui| {
            ui.label(dim("Sync By"));
            dialog::switch(ui, |ui| {
                for (mode, label) in [
                    (AngleSync::Timecode, "Timecode"),
                    (AngleSync::Sound, "Sound"),
                    (AngleSync::In, "In"),
                    (AngleSync::Out, "Out"),
                ] {
                    if ui
                        .selectable_label(self.sync_clips.sync == mode, label)
                        .clicked()
                    {
                        self.sync_clips.sync = mode;
                    }
                }
            });
            if ui
                .add_enabled(!waiting, egui::Button::new("Auto Sync"))
                .clicked()
            {
                self.auto_sync(&ctx);
            }
            if waiting {
                ui.spinner();
            }
            ui.add_space(24.0);
            let s = &mut self.sync_clips;
            let locked = s.rows.get(s.selected).is_some_and(|r| r.locked);
            let lock = egui::Button::selectable(
                locked,
                RichText::new(egui_phosphor::regular::ARROWS_CLOCKWISE).size(14.0),
            );
            if ui.add(lock).on_hover_text("Sync Lock").clicked()
                && let Some(row) = s.rows.get_mut(s.selected)
            {
                row.locked = !row.locked;
            }
            if ui.button(egui_phosphor::regular::STOP).clicked() {
                s.playing = None;
            }
            if ui.button(egui_phosphor::regular::PLAY).clicked() && s.playing.is_none() {
                s.playing = Some((Instant::now(), s.at));
            }
            let at = format_timecode(s.start + s.at, self.fps(), 0, false);
            ui.label(dim(at));
        });
    }

    fn sync_clips_rows(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Project) {
        let total = self.sync_clips.frames();
        let band = Rect::from_min_max(pos2(rect.left() + NUMBER + LOCK, rect.top()), rect.max);
        let per_frame = band.width() / (total as f32 * 1.1).max(1.0);
        let x = |frame: i64| band.left() + frame as f32 * per_frame;
        let ruler = Rect::from_min_size(band.min, vec2(band.width(), RULER));
        let painter = ui.painter_at(rect);
        self.sync_clips_ruler(&painter, ruler, per_frame);
        let scrub = ui.interact(
            ruler,
            ui.scope_id().with("sync-clips-ruler"),
            Sense::click_and_drag(),
        );
        if (scrub.clicked() || scrub.dragged())
            && let Some(pos) = scrub.interact_pointer_pos()
        {
            let s = &mut self.sync_clips;
            s.playing = None;
            s.at = (((pos.x - band.left()) / per_frame).round() as i64).clamp(0, total);
        }
        let count = self.sync_clips.rows.len().max(1);
        let row_height = ((rect.height() - RULER) / count as f32).clamp(LEAST_ROW, MOST_ROW);
        for i in 0..self.sync_clips.rows.len() {
            let row = self.sync_clips.rows[i].clone();
            let selected = i == self.sync_clips.selected;
            let top = ruler.bottom() + i as f32 * row_height;
            let strip =
                Rect::from_min_size(pos2(band.left(), top), vec2(band.width(), row_height - 3.0));
            painter.rect_filled(strip, CornerRadius::same(2), theme::colors().track);
            let color = if selected {
                ACTIVE_ANGLE
            } else {
                theme::colors().text
            };
            painter.text(
                pos2(rect.left() + NUMBER / 2.0, strip.center().y),
                Align2::CENTER_CENTER,
                (i + 1).to_string(),
                FontId::proportional(13.0),
                color,
            );
            if row.locked {
                painter.text(
                    pos2(rect.left() + NUMBER + LOCK / 2.0, strip.center().y),
                    Align2::CENTER_CENTER,
                    egui_phosphor::regular::ARROWS_CLOCKWISE,
                    FontId::proportional(13.0),
                    theme::colors().text,
                );
            }
            let film = Film {
                media: &row.media,
                frames: row.frames,
                x: x(row.at),
                per_frame,
            };
            self.sync_film(ui, strip, p, &film);
            let span = Rect::from_min_max(
                pos2(film.x, strip.top()),
                pos2(film.x + row.frames as f32 * per_frame, strip.bottom()),
            )
            .intersect(strip);
            if selected {
                painter.rect_stroke(
                    span,
                    CornerRadius::ZERO,
                    Stroke::new(1.5, ACTIVE_ANGLE),
                    egui::StrokeKind::Inside,
                );
            }
            let drag = ui.interact(
                span,
                ui.scope_id().with(("sync-clips-row", i)),
                Sense::click_and_drag(),
            );
            if drag.clicked() || drag.drag_started() {
                self.sync_clips.selected = i;
            }
            let s = &mut self.sync_clips;
            if drag.drag_started()
                && !row.locked
                && let Some(pos) = drag.interact_pointer_pos()
            {
                s.grab = Some((i, row.at, pos.x));
            }
            if drag.dragged()
                && let (Some((who, from, press)), Some(pos)) = (s.grab, drag.interact_pointer_pos())
                && who == i
            {
                let moved = ((pos.x - press) / per_frame).round() as i64;
                s.rows[i].at = (from + moved).max(0);
            }
            if drag.drag_stopped() {
                s.grab = None;
            }
        }
        let bottom = (ruler.bottom() + count as f32 * row_height).min(rect.bottom());
        let line = x(self.sync_clips.at);
        painter.line_segment(
            [pos2(line, band.top()), pos2(line, bottom)],
            Stroke::new(1.5, ACTIVE_ANGLE),
        );
    }

    fn sync_clips_ruler(&self, painter: &egui::Painter, ruler: Rect, per_frame: f32) {
        let fps = self.fps();
        let start = self.sync_clips.start;
        let total = self.sync_clips.frames();
        let last = ((ruler.width() / per_frame) as i64).max(total);
        let every = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600]
            .into_iter()
            .map(|secs| (secs as f64 * fps.as_f64()).round() as i64)
            .find(|frames| *frames as f32 * per_frame >= 90.0)
            .unwrap_or(last.max(1));
        let mut frame = 0;
        while frame <= last {
            let at = ruler.left() + frame as f32 * per_frame;
            painter.line_segment(
                [pos2(at, ruler.top() + 13.0), pos2(at, ruler.bottom())],
                Stroke::new(1.0, theme::colors().text_dim),
            );
            painter.text(
                pos2(at + 2.0, ruler.top()),
                Align2::LEFT_TOP,
                format_timecode(start + frame, fps, 0, false),
                FontId::proportional(10.0),
                theme::colors().text_dim,
            );
            frame += every;
        }
    }

    fn sync_clips_footer(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        let waiting = self.sync_clips.waiting.is_some();
        let mut cancel = false;
        dialog::footer(ui, |ui| {
            if dialog::main_button(ui, "Save Sync", !waiting) {
                self.save_sync(&ctx);
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
        cancel
    }
}
