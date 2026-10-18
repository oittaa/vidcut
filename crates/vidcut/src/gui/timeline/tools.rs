use eframe::egui::{self, Color32, CornerRadius, Painter, Rect, RichText};
use serde_json::{Value, json};
use vidcut_core::model::Project;
use vidcut_core::ops;

use super::{EditSel, Side, TrackRef, audio_edit_outgoing};
use crate::gui::export::{Ask, near_center};
use crate::gui::shuttle::{Jkl, first_press};
use crate::gui::theme;
use crate::gui::timeline::geometry::{Rows, Scale};
use crate::gui::workspace::{Workspace, dim};
use crate::live::{Audition, MAX_SPEED, PlayerId};

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
    around: Option<Around>,
    stopped: Option<(DynamicTarget, String)>,
    end_edge: bool,
}

#[derive(Debug, Clone, Copy)]
struct Around {
    until: i64,
    back: i64,
    playing: bool,
}

impl Dynamic {
    pub fn speed(&self) -> f64 {
        self.run.map_or(0.0, |(speed, ..)| speed)
    }

    pub fn running(&self) -> bool {
        self.run.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DynamicTarget {
    Edit(EditSel),
    Clips(Vec<String>),
}

const LANDING: std::time::Duration = std::time::Duration::from_secs(1);

pub fn landing(at: i64, since: std::time::Duration, from: i64, position: i64) -> Option<i64> {
    (position == from && since < LANDING).then_some(at)
}

pub fn faster(speed: f64, dir: f64) -> f64 {
    if speed * dir > 0.0 {
        (speed * 2.0).clamp(-MAX_SPEED, MAX_SPEED)
    } else {
        dir
    }
}

pub fn first_clip<'a>(p: &'a Project, ids: &[String]) -> Option<&'a vidcut_core::model::Clip> {
    ids.iter()
        .filter_map(|id| p.clip(id))
        .min_by_key(|c| c.start)
}

pub fn nearer_end(at: i64, start: i64, end: i64) -> bool {
    (end - at).abs() <= (at - start).abs()
}

pub fn furthest(from: i64, dir: i64, fits: impl Fn(i64) -> bool) -> i64 {
    let (mut ok, mut step) = (from, 1i64);
    while step < 1 << 40 && fits(from + dir * step) {
        ok = from + dir * step;
        step *= 2;
    }
    let mut bad = from + dir * step;
    while (bad - ok).abs() > 1 {
        let mid = ok + (bad - ok) / 2;
        if fits(mid) {
            ok = mid;
        } else {
            bad = mid;
        }
    }
    ok
}

pub fn heard(
    p: &Project,
    target: &DynamicTarget,
    mode: DynamicMode,
    end_edge: bool,
    pending: i64,
    speed: f64,
) -> Option<Audition> {
    let trimmed = |by: i64| {
        let mut q = p.clone();
        dynamic_preview(&mut q, target, mode, by).ok().map(|_| q)
    };
    let dir = if speed > 0.0 { 1 } else { -1 };
    let now = trimmed(pending)?;
    let (id, out) = match target {
        DynamicTarget::Edit(e) if e.side == Side::Audio => {
            let by = match dir {
                1 => pending,
                _ => furthest(pending, dir, |b| trimmed(b).is_some()),
            };
            return Some(Audition {
                timeline: Box::new(trimmed(by)?),
                at: e.at + pending,
                speed,
            });
        }
        DynamicTarget::Edit(e) => {
            let t = p.track(&e.track.id())?;
            let out = t.clips.iter().find(|c| c.end() == e.at);
            let inc = t.clips.iter().find(|c| c.start == e.at);
            match (e.side, out, inc) {
                (Side::Out, Some(o), _) | (_, Some(o), None) => (o.id.clone(), true),
                (_, _, Some(i)) => (i.id.clone(), false),
                _ => return None,
            }
        }
        DynamicTarget::Clips(ids) => {
            let c = first_clip(&now, ids)?;
            if mode == DynamicMode::Slip {
                return slip_heard(&now, c, end_edge, speed);
            }
            let t = now
                .tracks
                .iter()
                .find(|t| t.clips.iter().any(|x| x.id == c.id))?;
            let beside = match end_edge {
                true => t.clips.iter().find(|x| x.start == c.end()),
                false => t.clips.iter().find(|x| x.end() == c.start),
            };
            (beside?.id.clone(), !end_edge)
        }
    };
    let c = now.clip(&id)?;
    let edge = match out {
        true => c.end() - 1,
        false => c.start,
    };
    let source = c.source_at(edge);
    let into_run = first_showing(c, source).map(|first| edge - first);
    let q = match (speed > 0.0) == out {
        true => trimmed(furthest(pending, dir, |b| trimmed(b).is_some()))?,
        false => now,
    };
    let qc = q.clip(&id)?;
    let at = match (first_showing(qc, source), into_run) {
        (Some(first), Some(k)) => (first + k).min(qc.end() - 1),
        _ => frame_showing(qc, source)?,
    };
    Some(Audition {
        timeline: Box::new(q),
        at,
        speed,
    })
}

