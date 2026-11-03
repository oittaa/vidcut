use std::time::Duration;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, Rect, RichText, Sense, Stroke, pos2, vec2,
};
use serde_json::{Value, json};
use vidcut_av::player::{Config, PacedOutput, Player};
use vidcut_core::model::{AngleSync, Media, Project};
use vidcut_core::multicam::{close_far_gaps, group_files, ruler_start, sync_clips_files};
use vidcut_core::render::{RenderPlan, places_sound};
use vidcut_core::time::{Rational, format_timecode};

use super::dialog;
use super::export::Ask;
use super::pool::elide;
use super::sync_bin::Film;
use super::theme;
use super::viewer::{ACTIVE_ANGLE, GridCell, grid_pictures};
use super::workspace::{Workspace, dim, fit};

const FAST_NUDGE: i64 = 5;
const RULER: f32 = 22.0;
const RULER_LABEL: f32 = 70.0;
const NUMBER: f32 = 26.0;
const LOCK: f32 = 24.0;
const MOST_ROW: f32 = 60.0;
const LEAST_ROW: f32 = 22.0;
const FOOTER: f32 = 56.0;

#[derive(Debug, Clone)]
struct Row {
    media: String,
    label: String,
    frames: i64,
    at: i64,
    hidden: i64,
    locked: bool,
}

