use std::collections::HashMap;

use eframe::egui::{self, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, pos2};
use serde_json::{Value, json};
use vidcut_core::model::Project;
use vidcut_core::ops::{self, Edge};
use vidcut_core::request::Detect;

use super::{EditSel, Side, TrackRef};
use crate::gui::export::Ask;
use crate::gui::shuttle::{Jkl, first_press};
use crate::gui::timeline::geometry::{Rows, Scale};
use crate::gui::workspace::{Workspace, dim};
use crate::live::{Audition, MAX_SPEED, PlayerId};

#[derive(Debug, Clone, PartialEq)]
pub struct Stripe {
    pub clip: String,
    pub start: i64,
    pub end: i64,
    pub keep: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Shot {
    detect: Detect,
    threshold_db: f64,
    speech_threshold: f64,
    min_speech: f64,
    pre_head: f64,
    post_tail: f64,
    min: f64,
    stripes: Vec<Stripe>,
    found: Vec<Stripe>,
    found_for: Option<Value>,
    error: Option<String>,
}

#[derive(Debug)]
pub struct SilenceDialog {
    pub clips: Vec<String>,
    pub detect: Detect,
    pub threshold_db: f64,
    pub speech_threshold: f64,
    pub min_speech: f64,
    pub pre_head: f64,
    pub post_tail: f64,
    pub min: f64,
    pub stripes: Vec<Stripe>,
    found: Vec<Stripe>,
    pub error: Option<String>,
    asked: Option<(Value, Ask)>,
    found_for: Option<Value>,
    held: Option<(usize, Edge)>,
    skipped: Option<Vec<Stripe>>,
    undone: Vec<Shot>,
    redone: Vec<Shot>,
    download: bool,
    cross_fade: bool,
}

impl SilenceDialog {
    pub fn new(clips: Vec<String>) -> Self {
        Self {
            clips,
            detect: Detect::Level,
            threshold_db: -40.0,
            speech_threshold: 0.5,
            min_speech: 0.25,
            pre_head: 0.15,
            post_tail: 0.15,
            min: 0.5,
            stripes: Vec::new(),
            found: Vec::new(),
            error: None,
            asked: None,
            found_for: None,
            held: None,
            skipped: None,
            undone: Vec::new(),
            redone: Vec::new(),
            download: false,
            cross_fade: false,
        }
    }

    pub fn params(&self) -> Value {
        let mut p = json!({
            "clips": self.clips,
            "detect": self.detect,
            "pre_head": format!("{}s", self.pre_head),
            "post_tail": format!("{}s", self.post_tail),
            "min": format!("{}s", self.min),
        });
        match self.detect {
            Detect::Level => p["threshold_db"] = json!(self.threshold_db),
            Detect::Speech => {
                p["speech_threshold"] = json!(self.speech_threshold);
                p["min_speech"] = json!(format!("{}s", self.min_speech));
                if self.download {
                    p["download"] = json!(true);
                }
            }
        }
        p
    }

    pub fn current(&self) -> bool {
        self.found_for.as_ref() == Some(&self.params())
    }

    fn shot(&self) -> Shot {
        Shot {
            detect: self.detect,
            threshold_db: self.threshold_db,
            speech_threshold: self.speech_threshold,
            min_speech: self.min_speech,
            pre_head: self.pre_head,
            post_tail: self.post_tail,
            min: self.min,
            stripes: self.stripes.clone(),
            found: self.found.clone(),
            found_for: self.found_for.clone(),
            error: self.error.clone(),
        }
    }

    fn restore(&mut self, s: Shot) {
        self.detect = s.detect;
        self.threshold_db = s.threshold_db;
        self.speech_threshold = s.speech_threshold;
        self.min_speech = s.min_speech;
        self.pre_head = s.pre_head;
        self.post_tail = s.post_tail;
        self.min = s.min;
        self.stripes = s.stripes;
        self.found = s.found;
        self.found_for = s.found_for;
        self.error = s.error;
        self.asked = None;
        self.held = None;
    }

    fn keep(&mut self, before: Shot) {
        if self.undone.last() != Some(&before) {
            self.undone.push(before);
            self.redone.clear();
        }
    }

    fn mark(&mut self) {
        self.keep(self.shot());
    }

    pub fn can_undo(&self) -> bool {
        !self.undone.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redone.is_empty()
    }

    pub fn undo(&mut self) {
        let now = self.shot();
        while let Some(s) = self.undone.pop() {
            if s != now {
                self.redone.push(now);
                self.restore(s);
                return;
            }
        }
    }

