use eframe::egui::{self, Color32, CornerRadius, Painter, Rect, RichText};
use serde_json::{Value, json};
use vidcut_core::model::Project;
use vidcut_core::ops;

use super::{EditSel, Side, TrackRef, audio_edit_outgoing};
use crate::gui::export::{Ask, near_center};
use crate::gui::theme;
use crate::gui::timeline::geometry::{Rows, Scale};
use crate::gui::workspace::{Workspace, dim};
use crate::live::PlayerId;

#[derive(Debug)]
pub struct SilenceDialog {
    pub clips: Vec<String>,
    pub threshold_db: f64,
    pub pre_head: f64,
    pub post_tail: f64,
    pub min: f64,
    pub parts: Vec<(String, i64, i64)>,
    pub frames: i64,
    pub error: Option<String>,
    asked: Option<(Value, Ask)>,
    found_for: Option<Value>,
}

impl SilenceDialog {
    pub fn new(clips: Vec<String>) -> Self {
        Self {
            clips,
            threshold_db: -40.0,
            pre_head: 0.15,
            post_tail: 0.15,
            min: 0.5,
            parts: Vec::new(),
            frames: 0,
            error: None,
            asked: None,
            found_for: None,
        }
    }

    pub fn params(&self) -> Value {
        json!({
            "clips": self.clips,
            "threshold_db": self.threshold_db,
            "pre_head": format!("{}s", self.pre_head),
            "post_tail": format!("{}s", self.post_tail),
            "min": format!("{}s", self.min),
        })
    }