impl Row {
    fn new(p: &Project, m: &Media, position: usize) -> Self {
        let label = match &m.camera {
            Some(camera) => format!("Camera {camera}"),
            None => format!("Camera {}", position + 1),
        };
        Self {
            media: m.id.clone(),
            label,
            frames: p.media_frames(m),
            at: 0,
            hidden: 0,
            locked: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    Places,
    AutoSync,
    Save,
}

struct Grab {
    row: usize,
    reals: Vec<i64>,
    press: f32,
    per_frame: f32,
}

#[derive(Default)]
pub(super) struct SyncClips {
    pub(super) open: bool,
    group: Option<String>,
    bin: String,
    rows: Vec<Row>,
    selected: usize,
    viewed: usize,
    sync: AngleSync,
    ruler: i64,
    at: i64,
    playing: bool,
    sound: Option<Player>,
    sounded: Vec<(String, i64)>,
    heard: Option<(bool, usize)>,
    solo: bool,
    grab: Option<Grab>,
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

    fn reals(&self) -> Vec<i64> {
        self.rows.iter().map(|r| r.at + r.hidden).collect()
    }

    fn place(&mut self, fps: Rational, reals: &[i64]) {
        let spans: Vec<(i64, i64)> = self
            .rows
            .iter()
            .zip(reals)
            .map(|(r, real)| (*real, r.frames))
            .collect();
        for (row, (at, hidden)) in self.rows.iter_mut().zip(close_far_gaps(fps, &spans)) {
            row.at = at;
            row.hidden = hidden;
        }
        self.at = self.at.min(self.frames());
    }

    fn nudge(&mut self, fps: Rational, by: i64) {
        if self.rows.get(self.selected).is_none_or(|r| r.locked) {
            return;
        }
        let mut reals = self.reals();
        reals[self.selected] = (reals[self.selected] + by).max(0);
        self.place(fps, &reals);
    }

    fn take_layout(&mut self, fps: Rational, layout: &Value, lock: bool) {
        let unmatched = layout["unmatched"].as_array();
        let unmatched: Vec<&str> = unmatched
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let mut reals = self.reals();
        for angle in layout["angles"].as_array().into_iter().flatten() {
            let media = angle["media"].as_str().unwrap_or_default();
            if let Some(i) = self.rows.iter().position(|r| r.media == media) {
                reals[i] = angle["at"].as_i64().unwrap_or(reals[i]);
                self.rows[i].locked = lock && !unmatched.contains(&media);
            }
        }
        self.place(fps, &reals);
    }

    fn places(&self) -> Vec<(String, i64)> {
        self.rows.iter().map(|r| (r.media.clone(), r.at)).collect()
    }

    fn gaps(&self) -> Vec<(i64, i64)> {
        let mut rows: Vec<(i64, i64)> = self.rows.iter().map(|r| (r.at, r.hidden)).collect();
        rows.sort_unstable();
        let mut gaps = vec![(0, 0)];
        for (at, hidden) in rows {
            if gaps.last().is_some_and(|(_, seen)| hidden > *seen) {
                gaps.push((at, hidden));
            }
        }
        gaps
    }

    fn timecode(&self, p: &Project, frame: i64) -> String {
        let gaps = self.gaps();
        let before = gaps.iter().rev().find(|(at, _)| *at <= frame);
        let hidden = before.map_or(0, |(_, hidden)| *hidden);
        let drop = p.settings.counts_drop_frame();
        format_timecode(self.ruler + frame + hidden, p.settings.fps, 0, drop)
    }

    fn hear(&mut self) {
        let wanted = (self.solo, self.viewed);
        if self.heard == Some(wanted) {
            return;
        }
        self.heard = Some(wanted);
        let Some(sound) = &self.sound else {
            return;
        };
        for lane in 0..self.rows.len() {
            let silent = self.solo && lane != self.viewed;
            sound.set_lane_gain(lane, if silent { f64::NEG_INFINITY } else { 0.0 });
        }
    }

    fn stop(&mut self) {
        self.playing = false;
        if let Some(sound) = &self.sound {
            sound.pause();
        }
    }
}

fn nudge_of(event: &egui::Event) -> i64 {
    let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return 0;
    };
    if modifiers.alt || modifiers.command {
        return 0;
    }
    let is = |wanted: Key| *key == wanted || *physical_key == Some(wanted);
    let step = if modifiers.shift { FAST_NUDGE } else { 1 };
    if is(Key::Comma) {
        -step
    } else if is(Key::Period) {
        step
    } else {
        0
    }
}

impl Workspace {
    pub(super) fn open_sync_clips(&mut self, ctx: &egui::Context) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let bin = self.snap.pool.bin.clone();
        let files = match sync_clips_files(&p, &bin, &self.snap.selection.media) {
            Ok(files) => files,
            Err(e) => {
                self.say(e.message);
                return;
            }
        };
        if let Some(g) = files.first().and_then(|m| p.group_of(&m.id)) {
            let id = g.id.clone();
            self.open_sync_group(&id);
            return;
        }
        let rows: Vec<Row> = files
            .iter()
            .enumerate()
            .map(|(i, m)| Row::new(&p, m, i))
            .collect();
        let media: Vec<String> = rows.iter().map(|r| r.media.clone()).collect();
        let params = json!({ "media": media, "sync": AngleSync::Timecode, "full_extents": true });
        let ask = Ask::send(&self.live, ctx, "multicam.layout", params);
        let ruler = ruler_start(&p, &media);
        self.show_sync_clips(SyncClips {
            bin,
            rows,
            ruler,
            waiting: Some((Waiting::Places, ask)),
            ..SyncClips::default()
        });
    }