    pub fn redo(&mut self) {
        let now = self.shot();
        while let Some(s) = self.redone.pop() {
            if s != now {
                self.undone.push(now);
                self.restore(s);
                return;
            }
        }
    }

    pub fn edited(&self) -> bool {
        self.stripes != self.found
    }

    fn cut(&self) -> impl Iterator<Item = &Stripe> {
        self.stripes.iter().filter(|s| !s.keep)
    }

    pub fn cut_parts(&self) -> Value {
        self.cut()
            .map(|s| json!({ "clip": s.clip, "start": s.start, "end": s.end }))
            .collect()
    }

    pub fn spans(&self) -> Vec<ops::ClipSpans> {
        let mut out: Vec<ops::ClipSpans> = Vec::new();
        for s in self.cut() {
            match out.iter_mut().find(|(c, _)| *c == s.clip) {
                Some((_, spans)) => spans.push((s.start, s.end)),
                None => out.push((s.clip.clone(), vec![(s.start, s.end)])),
            }
        }
        out
    }

    pub fn remove_params(&self) -> Value {
        let mut p = self.params();
        p["parts"] = self.cut_parts();
        p["cross_fade"] = json!(self.cross_fade);
        p
    }

    pub fn at(&self, frame: i64) -> Option<usize> {
        self.stripes
            .iter()
            .position(|s| s.start <= frame && frame < s.end)
    }

    pub fn next(&self, from: i64, dir: i64) -> Option<i64> {
        let starts = self.stripes.iter().map(|s| s.start);
        match dir {
            1 => starts.filter(|&s| s > from).min(),
            _ => starts.filter(|&s| s < from).max(),
        }
    }

    fn room(&self, i: usize, p: &Project) -> Option<(i64, i64)> {
        let s = self.stripes.get(i)?;
        let c = p.clip(&s.clip)?;
        let same = || {
            self.stripes
                .iter()
                .enumerate()
                .filter(move |(j, x)| *j != i && x.clip == s.clip)
                .map(|(_, x)| x)
        };
        let lo = same()
            .map(|x| x.end)
            .filter(|&e| e <= s.start)
            .fold(c.start, i64::max);
        let hi = same()
            .map(|x| x.start)
            .filter(|&a| a >= s.end)
            .fold(c.end(), i64::min);
        Some((lo, hi))
    }
}

pub fn clip_tracks(p: &Project) -> HashMap<&str, &str> {
    p.tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| (c.id.as_str(), t.id.as_str())))
        .collect()
}

fn hatch(painter: &Painter, rect: Rect, color: Color32) {
    let shown = rect.intersect(painter.clip_rect());
    if !shown.is_positive() {
        return;
    }
    let p = painter.with_clip_rect(shown);
    let (h, step) = (rect.height(), 6.0);
    let mut x = rect.left() + ((shown.left() - h - rect.left()) / step).floor() * step;
    while x < shown.right() {
        p.line_segment(
            [pos2(x, rect.bottom()), pos2(x + h, rect.top())],
            Stroke::new(1.0, color),
        );
        x += step;
    }
}

const STRIPE_EDGE: f32 = 5.0;

pub fn stripe_at(
    d: &SilenceDialog,
    p: &Project,
    rows: &Rows,
    scale: &Scale,
    pos: Pos2,
    edges: bool,
) -> Option<(usize, Option<Edge>)> {
    if !rows.area.contains(pos) {
        return None;
    }
    let row = rows.at(pos.y)?;
    let tracks = clip_tracks(p);
    let on_row = |s: &Stripe| tracks.get(s.clip.as_str()) == Some(&row.track.as_str());
    let spans = d
        .stripes
        .iter()
        .enumerate()
        .filter(|(_, s)| on_row(s))
        .map(|(i, s)| (i, scale.xf(s.start), scale.xf(s.end)));
    if edges {
        let edge = spans
            .clone()
            .flat_map(|(i, a, b)| {
                let grab = STRIPE_EDGE.min((b - a) / 3.0);
                [(i, Edge::Head, pos.x - a, a), (i, Edge::Tail, b - pos.x, b)]
                    .into_iter()
                    .filter(move |&(.., inside, _)| (-STRIPE_EDGE..=grab).contains(&inside))
            })
            .min_by(|x, y| (x.3 - pos.x).abs().total_cmp(&(y.3 - pos.x).abs()));
        if let Some((i, edge, ..)) = edge {
            return Some((i, Some(edge)));
        }
    }
    spans
        .filter(|&(_, a, b)| {
            let half = ((b - a) / 2.0).max(3.0);
            ((a + b) / 2.0 - pos.x).abs() <= half
        })
        .map(|(i, ..)| (i, None))
        .next()
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
    Edits(Vec<EditSel>),
    Clips(Vec<String>),
}