fn first_showing(c: &vidcut_core::model::Clip, source: i64) -> Option<i64> {
    let (mut lo, mut hi) = (c.start, c.end().checked_sub(1)?);
    let rising = c.source_at(hi) >= c.source_at(lo);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let before = match rising {
            true => c.source_at(mid) < source,
            false => c.source_at(mid) > source,
        };
        if before {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    (c.source_at(lo) == source).then_some(lo)
}

fn frame_showing(c: &vidcut_core::model::Clip, source: i64) -> Option<i64> {
    let (mut lo, mut hi) = (c.start, c.end().checked_sub(1)?);
    let rising = c.source_at(hi) >= c.source_at(lo);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if (c.source_at(mid) < source) == rising {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    [lo - 1, lo]
        .into_iter()
        .filter(|&x| x >= c.start)
        .min_by_key(|&x| (c.source_at(x) - source).abs())
}

pub fn slip_room(media_frames: i64, speed: f64, duration: i64) -> i64 {
    let slowed = match speed.abs() {
        s if s > 0.0 => (media_frames as f64 / s).ceil().min(1e15) as i64,
        _ => 0,
    };
    slowed + duration
}

fn slip_heard(
    p: &Project,
    c: &vidcut_core::model::Clip,
    end_edge: bool,
    speed: f64,
) -> Option<Audition> {
    let edge_frame = match end_edge {
        true => c.end() - 1,
        false => c.start,
    };
    let mut q = p.clone();
    for t in &mut q.tracks {
        t.clips.retain(|x| x.id == c.id);
        t.transitions.clear();
    }
    let media = q.media(&c.media).map_or(0, |m| q.media_frames(m));
    let room = slip_room(media, c.speed(), c.duration);
    let moved = q.tracks.iter_mut().flat_map(|t| t.clips.iter_mut());
    moved.filter(|x| x.id == c.id).for_each(|x| x.start += room);
    let (edge, dir) = match speed > 0.0 {
        true => (ops::Edge::Head, -1),
        false => (ops::Edge::Tail, 1),
    };
    let grows = |k: i64| ops::trim(&mut q.clone(), &c.id, edge, dir * k, Some(false)).is_ok();
    let k = furthest(0, 1, grows);
    ops::trim(&mut q, &c.id, edge, dir * k, Some(false)).ok()?;
    Some(Audition {
        timeline: Box::new(q),
        at: edge_frame + room,
        speed: -speed,
    })
}

pub fn dynamic_preview(
    p: &mut Project,
    target: &DynamicTarget,
    mode: DynamicMode,
    by: i64,
) -> vidcut_core::Result<i64> {
    match target {
        DynamicTarget::Edit(e) => {
            if e.side == Side::Audio {
                let id = p
                    .track(&e.track.id())
                    .and_then(|t| audio_edit_outgoing(t, e.at))
                    .ok_or_else(|| vidcut_core::Error::not_found("no audio edit"))?;
                ops::audio_roll(p, &id, ops::Edge::Tail, by)?;
                return Ok(e.at + by);
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
            Ok(at)
        }
        DynamicTarget::Clips(ids) => {
            for id in ids {
                match mode {
                    DynamicMode::Slip => ops::slip(p, id, -by)?,
                    DynamicMode::Slide => ops::slide(p, id, by)?,
                }
            }
            first_clip(p, ids)
                .map(|c| c.start)
                .ok_or_else(|| vidcut_core::Error::usage("no clip"))
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
            self.dynamic_stop();
            self.timeline.dynamic = None;
            self.timeline.edit = None;
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
        self.show_timeline();
        self.dynamic_edge();
        self.dynamic_to_head();
    }

    fn dynamic_edge(&mut self) {
        let position = self.snap.timeline.position;
        let end = match (self.dynamic_target(), self.shown_project()) {
            (Some(DynamicTarget::Clips(ids)), Some(p)) => {
                first_clip(&p, &ids).is_some_and(|c| nearer_end(position, c.start, c.end()))
            }
            _ => false,
        };
        if let Some(d) = &mut self.timeline.dynamic {
            d.end_edge = end;
        }
    }

    fn dynamic_end_edge(&self) -> bool {
        self.timeline.dynamic.as_ref().is_some_and(|d| d.end_edge)
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
            self.dynamic_stop();
        }
        match r.run {
            Some(0.0) => self.dynamic_stop(),
            Some(speed) => self.dynamic_start(speed, now),
            None => {}
        }
    }

    pub(crate) fn dynamic_drawn(&self, p: &Project) -> Option<(Project, i64)> {
        let pending = self.timeline.dynamic.as_ref()?.pending;
        if pending == 0 {
            return None;
        }
        let target = self.dynamic_target()?;
        let mut trimmed = p.clone();
        let at = dynamic_preview(
            &mut trimmed,
            &target,
            self.timeline.options.dynamic_mode,
            pending,
        )
        .ok()?;
        let at = match &target {
            DynamicTarget::Clips(ids) if self.dynamic_end_edge() => {
                first_clip(&trimmed, ids).map_or(at, |c| c.end())
            }
            _ => at,
        };
        Some((trimmed, at))
    }

    pub(crate) fn dynamic_stopped(&self) -> Option<&str> {
        let d = self.timeline.dynamic.as_ref()?;
        let (target, why) = d.stopped.as_ref()?;
        (!d.running() && self.dynamic_target().as_ref() == Some(target)).then_some(why.as_str())
    }

    fn dynamic_head(&mut self) -> Option<i64> {
        match self.dynamic_target()? {
            DynamicTarget::Edit(e) => Some(e.at),
            DynamicTarget::Clips(ids) => {
                let end = self.dynamic_end_edge();
                let p = self.shown_project()?;
                first_clip(&p, &ids).map(|c| if end { c.end() } else { c.start })
            }
        }
    }

    fn dynamic_to_head(&mut self) {
        if let Some(at) = self.dynamic_head()
            && at != self.snap.timeline.position
        {
            self.seek(PlayerId::Timeline, at, false);
            let from = self.snap.timeline.position;
            self.timeline.landing = Some((at, std::time::Instant::now(), from));
        }
    }

    fn dynamic_start(&mut self, speed: f64, now: f64) {
        if self.dynamic_target().is_none() {
            return;
        }
        let playing = self.snap.timeline.playing;
        let fresh = !self.timeline.dynamic.as_ref().is_some_and(Dynamic::running);
        if fresh {
            self.dynamic_edge();
        }
        let Some(d) = &mut self.timeline.dynamic else {
            return;
        };
        d.around = None;
        d.stopped = None;
        d.run = Some((speed, now, d.pending));
        if playing && fresh {
            self.send("player.pause", json!({}));
        }
        self.show_timeline();
        self.dynamic_to_head();
        self.dynamic_sound(speed);
    }

    fn dynamic_sound(&mut self, speed: f64) {
        let (Some(target), Some(p), Some(d)) = (
            self.dynamic_target(),
            self.shown_project(),
            self.timeline.dynamic.as_ref(),
        ) else {
            return;
        };
        let mode = self.timeline.options.dynamic_mode;
        let a = heard(&p, &target, mode, d.end_edge, d.pending, speed);
        self.timeline.heard = a.is_some();
        self.live.audition(a);
    }

    fn dynamic_hush(&mut self) {
        if std::mem::take(&mut self.timeline.heard) {
            self.live.audition(None);
        }
    }

    pub(crate) fn dynamic_stop(&mut self) {
        self.dynamic_commit();
        self.dynamic_to_head();
    }

    pub(crate) fn dynamic_commit(&mut self) {
        let playing = self.snap.timeline.playing;
        if let Some(d) = &mut self.timeline.dynamic
            && d.around.take().is_some()
            && playing
        {
            self.send("player.pause", json!({}));
        }
        self.dynamic_hush();
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
        d.stopped = None;
        if let Some(DynamicTarget::Clips(_)) = self.dynamic_target()
            && let Some(q) = trimmed
        {
            self.trim_sent(q);
        }
        match self.dynamic_target() {
            Some(DynamicTarget::Edit(e)) => self.nudge_edit(e.side, by),
            Some(DynamicTarget::Clips(ids)) => {
                let (method, by) = match self.timeline.options.dynamic_mode {
                    DynamicMode::Slip => ("edit.slip", -by),
                    DynamicMode::Slide => ("edit.slide", by),
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

    pub(crate) fn dynamic_jkl(&mut self, jkl: Jkl, now: f64) {
        let running = self.timeline.dynamic.as_ref().is_some_and(Dynamic::running);
        match jkl {
            Jkl::Play(dir) => {
                let speed = self.timeline.dynamic.as_ref().map_or(0.0, Dynamic::speed);
                self.dynamic_start(faster(speed, dir), now);
            }
            Jkl::Step(dir) => {
                if let Some(d) = &mut self.timeline.dynamic {
                    d.pending += dir as i64;
                }
                self.dynamic_stop();
            }
            Jkl::Slow(speed) => self.dynamic_start(speed, now),
            Jkl::Stop if running => self.dynamic_stop(),
            Jkl::Stop => {
                if self.snap.timeline.playing {
                    self.send("player.pause", json!({}));
                }
            }
        }
    }

    pub(crate) fn dynamic_keys(&mut self, input: &egui::InputState) {
        let pressed = |k: egui::Key| input.key_pressed(k);
        let running = self.timeline.dynamic.as_ref().is_some_and(Dynamic::running);
        let playing = self.snap.timeline.playing;
        if self.dynamic_target().is_some() && first_press(input, egui::Key::K) {
            if running {
                self.dynamic_stop();
            } else if playing {
                self.send("player.pause", json!({}));
            }
        }
        if pressed(egui::Key::Space) {
            if running {
                self.dynamic_stop();
            } else if playing {
                self.send("player.pause", json!({}));
            } else {
                self.play_around();
            }
        }
    }

    fn play_around(&mut self) {
        let at = self.drawn_playhead();
        let two = 2 * self.fps().as_f64().round() as i64;
        self.seek(PlayerId::Timeline, (at - two).max(0), false);
        self.send("player.play", json!({ "speed": 1.0, "viewer": "timeline" }));
        if let Some(d) = &mut self.timeline.dynamic {
            d.around = Some(Around {
                until: at + two,
                back: at,
                playing: false,
            });
        }
    }

    pub(crate) fn dynamic_tick(&mut self, ctx: &egui::Context) {
        if !self.timeline.dynamic.as_ref().is_some_and(Dynamic::running) {
            self.dynamic_hush();
        }
        let fps = self.fps().as_f64();
        let now = ctx.input(|i| i.time);
        let playing = self.snap.timeline.playing;
        let position = self.snap.timeline.position;
        let mut ended = None;
        if let (Some(target), Some(p)) = (self.dynamic_target(), self.shown_project())
            && let Some(d) = &mut self.timeline.dynamic
            && let Some((speed, t0, base)) = d.run
        {
            let mode = self.timeline.options.dynamic_mode;
            let tried = |by: i64| dynamic_preview(&mut Project::clone(&p), &target, mode, by);
            let want = base + (speed * fps * (now - t0)).round() as i64;
            if want != d.pending
                && let Err(e) = tried(want)
            {
                let step = if want > d.pending { 1 } else { -1 };
                let mut why = e.message;
                while d.pending != want {
                    match tried(d.pending + step) {
                        Ok(_) => d.pending += step,
                        Err(e) => {
                            why = e.message;
                            break;
                        }
                    }
                }
                ended = Some(why);
            } else {
                d.pending = want;
                if let Some(wait) = next_step_in(speed, fps, now - t0) {
                    super::repaint_in(ctx, std::time::Duration::from_secs_f64(wait));
                }
            }
        }
        if let Some(why) = ended {
            self.dynamic_stop();
            let target = self.dynamic_target();
            if let (Some(d), Some(target)) = (&mut self.timeline.dynamic, target) {
                d.stopped = Some((target, why));
            }
            ctx.request_repaint();
        }
        let Some(d) = &mut self.timeline.dynamic else {
            return;
        };
        let Some(a) = &mut d.around else {
            return;
        };
        a.playing |= playing;
        if a.playing && (!playing || position >= a.until) {
            let back = a.back;
            d.around = None;
            if playing {
                self.send("player.pause", json!({}));
            }
            self.seek(PlayerId::Timeline, back, false);
        }
    }
}

fn next_step_in(speed: f64, fps: f64, elapsed: f64) -> Option<f64> {
    let rate = speed.abs() * fps;
    if rate <= 0.0 {
        return None;
    }
    let done = rate * elapsed.max(0.0);
    Some(((done + 0.5).floor() + 0.5 - done) / rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_run_at_25_fps_steps_half_a_frame_after_it_starts() {
        assert_eq!(next_step_in(1.0, 25.0, 0.0), Some(0.02));
    }

    #[test]
    fn a_run_backward_at_twice_the_speed_steps_a_frame_after_its_last_step() {
        assert_eq!(next_step_in(-2.0, 25.0, 0.01), Some(0.02));
    }

    #[test]
    fn a_run_at_no_speed_never_steps() {
        assert_eq!(next_step_in(0.0, 25.0, 1.0), None);
    }

    #[test]
    fn the_playhead_is_drawn_at_the_cut_until_its_seek_lands() {
        assert_eq!(landing(90, Duration::from_millis(100), 60, 60), Some(90));
    }

    #[test]
    fn the_playhead_is_drawn_where_it_is_once_the_seek_lands() {
        assert_eq!(landing(90, Duration::from_millis(100), 60, 90), None);
    }

    #[test]
    fn the_playhead_is_drawn_where_it_is_once_another_seek_moves_it() {
        assert_eq!(landing(90, Duration::from_millis(100), 60, 0), None);
    }

    #[test]
    fn a_seek_that_never_lands_stops_holding_the_playhead_after_a_second() {
        assert_eq!(landing(90, Duration::from_secs(2), 60, 60), None);
    }

    #[test]
    fn the_end_of_a_clip_is_nearer_a_playhead_halfway_along() {
        assert!(nearer_end(90, 60, 120));
    }

    #[test]
    fn the_start_of_a_clip_is_nearer_a_playhead_in_its_first_half() {
        assert!(!nearer_end(240, 192, 316));
    }

    #[test]
    fn the_furthest_trim_forward_is_the_last_that_fits() {
        assert_eq!(furthest(0, 1, |by| by <= 37), 37);
    }

    #[test]
    fn the_furthest_trim_backward_is_the_last_that_fits() {
        assert_eq!(furthest(5, -1, |by| by >= -3), -3);
    }

    #[test]
    fn a_trim_with_no_room_goes_nowhere() {
        assert_eq!(furthest(4, 1, |by| by <= 4), 4);
    }

    #[test]
    fn a_one_percent_clip_needs_a_hundred_frames_of_room_for_each_frame_of_media() {
        assert_eq!(slip_room(300, 0.01, 60), 30_060);
    }

    #[test]
    fn a_freeze_needs_room_for_its_own_length_only() {
        assert_eq!(slip_room(300, 0.0, 60), 60);
    }

    #[test]
    fn l_again_at_32_times_runs_at_64() {
        assert_eq!(faster(32.0, 1.0), 64.0);
    }

    #[test]
    fn j_again_at_64_times_backward_stays_at_64() {
        assert_eq!(faster(-64.0, -1.0), -64.0);
    }

    #[test]
    fn j_during_a_run_forward_runs_backward_at_normal_speed() {
        assert_eq!(faster(8.0, -1.0), -1.0);
    }
}