    pub fn current(&self) -> bool {
        self.found_for.as_ref() == Some(&self.params())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum DynamicMode {
    #[default]
    Slip,
    Slide,
}

#[derive(Debug, Default)]
pub struct Dynamic {
    run: Option<(f64, f64, i64)>,
    pub pending: i64,
    around_until: Option<i64>,
    pub stopped: Option<String>,
}

impl Dynamic {
    pub fn speed(&self) -> f64 {
        self.run.map_or(0.0, |(speed, ..)| speed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DynamicTarget {
    Edit(EditSel),
    Clips(Vec<String>),
}

pub fn dynamic_preview(
    p: &mut Project,
    target: &DynamicTarget,
    mode: DynamicMode,
    by: i64,
) -> vidcut_core::Result<(i64, i64)> {
    match target {
        DynamicTarget::Edit(e) => {
            if e.side == Side::Audio {
                let id = p
                    .track(&e.track.id())
                    .and_then(|t| audio_edit_outgoing(t, e.at))
                    .ok_or_else(|| vidcut_core::Error::not_found("no audio edit"))?;
                ops::audio_roll(p, &id, ops::Edge::Tail, by)?;
                let at = e.at + by;
                return Ok((at - 1, at));
            }
            let t = p
                .track(&e.track.id())
                .ok_or_else(|| vidcut_core::Error::not_found(format!("no {}", e.track.id())))?;
            let out = t
                .clips
                .iter()
                .find(|c| c.end() == e.at)
                .map(|c| c.id.clone());
            let inc = t
                .clips
                .iter()
                .find(|c| c.start == e.at)
                .map(|c| c.id.clone());
            let at = match (e.side, out, inc) {
                (Side::Both, Some(o), Some(_)) => {
                    ops::roll(p, &o, ops::Edge::Tail, by)?;
                    e.at + by
                }
                (Side::Out | Side::Both, Some(o), _) => {
                    ops::trim(p, &o, ops::Edge::Tail, by, None)?;
                    e.at + by
                }
                (Side::In | Side::Both, _, Some(i)) => {
                    ops::trim(p, &i, ops::Edge::Head, by, None)?;
                    p.clip(&i).map_or(e.at, |c| c.start)
                }
                _ => return Err(vidcut_core::Error::not_found("no clip at the edit")),
            };
            Ok((at - 1, at))
        }
        DynamicTarget::Clips(ids) => {
            let id = ids
                .first()
                .ok_or_else(|| vidcut_core::Error::usage("no clip"))?;
            match mode {
                DynamicMode::Slip => ops::slip(p, id, by)?,
                DynamicMode::Slide => ops::slide(p, id, by)?,
            }
            let c = p
                .clip(id)
                .ok_or_else(|| vidcut_core::Error::not_found(format!("no clip {id}")))?;
            Ok(match mode {
                DynamicMode::Slip => (c.start, c.end() - 1),
                DynamicMode::Slide => (c.start - 1, c.end()),
            })
        }
    }
}

impl Workspace {
    pub(crate) fn open_silence(&mut self, clips: Vec<String>) {
        if !clips.is_empty() {
            self.timeline.silence = Some(SilenceDialog::new(clips));
        }
    }

    pub(super) fn silence_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.silence.take() else {
            return;
        };
        if let Some((params, ask)) = &d.asked
            && let Some(result) = ask.take()
        {
            match result {
                Ok(r) => {
                    d.parts = r.result["removed"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| {
                            Some((
                                p["clip"].as_str()?.to_string(),
                                p["start"].as_i64()?,
                                p["end"].as_i64()?,
                            ))
                        })
                        .collect();
                    d.frames = r.result["frames"].as_i64().unwrap_or(0);
                    d.error = None;
                }
                Err(e) => {
                    d.parts.clear();
                    d.error = Some(e.message);
                }
            }
            d.found_for = Some(params.clone());
            d.asked = None;
        }
        let params = d.params();
        if d.asked.is_none() && !d.current() {
            let mut dry = params.clone();
            dry["dry_run"] = json!(true);
            d.asked = Some((
                params.clone(),
                Ask::send(&self.live, ctx, "edit.ripple_delete_silence", dry),
            ));
        }
        let mut open = true;
        let (mut remove, mut cancel) = (false, false);
        let fps = self.fps().as_f64().max(1.0);
        let what = match d.clips.as_slice() {
            [one] => one.clone(),
            many => format!("{} clips", many.len()),
        };
        egui::Window::new("Ripple Delete Silence")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 360.0, 220.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(dim(format!(
                    "Cuts the silent parts out of {what} and closes the gaps."
                )));
                egui::Grid::new("silence-settings")
                    .num_columns(3)
                    .show(ui, |ui| {
                        ui.label("Threshold");
                        ui.add(
                            egui::DragValue::new(&mut d.threshold_db)
                                .range(-90.0..=0.0)
                                .speed(0.5)
                                .suffix(" dB"),
                        );
                        ui.label(dim("sound below this is silence"));
                        ui.end_row();
                        ui.label("Pre Head");
                        ui.add(
                            egui::DragValue::new(&mut d.pre_head)
                                .range(0.0..=5.0)
                                .speed(0.01)
                                .suffix(" s"),
                        );
                        ui.label(dim("kept before each audible part"));
                        ui.end_row();
                        ui.label("Post Tail");
                        ui.add(
                            egui::DragValue::new(&mut d.post_tail)
                                .range(0.0..=5.0)
                                .speed(0.01)
                                .suffix(" s"),
                        );
                        ui.label(dim("kept after each audible part"));
                        ui.end_row();
                        ui.label("Minimum to Trip");
                        ui.add(
                            egui::DragValue::new(&mut d.min)
                                .range(1.0 / fps..=60.0)
                                .speed(0.01)
                                .suffix(" s"),
                        );
                        ui.label(dim("shorter silences stay"));
                        ui.end_row();
                    });
                ui.separator();
                let status = match (&d.error, d.current(), d.asked.is_some()) {
                    (Some(e), _, _) => RichText::new(e).color(theme::colors().error),
                    (None, _, true) | (None, false, _) => dim("Finding silence…"),
                    (None, true, false) if d.parts.is_empty() => dim("No silence to remove."),
                    (None, true, false) => RichText::new(format!(
                        "Removes {} parts, {} ({} frames): the red stripes.",
                        d.parts.len(),
                        self.src_tc(d.frames),
                        d.frames
                    )),
                };
                ui.label(status);
                ui.label(dim("Cuts are de-clicked with 3 ms fades."));
                ui.horizontal(|ui| {
                    remove = ui
                        .add_enabled(
                            d.current() && !d.parts.is_empty(),
                            egui::Button::new("Remove"),
                        )
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if remove {
            self.send("edit.ripple_delete_silence", params);
        } else if open && !cancel {
            self.timeline.silence = Some(d);
        }
    }

    pub(super) fn paint_silence(&self, painter: &Painter, rows: &Rows, scale: &Scale, p: &Project) {
        let Some(d) = &self.timeline.silence else {
            return;
        };
        for (clip, a, b) in &d.parts {
            let Some(track) = p
                .tracks
                .iter()
                .find(|t| t.clips.iter().any(|c| &c.id == clip))
            else {
                continue;
            };
            let Some(row) = rows.get(&track.id) else {
                continue;
            };
            let r = row.rect.rect;
            let rect = Rect::from_x_y_ranges(scale.xf(*a)..=scale.xf(*b), r.y_range());
            painter.rect_filled(
                rect,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(225, 50, 45, 110),
            );
        }
    }

    pub(super) fn paint_boring(&self, painter: &Painter, rows: &Rows, scale: &Scale, p: &Project) {
        let o = &self.timeline.options;
        if !o.boring {
            return;
        }
        let boring = (o.boring_secs * p.settings.fps.as_f64()).round() as i64;
        let (long, short) = vidcut_core::analysis::boring(p, boring, o.jump_frames);
        for (clips, color) in [
            (long, Color32::from_rgba_unmultiplied(220, 220, 225, 90)),
            (short, Color32::from_rgba_unmultiplied(230, 50, 45, 150)),
        ] {
            for c in clips {
                let Some(t) = p
                    .tracks
                    .iter()
                    .find(|t| t.clips.iter().any(|x| x.id == c.id))
                else {
                    continue;
                };
                let Some(row) = rows.get(&t.id) else { continue };
                let rect = Rect::from_x_y_ranges(
                    scale.xf(c.start)..=scale.xf(c.end()).max(scale.xf(c.start) + 2.0),
                    row.rect.rect.y_range(),
                );
                painter.rect_filled(rect, CornerRadius::ZERO, color);
            }
        }
    }

    pub(super) fn boring_window(&mut self, ctx: &egui::Context) {
        let Some((mut secs, mut frames)) = self.timeline.boring_dialog else {
            return;
        };
        let mut open = true;
        let (mut analyze, mut cancel) = (false, false);
        egui::Window::new("Analyze Timeline Edits")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 320.0, 140.0))
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("boring-settings")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Boring Clips");
                        ui.add(
                            egui::Slider::new(&mut secs, super::BORING_SECS)
                                .logarithmic(true)
                                .suffix(" s"),
                        )
                        .on_hover_text("Longer clips show light gray");
                        ui.end_row();
                        ui.label("Jump Cuts");
                        ui.add(
                            egui::Slider::new(&mut frames, super::JUMP_FRAMES).suffix(" frames"),
                        )
                        .on_hover_text("Shorter clips show red; 2 finds flash frames");
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    analyze = ui.button("Analyze").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if analyze {
            let o = &mut self.timeline.options;
            o.boring = true;
            o.boring_secs = secs;
            o.jump_frames = frames;
            o.save();
            self.timeline.boring_dialog = None;
        } else if !open || cancel {
            self.timeline.boring_dialog = None;
        } else {
            self.timeline.boring_dialog = Some((secs, frames));
        }
    }

    pub(crate) fn dynamic_target(&self) -> Option<DynamicTarget> {
        match self.timeline.edit {
            Some(e) => Some(DynamicTarget::Edit(e)),
            None if !self.snap.selection.clips.is_empty() => {
                Some(DynamicTarget::Clips(self.snap.selection.clips.clone()))
            }
            None => None,
        }
    }

    pub(crate) fn toggle_dynamic(&mut self) {
        if self.timeline.dynamic.is_some() {
            self.dynamic_commit();
            self.timeline.dynamic = None;
            return;
        }
        if self.dynamic_target().is_none()
            && let Some(p) = self.project()
            && p.end() > 0
        {
            let at = ops::nearest_edit(p, self.snap.timeline.position);
            self.timeline.edit = Some(EditSel {
                at,
                side: Side::Both,
                track: TrackRef::MAIN,
            });
        }
        self.timeline.dynamic = Some(Dynamic::default());
    }

    pub fn dynamic_trim_request(&mut self, r: crate::live::DynamicTrim, now: f64) {
        use vidcut_core::request::DynamicTrimMode;
        if let Some(mode) = r.mode {
            self.timeline.options.dynamic_mode = match mode {
                DynamicTrimMode::Slip => DynamicMode::Slip,
                DynamicTrimMode::Slide => DynamicMode::Slide,
            };
            self.timeline.options.save();
        }
        if let Some(on) = r.on
            && on != self.timeline.dynamic.is_some()
        {
            self.toggle_dynamic();
        }
        if let Some(side) = r.side
            && self.timeline.edit.is_some()
        {
            let _ = self.set_edit_side(side.into());
        }
        if self.timeline.dynamic.is_none() {
            return;
        }
        if let Some(step) = r.step {
            if let Some(d) = &mut self.timeline.dynamic {
                d.pending += step;
            }
            self.dynamic_commit();
        }
        match r.run {
            Some(0.0) => self.dynamic_commit(),
            Some(speed) => {
                if let Some(d) = &mut self.timeline.dynamic {
                    d.run = Some((speed, now, d.pending));
                    d.stopped = None;
                }
            }
            None => {}
        }
    }

    pub(crate) fn dynamic_drawn(&self, p: &Project) -> Option<(Project, Option<i64>)> {
        let pending = self.timeline.dynamic.as_ref()?.pending;
        if pending == 0 {
            return None;
        }
        let target = self.dynamic_target()?;
        let mut trimmed = p.clone();
        let (_, right) = dynamic_preview(
            &mut trimmed,
            &target,
            self.timeline.options.dynamic_mode,
            pending,
        )
        .ok()?;
        let at = matches!(target, DynamicTarget::Edit(_)).then_some(right);
        Some((trimmed, at))
    }

    pub(crate) fn dynamic_commit(&mut self) {
        let trimmed = self
            .shown_project()
            .and_then(|p| self.dynamic_drawn(&p))
            .map(|(q, _)| q);
        let Some(d) = &mut self.timeline.dynamic else {
            return;
        };
        d.run = None;
        let by = std::mem::take(&mut d.pending);
        if by == 0 {
            return;
        }
        if let Some(DynamicTarget::Clips(_)) = self.dynamic_target()
            && let Some(q) = trimmed
        {
            self.trim_sent(q);
        }
        match self.dynamic_target() {
            Some(DynamicTarget::Edit(e)) => self.nudge_edit(e.side, by),
            Some(DynamicTarget::Clips(ids)) => {
                let method = match self.timeline.options.dynamic_mode {
                    DynamicMode::Slip => "edit.slip",
                    DynamicMode::Slide => "edit.slide",
                };
                match ids.as_slice() {
                    [one] => self.send(method, json!({ "clip": one, "by": by })),
                    many => self.send(
                        "apply",
                        json!({
                            "label": "Dynamic Trim",
                            "commands": many
                                .iter()
                                .map(|id| json!({ "method": method, "params": { "clip": id, "by": by } }))
                                .collect::<Vec<_>>(),
                        }),
                    ),
                }
            }
            None => {}
        }
    }

    pub(crate) fn dynamic_keys(&mut self, input: &egui::InputState) {
        let now = input.time;
        let pressed = |k: egui::Key| input.key_pressed(k);
        let k_down = input.key_down(egui::Key::K);
        for (key, dir) in [(egui::Key::L, 1.0), (egui::Key::J, -1.0)] {
            if !pressed(key) {
                continue;
            }
            if k_down {
                if let Some(d) = &mut self.timeline.dynamic {
                    d.pending += dir as i64;
                }
                self.dynamic_commit();
            } else if let Some(d) = &mut self.timeline.dynamic {
                let speed = match d.run {
                    Some((s, _, _)) if s * dir > 0.0 => (s * 2.0).clamp(-8.0, 8.0),
                    _ => dir,
                };
                d.run = Some((speed, now, d.pending));
                d.stopped = None;
            }
        }
        if pressed(egui::Key::K)
            && self
                .timeline
                .dynamic
                .as_ref()
                .is_some_and(|d| d.run.is_some() || d.pending != 0)
        {
            self.dynamic_commit();
        }
        if pressed(egui::Key::Space) {
            self.dynamic_commit();
            let center = match self.dynamic_target() {
                Some(DynamicTarget::Edit(e)) => Some(e.at),
                Some(DynamicTarget::Clips(ids)) => self
                    .project()
                    .and_then(|p| p.clip(&ids[0]))
                    .map(|c| c.start),
                None => None,
            };
            if let Some(at) = center {
                let two = 2 * self.fps().as_f64().round() as i64;
                self.seek(PlayerId::Timeline, (at - two).max(0), false);
                self.send("player.play", json!({ "speed": 1.0, "viewer": "timeline" }));
                if let Some(d) = &mut self.timeline.dynamic {
                    d.around_until = Some(at + two);
                }
            }
        }
    }

    pub(crate) fn dynamic_tick(&mut self, ctx: &egui::Context) {
        let fps = self.fps().as_f64();
        let now = ctx.input(|i| i.time);
        let (Some(target), Some(p)) = (self.dynamic_target(), self.shown_project()) else {
            return;
        };
        let mode = self.timeline.options.dynamic_mode;
        let playing = self.snap.timeline.playing;
        let position = self.snap.timeline.position;
        let Some(d) = &mut self.timeline.dynamic else {
            return;
        };
        if let Some((speed, t0, base)) = d.run {
            let want = base + (speed * fps * (now - t0)).round() as i64;
            let mut last = d.pending;
            let step = if want > last { 1 } else { -1 };
            while last != want {
                if let Err(e) = dynamic_preview(&mut Project::clone(&p), &target, mode, last + step)
                {
                    d.run = None;
                    d.stopped = Some(e.message);
                    break;
                }
                last += step;
            }
            d.pending = last;
            ctx.request_repaint();
        }
        if let Some(until) = d.around_until
            && (!playing || position >= until)
        {
            d.around_until = None;
            if playing {
                self.send("player.pause", json!({}));
            }
        }
    }
}