    pub(super) fn open_sync_group(&mut self, id: &str) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(g) = p.group(id) else {
            return;
        };
        let rows: Vec<Row> = group_files(&p, g)
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let mut row = Row::new(&p, m, i);
                if let Some(a) = g.angles.iter().find(|a| a.media == m.id) {
                    row.at = a.at;
                    row.hidden = a.hidden;
                    row.locked = a.locked;
                }
                row
            })
            .collect();
        let media: Vec<String> = rows.iter().map(|r| r.media.clone()).collect();
        let mut window = SyncClips {
            group: Some(g.id.clone()),
            bin: g.bin.clone(),
            rows,
            ruler: ruler_start(&p, &media),
            ..SyncClips::default()
        };
        let reals = window.reals();
        window.place(p.settings.fps, &reals);
        self.show_sync_clips(window);
    }

    fn show_sync_clips(&mut self, window: SyncClips) {
        let solo = self.sync_clips.solo;
        self.close_sync_clips();
        self.send("player.pause", json!({}));
        self.sync_clips = SyncClips {
            open: true,
            solo,
            ..window
        };
    }

    pub(super) fn close_sync_clips(&mut self) {
        let s = &mut self.sync_clips;
        if let Some((_, ask)) = &s.waiting {
            ask.stop();
        }
        if let Some(sound) = s.sound.take() {
            let _ = crate::threads::closing(
                "vidcut-sync-clips-close",
                crate::threads::Work::Interactive,
                move || drop(sound),
            );
        }
        self.sync_clips = SyncClips {
            solo: s.solo,
            ..SyncClips::default()
        };
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
        let nudge: i64 = ctx.input(|i| i.events.iter().map(nudge_of).sum());
        if nudge != 0 {
            let fps = self.fps();
            self.sync_clips.nudge(fps, nudge);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Space)) {
            if self.sync_clips.playing {
                self.sync_clips.stop();
            } else {
                self.sync_clips_play();
            }
        }
    }

    fn sound_player(&mut self, plan: RenderPlan) -> Option<Player> {
        let screens = self.screens.as_ref();
        let gpu_pictures = screens.map(|s| s.gpu_pictures()).unwrap_or_default();
        let config = || Config {
            part: 1,
            audio_scrubbing: false,
            on_picture: None,
            hardware: crate::preferences::gpu_decoding().on,
            gpu_pictures: gpu_pictures.clone(),
        };
        match Player::new(plan.clone(), config(), super::app::audio_device()) {
            Ok(player) => return Some(player),
            Err(e) => self.say(e.to_string()),
        }
        let silent = Box::new(PacedOutput::new(Duration::from_millis(10), Duration::ZERO));
        match Player::new(plan, config(), silent) {
            Ok(player) => Some(player),
            Err(e) => {
                self.say(e.to_string());
                None
            }
        }
    }

    fn sync_clips_play(&mut self) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let places = self.sync_clips.places();
        let plan = match places_sound(&p, &places) {
            Ok(plan) => plan,
            Err(e) => {
                self.say(e.message);
                return;
            }
        };
        if self.sync_clips.sound.is_none() {
            self.sync_clips.sound = self.sound_player(plan.clone());
        }
        let at = self.sync_clips.at;
        let Some(sound) = &self.sync_clips.sound else {
            return;
        };
        if let Err(e) = sound.set_plan_at(plan, at) {
            self.say(e.to_string());
            return;
        }
        let s = &mut self.sync_clips;
        s.sounded = places;
        s.heard = None;
        s.hear();
        if let Some(sound) = &s.sound {
            sound.play(1.0);
        }
        s.playing = true;
    }

    fn sync_clips_follow(&mut self, ctx: &egui::Context, p: &Project) {
        if !self.sync_clips.playing {
            return;
        }
        let places = self.sync_clips.places();
        if places != self.sync_clips.sounded {
            let moved = places_sound(p, &places)
                .map_err(|e| e.message)
                .and_then(|plan| {
                    let sound = self.sync_clips.sound.as_ref();
                    sound.map_or(Ok(()), |s| s.set_plan(plan).map_err(|e| e.to_string()))
                });
            if let Err(message) = moved {
                self.sync_clips.stop();
                self.say(message);
                return;
            }
            self.sync_clips.sounded = places;
            self.sync_clips.heard = None;
        }
        let s = &mut self.sync_clips;
        s.hear();
        let Some(status) = s.sound.as_ref().map(Player::status) else {
            s.playing = false;
            return;
        };
        s.at = status.position;
        s.playing = status.playing;
        if let Some(error) = status.error {
            s.stop();
            self.say(error);
            return;
        }
        ctx.request_repaint();
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
        let fps = self.fps();
        match what {
            Waiting::Places => self.sync_clips.take_layout(fps, &answer.result, false),
            Waiting::AutoSync => {
                self.sync_clips.take_layout(fps, &answer.result, true);
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
        let params = json!({ "media": media, "sync": s.sync, "full_extents": true });
        let ask = Ask::send(&self.live, ctx, "multicam.layout", params);
        self.sync_clips.waiting = Some((Waiting::AutoSync, ask));
    }

    fn save_sync(&mut self, ctx: &egui::Context) {
        let s = &self.sync_clips;
        let angles: Vec<Value> = s
            .rows
            .iter()
            .map(
                |r| json!({ "media": r.media, "at": r.at, "hidden": r.hidden, "locked": r.locked }),
            )
            .collect();
        let (method, params) = match &s.group {
            Some(group) => ("multicam.set", json!({ "group": group, "angles": angles })),
            None => {
                let media: Vec<&str> = s.rows.iter().map(|r| r.media.as_str()).collect();
                let mut params = json!({ "media": media, "angles": angles });
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
        self.sync_clips_follow(ctx, &p);
        let mut open = true;
        let mut cancel = false;
        dialog::tool_window(ctx, "Sync Clips", &mut open, [980.0, 700.0], |ui| {
            self.sync_clips_pictures(ui, &p);
            self.sync_clips_bar(ui, &p);
            let least = RULER + self.sync_clips.rows.len() as f32 * LEAST_ROW;
            let height = (ui.available_height() - FOOTER).max(least);
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
        if let Some(row) = s.rows.get(s.viewed) {
            painter.text(
                pos2((middle + rect.right()) / 2.0, title),
                Align2::CENTER_TOP,
                elide(&self.media_name(&row.media), 60),
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
                label: Some(r.label.clone()),
                media: r.media.clone(),
                frame: frame_of(r),
                active: i == s.viewed,
                outlined: false,
            })
            .collect();
        let mut pictures = grid_pictures(left, p, cells.len());
        if let Some(r) = s.rows.get(s.viewed) {
            cells.push(GridCell {
                number: None,
                label: Some(r.label.clone()),
                media: r.media.clone(),
                frame: frame_of(r),
                active: false,
                outlined: false,
            });
            pictures.push(fit(right.shrink(6.0), aspect));
        }
        let speed = if s.playing { 1.0 } else { 0.0 };
        let picked = self.camera_cells(ui, &pictures, p, &cells, speed);
        let s = &mut self.sync_clips;
        if let Some(i) = picked.and_then(|m| s.rows.iter().position(|r| r.media == m)) {
            s.selected = i;
            s.viewed = i;
        }
    }

    fn sync_clips_bar(&mut self, ui: &mut egui::Ui, p: &Project) {
        let ctx = ui.ctx().clone();
        let waiting = self.sync_clips.waiting.is_some();
        let mut play = false;
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
                s.stop();
            }
            if ui.button(egui_phosphor::regular::PLAY).clicked() && !s.playing {
                play = true;
            }
            ui.label(dim(s.timecode(p, s.at)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let c = theme::colors();
                let color = if s.solo { c.ok } else { c.text };
                let solo = egui::Button::new(
                    RichText::new(egui_phosphor::regular::HEADPHONES)
                        .size(14.0)
                        .color(color),
                );
                if ui.add(solo).on_hover_text("Solo Audio").clicked() {
                    s.solo = !s.solo;
                }
            });
        });
        if play {
            self.sync_clips_play();
        }
    }

    fn sync_clips_rows(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Project) {
        let fps = p.settings.fps;
        let total = self.sync_clips.frames();
        let band = Rect::from_min_max(pos2(rect.left() + NUMBER + LOCK, rect.top()), rect.max);
        let per_frame = band.width() / (total as f32 * 1.1).max(1.0);
        let x = |frame: i64| band.left() + frame as f32 * per_frame;
        let ruler = Rect::from_min_size(band.min, vec2(band.width(), RULER));
        let painter = ui.painter_at(rect);
        self.sync_clips_ruler(&painter, ruler, per_frame, p);
        let scrub = ui.interact(
            ruler,
            ui.scope_id().with("sync-clips-ruler"),
            Sense::click_and_drag(),
        );
        if (scrub.clicked() || scrub.dragged())
            && let Some(pos) = scrub.interact_pointer_pos()
        {
            let s = &mut self.sync_clips;
            s.stop();
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
            let color = if i == self.sync_clips.viewed {
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
            let whole = Rect::from_min_max(pos2(rect.left(), strip.top()), strip.max);
            let picked = ui.interact(
                whole,
                ui.scope_id().with(("sync-clips-strip", i)),
                Sense::click(),
            );
            if picked.clicked() {
                self.sync_clips.selected = i;
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
            let s = &mut self.sync_clips;
            if drag.clicked() || drag.drag_started() {
                s.selected = i;
            }
            if drag.drag_started()
                && !row.locked
                && let Some(pos) = drag.interact_pointer_pos()
            {
                s.grab = Some(Grab {
                    row: i,
                    reals: s.reals(),
                    press: pos.x,
                    per_frame,
                });
            }
            if drag.dragged()
                && let Some(pos) = drag.interact_pointer_pos()
                && let Some(grab) = s.grab.as_ref().filter(|grab| grab.row == i)
            {
                let moved = ((pos.x - grab.press) / grab.per_frame).round() as i64;
                let mut reals = grab.reals.clone();
                reals[i] = (reals[i] + moved).max(0);
                s.place(fps, &reals);
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

    fn sync_clips_ruler(&self, painter: &egui::Painter, ruler: Rect, per_frame: f32, p: &Project) {
        let fps = p.settings.fps.as_f64();
        let s = &self.sync_clips;
        let last = ((ruler.width() / per_frame) as i64).max(s.frames());
        let every = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600]
            .into_iter()
            .map(|secs| (secs as f64 * fps).round() as i64)
            .find(|frames| *frames as f32 * per_frame >= 90.0)
            .unwrap_or(last.max(1));
        let room = (RULER_LABEL / per_frame) as i64;
        let x = |frame: i64| ruler.left() + frame as f32 * per_frame;
        let gaps = s.gaps();
        for (i, (from, _)) in gaps.iter().enumerate() {
            if i > 0 {
                let bar = Rect::from_min_max(
                    pos2(x(from - fps.round() as i64), ruler.top()),
                    pos2(x(*from), ruler.bottom()),
                );
                painter.rect_filled(bar, CornerRadius::ZERO, Color32::BLACK);
            }
            let until = gaps.get(i + 1).map_or(last + 1 + room, |(next, _)| *next);
            let mut frame = *from;
            while frame == *from || frame + room < until {
                let at = x(frame);
                painter.line_segment(
                    [pos2(at, ruler.top() + 13.0), pos2(at, ruler.bottom())],
                    Stroke::new(1.0, theme::colors().text_dim),
                );
                painter.text(
                    pos2(at + 2.0, ruler.top()),
                    Align2::LEFT_TOP,
                    s.timecode(p, frame),
                    FontId::proportional(10.0),
                    theme::colors().text_dim,
                );
                frame += every;
            }
        }
    }

    fn sync_clips_footer(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        let waiting = self.sync_clips.waiting.is_some();
        let playing = self.sync_clips.playing;
        let mut cancel = false;
        dialog::footer(ui, |ui| {
            if dialog::main_button(ui, "Save Sync", !waiting && !playing) {
                self.save_sync(&ctx);
            }
            if ui
                .add_enabled(!playing, egui::Button::new("Cancel"))
                .clicked()
            {
                cancel = true;
            }
        });
        cancel
    }
}