impl DynamicTarget {
    pub fn lead(&self) -> Option<EditSel> {
        match self {
            Self::Edits(all) => all.last().copied(),
            Self::Clips(_) => None,
        }
    }
}

const LANDING: std::time::Duration = std::time::Duration::from_secs(1);

pub fn landing<T: PartialEq>(at: T, since: std::time::Duration, from: T, position: T) -> Option<T> {
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
    let (id, out) = match (target, target.lead()) {
        (_, Some(e)) if e.side == Side::Audio => {
            let by = match dir {
                1 => pending,
                _ => furthest(pending, dir, |b| trimmed(b).is_some()),
            };
            return Some(Audition {
                timeline: Box::new(trimmed(by)?),
                at: e.at + pending,
                speed,
                skips: None,
            });
        }
        (_, Some(e)) => {
            let t = p.track(&e.track.id())?;
            let out = t.clips.iter().find(|c| c.end() == e.at);
            let inc = t.clips.iter().find(|c| c.start == e.at);
            match (e.side, out, inc) {
                (Side::Out, Some(o), _) | (_, Some(o), None) => (o.id.clone(), true),
                (_, _, Some(i)) => (i.id.clone(), false),
                _ => return None,
            }
        }
        (DynamicTarget::Edits(_), None) => return None,
        (DynamicTarget::Clips(ids), None) => {
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
    let heard = c.sound_at(edge as f64);
    let q = match (speed > 0.0) == out {
        true => trimmed(furthest(pending, dir, |b| trimmed(b).is_some()))?,
        false => now,
    };
    let at = frame_hearing(q.clip(&id)?, heard)?;
    Some(Audition {
        timeline: Box::new(q),
        at,
        speed,
        skips: None,
    })
}

fn frame_hearing(c: &vidcut_core::model::Clip, heard: f64) -> Option<i64> {
    let (mut lo, mut hi) = (c.start, c.end().checked_sub(1)?);
    let at = |x: i64| c.sound_at(x as f64);
    let rising = at(hi) >= at(lo);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if (at(mid) < heard) == rising {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    [lo - 1, lo]
        .into_iter()
        .filter(|&x| x >= c.start)
        .min_by(|&x, &y| (at(x) - heard).abs().total_cmp(&(at(y) - heard).abs()))
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
        skips: None,
    })
}

pub fn dynamic_preview(
    p: &mut Project,
    target: &DynamicTarget,
    mode: DynamicMode,
    by: i64,
) -> vidcut_core::Result<i64> {
    match target {
        DynamicTarget::Edits(all) => {
            let lead = *all
                .last()
                .ok_or_else(|| vidcut_core::Error::not_found("no edit"))?;
            let trims = super::edits::Plan::new(p, all, lead)?.each(by);
            for (_, c, b) in &trims {
                c.apply(p, *b)?;
            }
            Ok(trims
                .iter()
                .find(|(e, ..)| *e == lead)
                .and_then(|(_, c, _)| c.at(p))
                .unwrap_or(lead.at))
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
        if clips.is_empty() {
            return;
        }
        self.timeline.silence = Some(SilenceDialog::new(clips));
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
                    d.stripes = r.result["removed"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| {
                            Some(Stripe {
                                clip: p["clip"].as_str()?.to_string(),
                                start: p["start"].as_i64()?,
                                end: p["end"].as_i64()?,
                                keep: false,
                            })
                        })
                        .collect();
                    d.error = None;
                }
                Err(e) => {
                    d.stripes.clear();
                    d.error = Some(e.message);
                }
            }
            d.found = d.stripes.clone();
            d.held = None;
            d.found_for = Some(params.clone());
            d.asked = None;
        }
        let params = d.params();
        let needs = match d.detect == Detect::Speech && !d.download {
            true => vidcut_av::transcribe::needed(&crate::paths::models_dir(), &[]),
            false => Vec::new(),
        };
        if d.asked.is_none() && !d.current() && needs.is_empty() {
            let mut dry = params.clone();
            dry["dry_run"] = json!(true);
            d.asked = Some((
                params.clone(),
                Ask::send(&self.live, ctx, "edit.ripple_delete_silence", dry),
            ));
        }
        self.silence_skips(&mut d);
        let mut open = true;
        let (mut remove, mut cancel) = (false, false);
        let fps = self.fps().as_f64().max(1.0);
        let what = match d.clips.as_slice() {
            [one] => one.clone(),
            many => format!("{} clips", many.len()),
        };
        let speech = d.detect == Detect::Speech;
        let (cut, kept, finding) = match speech {
            false => ("the silent parts", "audible", "Finding silence…"),
            true => ("what isn't speech", "spoken", "Finding speech…"),
        };
        let edited = d.edited();
        let others = self.project().is_some_and(|p| {
            d.cut().any(|s| {
                let own = p.find_clip(&s.clip).map(|(ti, _)| ti);
                p.tracks.iter().enumerate().any(|(ti, t)| {
                    Some(ti) != own
                        && !t.locked
                        && t.clips.iter().any(|c| c.start < s.end && s.start < c.end())
                })
            })
        });
        let settled = d.current();
        let before = d.shot();
        use super::super::dialog;
        dialog::window(ctx, "Ripple Delete Silence", &mut open, |ui| {
            dialog::form(ui, "silence-form", |ui| {
                dialog::row(ui, "Cuts", |ui| ui.label(format!("{cut} of {what}")));
                dialog::row(ui, "Detect", |ui| {
                    dialog::switch(ui, |ui| {
                        ui.selectable_value(&mut d.detect, Detect::Level, "Sound Level");
                        ui.selectable_value(&mut d.detect, Detect::Speech, "Speech");
                    });
                });
                dialog::row(ui, "Threshold", |ui| {
                    if speech {
                        ui.add(
                            egui::DragValue::new(&mut d.speech_threshold)
                                .update_while_editing(false)
                                .range(0.0..=1.0)
                                .speed(0.01)
                                .fixed_decimals(2),
                        );
                        ui.label(dim("speech from this probability"));
                    } else {
                        ui.add(
                            egui::DragValue::new(&mut d.threshold_db)
                                .update_while_editing(false)
                                .range(-90.0..=0.0)
                                .speed(0.5)
                                .suffix(" dB"),
                        );
                        ui.label(dim("quieter is silence"));
                    }
                });
                if speech {
                    dialog::row(ui, "Shortest Speech", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut d.min_speech)
                                .update_while_editing(false)
                                .range(0.0..=5.0)
                                .speed(0.01)
                                .suffix(" s"),
                        );
                    });
                }
                dialog::row(ui, "Pre Head", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut d.pre_head)
                            .update_while_editing(false)
                            .range(0.0..=5.0)
                            .speed(0.01)
                            .suffix(" s"),
                    );
                    ui.label(dim(format!("kept before each {kept} part")));
                });
                dialog::row(ui, "Post Tail", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut d.post_tail)
                            .update_while_editing(false)
                            .range(0.0..=5.0)
                            .speed(0.01)
                            .suffix(" s"),
                    );
                    ui.label(dim(format!("kept after each {kept} part")));
                });
                dialog::row(ui, "Shortest Cut", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut d.min)
                            .update_while_editing(false)
                            .range(1.0 / fps..=60.0)
                            .speed(0.01)
                            .suffix(" s"),
                    );
                });
                dialog::row(ui, "Cross Fade Audio", |ui| {
                    ui.checkbox(&mut d.cross_fade, "")
                });
            });
            if edited {
                dialog::warn(
                    ui,
                    "A changed setting finds stripes again; Ctrl+Z brings edits back",
                );
            }
            if others {
                dialog::warn(ui, "Other unlocked tracks lose these parts too");
            }
            let all = d.stripes.len();
            let n = d.cut().count();
            let frames: i64 = d.cut().map(|s| s.end - s.start).sum();
            if !needs.is_empty() {
                dialog::note(
                    ui,
                    format!(
                        "Speech needs {}, downloaded once.",
                        vidcut_av::models::listed(&needs)
                    ),
                );
                if ui.button("Download and Find").clicked() {
                    d.download = true;
                }
            } else {
                match (&d.error, d.current(), d.asked.is_some()) {
                    (Some(e), _, _) => {
                        dialog::warn(ui, e.clone());
                    }
                    (None, _, true) | (None, false, _) => {
                        dialog::note(ui, finding);
                    }
                    (None, true, false) if all == 0 => {
                        dialog::note(ui, "Nothing to remove.");
                    }
                    (None, true, false) => {
                        let of = match n == all {
                            true => String::new(),
                            false => format!(" of {all}"),
                        };
                        ui.label(format!(
                            "Removes {n}{of} parts, {}: the red stripes.",
                            self.src_tc(frames),
                        ));
                    }
                }
            }
            let ready = d.current() && all > 0;
            dialog::form(ui, "silence-stripes", |ui| {
                dialog::row(ui, "Stripes", |ui| {
                    if ui
                        .add_enabled(ready, egui::Button::new("Keep All"))
                        .clicked()
                    {
                        d.mark();
                        d.stripes.iter_mut().for_each(|s| s.keep = true);
                    }
                    if ui
                        .add_enabled(ready, egui::Button::new("Cut All"))
                        .clicked()
                    {
                        d.mark();
                        d.stripes.iter_mut().for_each(|s| s.keep = false);
                    }
                    ui.label(dim("click one to keep it; X at the playhead"));
                });
            });
            dialog::footer(ui, |ui| {
                remove = dialog::main_button(ui, "Remove", ready && n > 0);
                cancel = ui.button("Cancel").clicked();
                if ready && n == 0 {
                    dialog::why(ui, "every part is kept");
                }
            });
        });
        if settled && d.params() != params {
            d.keep(before);
        }
        if remove || cancel || !open {
            self.live.skip(None);
        }
        if remove {
            self.send("edit.ripple_delete_silence", d.remove_params());
        } else if open && !cancel {
            self.timeline.silence = Some(d);
        }
    }

    fn silence_skips(&mut self, d: &mut SilenceDialog) {
        if d.held.is_some() || d.skipped.as_ref() == Some(&d.stripes) {
            return;
        }
        d.skipped = Some(d.stripes.clone());
        let spans = d.spans();
        self.live.skip((!spans.is_empty()).then_some(spans));
    }

    pub(crate) fn silence_keys(&mut self, input: &egui::InputState) {
        let at = self.drawn_playhead();
        let Some(d) = &mut self.timeline.silence else {
            return;
        };
        let m = input.modifiers;
        if input.key_pressed(egui::Key::X)
            && !m.command
            && !m.alt
            && let Some(i) = d.at(at)
        {
            d.mark();
            d.stripes[i].keep ^= true;
        }
        let struck = |k: egui::Key| {
            input.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key, physical_key, pressed: true, .. }
                    if *key == k || *physical_key == Some(k))
            })
        };
        let dir = match (
            struck(egui::Key::OpenBracket),
            struck(egui::Key::CloseBracket),
        ) {
            (true, false) => -1,
            (false, true) => 1,
            _ => return,
        };
        if let Some(to) = d.next(at, dir) {
            self.show_timeline();
            self.seek(PlayerId::Timeline, to, false);
            let from = self.snap.timeline.position;
            self.timeline.landing = Some((to, std::time::Instant::now(), from));
        }
    }

    pub(super) fn stripe_hover(
        &self,
        ui: &egui::Ui,
        p: &Project,
        rows: &Rows,
        scale: &Scale,
    ) -> bool {
        let Some(d) = &self.timeline.silence else {
            return false;
        };
        let hit = ui
            .input(|i| i.pointer.hover_pos())
            .and_then(|pos| stripe_at(d, p, rows, scale, pos, true));
        match (d.held.is_some(), hit) {
            (true, _) | (_, Some((_, Some(_)))) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                true
            }
            (false, Some((_, None))) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                true
            }
            (false, None) => false,
        }
    }

    pub(super) fn stripe_press(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        p: &Project,
        rows: &Rows,
        scale: &Scale,
    ) -> bool {
        let Some(d) = &mut self.timeline.silence else {
            return false;
        };
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some((i, edge)) = stripe_at(d, p, rows, scale, press, true)
        {
            if edge.is_some() {
                d.mark();
            }
            d.held = edge.map(|e| (i, e));
            return true;
        }
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
            && let Some((i, _)) = stripe_at(d, p, rows, scale, pos, false)
        {
            d.mark();
            d.stripes[i].keep ^= true;
            return true;
        }
        false
    }

    pub(super) fn stripe_drag(&mut self, ui: &egui::Ui, p: &Project, scale: &Scale) {
        let Some(d) = &mut self.timeline.silence else {
            return;
        };
        let Some((i, edge)) = d.held else {
            return;
        };
        let (pos, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
        let (Some(pos), true, Some((lo, hi))) = (pos, down, d.room(i, p)) else {
            d.held = None;
            return;
        };
        let at = scale.boundary(pos.x);
        let s = &mut d.stripes[i];
        let (from, to, edge) = match edge {
            Edge::Head => (lo, s.end - 1, &mut s.start),
            Edge::Tail => (s.start + 1, hi, &mut s.end),
        };
        if from > to {
            d.held = None;
            return;
        }
        *edge = at.clamp(from, to);
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    pub(super) fn paint_silence(&self, painter: &Painter, rows: &Rows, scale: &Scale, p: &Project) {
        let Some(d) = &self.timeline.silence else {
            return;
        };
        let tracks = clip_tracks(p);
        let cut = Color32::from_rgba_unmultiplied(225, 50, 45, 110);
        let kept = Color32::from_rgba_unmultiplied(150, 150, 155, 70);
        let lines = Color32::from_rgba_unmultiplied(205, 205, 210, 120);
        for (i, s) in d.stripes.iter().enumerate() {
            let Some(row) = tracks.get(s.clip.as_str()).and_then(|t| rows.get(t)) else {
                continue;
            };
            let rect =
                Rect::from_x_y_ranges(scale.xf(s.start)..=scale.xf(s.end), row.rect.rect.y_range());
            if !painter.clip_rect().intersects(rect) {
                continue;
            }
            if s.keep {
                painter.rect_filled(rect, CornerRadius::ZERO, kept);
                hatch(painter, rect, lines);
            } else {
                painter.rect_filled(rect, CornerRadius::ZERO, cut);
            }
            if let Some((h, edge)) = d.held
                && h == i
            {
                let x = match edge {
                    Edge::Head => rect.left(),
                    Edge::Tail => rect.right(),
                };
                painter.vline(x, rect.y_range(), Stroke::new(2.0, Color32::WHITE));
            }
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
        use super::super::dialog;
        dialog::window(ctx, "Analyze Timeline Edits", &mut open, |ui| {
            dialog::form(ui, "boring-form", |ui| {
                dialog::row(ui, "Boring Clips", |ui| {
                    ui.add(
                        egui::Slider::new(&mut secs, super::BORING_SECS)
                            .logarithmic(true)
                            .suffix(" s"),
                    );
                    ui.label(dim("longer shows gray"));
                });
                dialog::row(ui, "Jump Cuts", |ui| {
                    ui.add(egui::Slider::new(&mut frames, super::JUMP_FRAMES).suffix(" frames"));
                    ui.label(dim("shorter shows red"));
                });
            });
            dialog::footer(ui, |ui| {
                analyze = dialog::main_button(ui, "Analyze", true);
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
        let edits = self.timeline.selected_edits();
        if !edits.is_empty() {
            return Some(DynamicTarget::Edits(edits));
        }
        let clips = &self.snap.selection.clips;
        (!clips.is_empty()).then(|| DynamicTarget::Clips(clips.clone()))
    }

    pub(crate) fn toggle_dynamic(&mut self) {
        if self.timeline.dynamic.is_some() {
            self.dynamic_stop();
            self.timeline.dynamic = None;
            self.timeline.set_edit(None);
            return;
        }
        if self.dynamic_target().is_none()
            && let Some(p) = self.project()
            && p.end() > 0
        {
            let at = ops::nearest_edit(p, self.snap.timeline.position);
            self.timeline.set_edit(Some(EditSel {
                at,
                side: Side::Both,
                track: TrackRef::MAIN,
            }));
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
            && self.timeline.lead_edit().is_some()
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

    pub(crate) fn trim_drawn(&self, p: &Project) -> Option<(Project, i64)> {
        self.dynamic_drawn(p).or_else(|| self.strip_drawn(p))
    }

    pub(crate) fn strip_drawn(&self, p: &Project) -> Option<(Project, i64)> {
        let (e, by) = self.timeline.strip_drag?;
        let selected = self.timeline.edit?;
        if (selected.at, selected.track) != (e.at, e.track) {
            return None;
        }
        let mut trimmed = p.clone();
        let mode = self.timeline.options.dynamic_mode;
        let at = dynamic_preview(&mut trimmed, &DynamicTarget::Edits(vec![e]), mode, by).ok()?;
        Some((trimmed, at))
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
            DynamicTarget::Edits(all) => all.last().map(|e| e.at),
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
            Some(DynamicTarget::Edits(_)) => self.nudge_edits(by),
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
