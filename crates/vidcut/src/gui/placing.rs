use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use serde_json::{Value, json};
use vidcut_core::model::{Clip, Picture, PictureTool, Project, TrackKind};
use vidcut_core::ops::picture::has_picture;
use vidcut_core::place::{Level, Placing};
use vidcut_core::render::Placed;

use super::timeline::tools::landing;
use super::workspace::Workspace;

const GRAB: f32 = 6.0;
const SNAP: f32 = 8.0;
const LIFT: f32 = 26.0;
const LINES: [f64; 3] = [1.0 / 3.0, 0.5, 2.0 / 3.0];
const ZOOM: (f64, f64) = (0.01, 100.0);
const LEFT_SHOWING: f32 = 4.0;
const TURN_BAND: f32 = 12.0;
const STEP: f64 = 15.0;
const NOTCH_ZOOM: f64 = 1.1;
const STILL: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

    fn across(self) -> bool {
        matches!(self, Side::Left | Side::Right)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    Move,
    Corner,
    Stretch(Side),
    Turn,
    Crop(Side),
}

#[derive(Debug, Clone)]
struct Held {
    clip: String,
    hold: Hold,
    from: Pos2,
    start: Picture,
    now: Picture,
    guides: (Option<f64>, Option<f64>),
}

#[derive(Debug, Clone)]
struct Wheeled {
    clip: String,
    start: Picture,
    now: Picture,
    zoom: f64,
    turn: f64,
    steps: f64,
    last: Instant,
}

#[derive(Debug, Default)]
pub struct Grab {
    pub(crate) tool: Option<PictureTool>,
    held: Option<Held>,
    wheel: Option<Wheeled>,
    landing: Option<(String, Picture, Instant, Picture)>,
    sent: Option<(String, Picture)>,
}

struct Shown {
    clip: String,
    picture: Picture,
    level: Level,
    origin: Pos2,
    scale: Vec2,
}

impl Shown {
    fn screen(&self, uv: [f64; 2]) -> Pos2 {
        let [x, y] = self.level.forward(uv);
        self.origin + vec2(x as f32 * self.scale.x, y as f32 * self.scale.y)
    }

    fn uv(&self, p: Pos2) -> [f64; 2] {
        let x = f64::from((p.x - self.origin.x) / self.scale.x);
        let y = f64::from((p.y - self.origin.y) / self.scale.y);
        let [a, b, e, c, d, f] = self.level.back;
        [a * x + b * y + e, c * x + d * y + f]
    }

    fn corners(&self) -> [Pos2; 4] {
        let [u0, v0, u1, v1] = self.level.shown;
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|uv| self.screen(uv))
    }

    fn side(&self, side: Side) -> Pos2 {
        let [u0, v0, u1, v1] = self.level.shown;
        let (um, vm) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
        self.screen(match side {
            Side::Left => [u0, vm],
            Side::Right => [u1, vm],
            Side::Top => [um, v0],
            Side::Bottom => [um, v1],
        })
    }

    fn pivot(&self) -> Pos2 {
        self.screen([0.5, 0.5])
    }

    fn turn_at(&self) -> Pos2 {
        let (top, bottom) = if self.picture.flip_vertical {
            (Side::Bottom, Side::Top)
        } else {
            (Side::Top, Side::Bottom)
        };
        let (a, b) = (self.side(top), self.side(bottom));
        a + (a - b).normalized() * LIFT
    }

    fn axis(&self, side: Side) -> Vec2 {
        let (a, b) = if side.across() {
            (self.screen([1.0, 0.5]), self.screen([0.0, 0.5]))
        } else {
            (self.screen([0.5, 1.0]), self.screen([0.5, 0.0]))
        };
        (a - b).normalized()
    }

    fn inside(&self, p: Pos2, margin: f32) -> bool {
        let [u, v] = self.uv(p);
        let [a, b, _, c, d, _] = self.level.back;
        let per = |x: f64, y: f64| f64::from(margin / self.scale.x) * x.hypot(y);
        let (mu, mv) = (per(a, b), per(c, d));
        let [u0, v0, u1, v1] = self.level.shown;
        u >= u0 - mu && u <= u1 + mu && v >= v0 - mv && v <= v1 + mv
    }

    fn grip(&self, p: Pos2, tool: PictureTool) -> Option<Hold> {
        let near = |q: Pos2| q.distance(p) <= GRAB * 1.6;
        match tool {
            PictureTool::Transform => {
                if near(self.turn_at()) {
                    Some(Hold::Turn)
                } else if self.corners().into_iter().any(near) {
                    Some(Hold::Corner)
                } else if let Some(s) = Side::ALL.into_iter().find(|s| near(self.side(*s))) {
                    Some(Hold::Stretch(s))
                } else if self.inside(p, GRAB) {
                    Some(Hold::Move)
                } else {
                    let reach = GRAB * 1.6 + TURN_BAND;
                    self.corners()
                        .into_iter()
                        .any(|q| q.distance(p) <= reach)
                        .then_some(Hold::Turn)
                }
            }
            PictureTool::Crop => Side::ALL
                .into_iter()
                .find(|s| near(self.side(*s)))
                .map(Hold::Crop),
        }
    }

    fn cursor(&self, hold: Hold, p: Pos2) -> CursorIcon {
        let along = |side: Side| {
            let n = self.axis(side);
            if n.x.abs() >= n.y.abs() {
                CursorIcon::ResizeHorizontal
            } else {
                CursorIcon::ResizeVertical
            }
        };
        match hold {
            Hold::Move => CursorIcon::Move,
            Hold::Turn => CursorIcon::PointingHand,
            Hold::Corner => {
                let d = p - self.pivot();
                if d.x * d.y > 0.0 {
                    CursorIcon::ResizeNwSe
                } else {
                    CursorIcon::ResizeNeSw
                }
            }
            Hold::Stretch(s) | Hold::Crop(s) => along(s),
        }
    }
}

fn content(p: &Project, c: &Clip, (w, h): (f64, f64)) -> [f64; 4] {
    let shown = p
        .media(&c.media)
        .and_then(|m| m.info.video.as_ref())
        .map(vidcut_core::model::VideoInfo::display_size)
        .filter(|(dw, dh)| *dw > 0 && *dh > 0);
    match shown {
        Some((dw, dh)) => {
            let dar = f64::from(dw) / f64::from(dh);
            let (fw, fh) = if dar > w / h {
                (w, w / dar)
            } else {
                (h * dar, h)
            };
            [(w - fw) / 2.0, (h - fh) / 2.0, fw, fh]
        }
        None => [0.0, 0.0, w, h],
    }
}

fn shape_of(p: &Project, c: &Clip, shown: Picture, picture: Rect) -> Option<Shown> {
    let (w, h) = (f64::from(p.settings.width), f64::from(p.settings.height));
    let placed = [Placed {
        clip: c.id.clone(),
        picture: shown.clone(),
    }];
    let level = Placing::new(&placed, (w, h), content(p, c, (w, h)))
        .levels
        .pop()?;
    Some(Shown {
        clip: c.id.clone(),
        picture: shown,
        level,
        origin: picture.min,
        scale: vec2(picture.width() / w as f32, picture.height() / h as f32),
    })
}

fn rounded(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

fn angle_of(v: Vec2) -> f32 {
    (-v.y).atan2(v.x)
}

fn wrapped(r: f64) -> f64 {
    (r + 180.0).rem_euclid(360.0) - 180.0
}

fn stepped(r: f64, up: bool) -> f64 {
    let n = r / STEP;
    let n = if up {
        (n + 1e-6).floor() + 1.0
    } else {
        (n - 1e-6).ceil() - 1.0
    };
    wrapped(n * STEP)
}

pub(super) fn target<'a>(p: &'a Project, selected: &[String], at: i64) -> Option<&'a Clip> {
    let shows = |ti: usize, c: &Clip| {
        let t = &p.tracks[ti];
        t.kind == TrackKind::Video
            && t.video_enabled
            && c.enabled
            && c.start <= at
            && at < c.end()
            && has_picture(c)
    };
    selected.iter().find_map(|id| {
        let (ti, ci) = p.find_clip(id)?;
        let c = &p.tracks[ti].clips[ci];
        shows(ti, c).then_some(c)
    })
}

pub(super) fn topmost(p: &Project, at: i64) -> Option<String> {
    p.tracks
        .iter()
        .rev()
        .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
        .flat_map(|t| t.clips.iter())
        .find(|c| c.enabled && c.start <= at && at < c.end() && has_picture(c))
        .map(|c| c.id.clone())
}

impl Workspace {
    fn picture_wanted(&self) -> Option<(String, Picture)> {
        let g = &self.timeline.picture_grab;
        if let Some(h) = &g.held {
            return Some((h.clip.clone(), h.now.clone()));
        }
        if let Some(w) = &g.wheel {
            return Some((w.clip.clone(), w.now.clone()));
        }
        let (clip, to, since, from) = g.landing.as_ref()?;
        let stored = self
            .project()?
            .clip(clip)?
            .picture
            .clone()
            .unwrap_or_default();
        landing(to, since.elapsed(), from, &stored).map(|t| (clip.clone(), t.clone()))
    }

    fn sync_picture_preview(&mut self) {
        let wanted = self.picture_wanted();
        let g = &mut self.timeline.picture_grab;
        if wanted.is_none() {
            g.landing = None;
        }
        if wanted != g.sent {
            self.live.preview_picture(wanted.clone());
            g.sent = wanted;
        }
    }

    fn land_picture(&mut self, clip: &str, from: &Picture, to: &Picture) {
        if let Some(params) = changed(clip, from, to) {
            self.send("picture.set", params);
            self.timeline.picture_grab.landing =
                Some((clip.to_string(), to.clone(), Instant::now(), from.clone()));
        }
    }

    fn end_picture_wheel(&mut self) {
        if let Some(w) = self.timeline.picture_grab.wheel.take() {
            self.land_picture(&w.clip, &w.start, &w.now);
        }
    }

    pub(super) fn settle_picture_wheel(&mut self, ctx: &egui::Context) {
        let Some(last) = self.timeline.picture_grab.wheel.as_ref().map(|w| w.last) else {
            return;
        };
        let quiet = last.elapsed();
        if quiet < STILL {
            ctx.request_repaint_after(STILL - quiet);
        } else {
            self.end_picture_wheel();
            self.sync_picture_preview();
        }
    }

    fn picture_wheel(&mut self, ui: &egui::Ui, s: &Shown, mods: (bool, bool)) {
        let (shift, alt) = mods;
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        let notch = ui
            .ctx()
            .options(|o| o.input_options.line_scroll_speed)
            .max(1.0);
        let notches = f64::from(if shift { scroll.x + scroll.y } else { scroll.y } / notch);
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        let k = f64::from(zoom) * if alt { 1.0 } else { NOTCH_ZOOM.powf(notches) };
        let turn = if alt { notches } else { 0.0 };
        if k == 1.0 && turn == 0.0 {
            return;
        }
        let w = self
            .timeline
            .picture_grab
            .wheel
            .get_or_insert_with(|| Wheeled {
                clip: s.clip.clone(),
                start: s.picture.clone(),
                now: s.picture.clone(),
                zoom: 1.0,
                turn: s.picture.turn(),
                steps: 0.0,
                last: Instant::now(),
            });
        w.last = Instant::now();
        if k != 1.0 {
            let [zx, zy] = w.start.scale();
            w.zoom = (w.zoom * k).clamp(ZOOM.0 / zx.min(zy), ZOOM.1 / zx.max(zy));
            w.now.zoom = Some([rounded(zx * w.zoom, 4), rounded(zy * w.zoom, 4)]);
        }
        if turn != 0.0 {
            if shift {
                w.steps += turn;
                while w.steps >= 0.5 {
                    w.turn = stepped(w.turn, true);
                    w.steps -= 1.0;
                }
                while w.steps <= -0.5 {
                    w.turn = stepped(w.turn, false);
                    w.steps += 1.0;
                }
            } else {
                w.steps = 0.0;
                w.turn = wrapped(w.turn + turn);
            }
            w.now.rotation = Some(rounded(w.turn, 2));
        }
    }

    pub(super) fn picture_tool(&mut self, tool: PictureTool) {
        if self.timeline.picture_grab.tool == Some(tool) {
            self.timeline.picture_grab.tool = None;
        } else {
            self.show_picture_tool(tool);
        }
    }

    pub(super) fn show_picture_tool(&mut self, tool: PictureTool) {
        self.timeline.picture_grab.tool = Some(tool);
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let Some(p) = self.project_shared() else {
            return;
        };
        if target(&p, &self.snap.selection.clips, at).is_none()
            && let Some(id) = topmost(&p, at)
        {
            self.send("selection.set", json!({ "clips": [id] }));
        }
    }

    fn picture_shape(&self, picture: Rect) -> Option<Shown> {
        let p = self.project_shared()?;
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let c = target(&p, &self.snap.selection.clips, at)?;
        let shown = match self.picture_wanted() {
            Some((id, x)) if id == c.id => x,
            _ => c.picture.clone().unwrap_or_default(),
        };
        shape_of(&p, c, shown, picture)
    }

    fn picture_moved(
        &self,
        held: &Held,
        s: &Shown,
        to: Pos2,
        picture: Rect,
        mods: (bool, bool),
    ) -> Held {
        let (shift, free) = mods;
        let mut next = held.clone();
        let p0 = &held.start;
        let pivot = s.pivot();
        match held.hold {
            Hold::Move => {
                let mut d = to - held.from;
                let across = d.x.abs() >= d.y.abs();
                let (x_free, y_free) = (!shift || across, !shift || !across);
                if !x_free {
                    d.x = 0.0;
                }
                if !y_free {
                    d.y = 0.0;
                }
                let [x0, y0] = p0.place();
                let mut x = x0 + f64::from(d.x / picture.width());
                let mut y = y0 + f64::from(d.y / picture.height());
                let mut guides = (None, None);
                if !free {
                    let near = |v: f64, span: f32| {
                        LINES
                            .into_iter()
                            .find(|l| ((v - l).abs() as f32) * span <= SNAP)
                    };
                    if let Some(l) = near(x, picture.width()).filter(|_| x_free) {
                        x = l;
                        guides.0 = Some(l);
                    }
                    if let Some(l) = near(y, picture.height()).filter(|_| y_free) {
                        y = l;
                        guides.1 = Some(l);
                    }
                }
                next.now.position = Some([rounded(x, 4), rounded(y, 4)]);
                next.guides = guides;
            }
            Hold::Corner => {
                let (d0, d1) = (held.from.distance(pivot), to.distance(pivot));
                if d0 > 1.0 {
                    let [zx, zy] = p0.scale();
                    let k = f64::from(d1 / d0);
                    let k = k.clamp(ZOOM.0 / zx.min(zy), ZOOM.1 / zx.max(zy));
                    next.now.zoom = Some([rounded(zx * k, 4), rounded(zy * k, 4)]);
                }
            }
            Hold::Stretch(side) => {
                let axis = s.axis(side);
                let (s0, s1) = ((held.from - pivot).dot(axis), (to - pivot).dot(axis));
                if s0.abs() > 1.0 {
                    let [zx, zy] = p0.scale();
                    let k = f64::from(s1 / s0).max(ZOOM.0);
                    next.now.zoom = Some(if side.across() {
                        [rounded((zx * k).clamp(ZOOM.0, ZOOM.1), 4), zy]
                    } else {
                        [zx, rounded((zy * k).clamp(ZOOM.0, ZOOM.1), 4)]
                    });
                }
            }
            Hold::Turn => {
                let turned =
                    f64::from(angle_of(to - pivot) - angle_of(held.from - pivot)).to_degrees();
                let mut r = p0.turn() + turned;
                if shift {
                    r = (r / STEP).round() * STEP;
                }
                next.now.rotation = Some(rounded(wrapped(r), 2));
            }
            Hold::Crop(side) => {
                let [u, v] = s.uv(to);
                let [a, b, _, c, d, _] = s.level.back;
                let least = |x: f64, y: f64| f64::from(LEFT_SHOWING / s.scale.x) * x.hypot(y);
                let (lu, lv) = (least(a, b), least(c, d));
                let k = &mut next.now.crop;
                let side_of = |v: f64, other: f64, least: f64| {
                    rounded(v.clamp(0.0, (1.0 - other - least).max(0.0)), 4)
                };
                match side {
                    Side::Left => k.left = side_of(u, k.right, lu),
                    Side::Right => k.right = side_of(1.0 - u, k.left, lu),
                    Side::Top => k.top = side_of(v, k.bottom, lv),
                    Side::Bottom => k.bottom = side_of(1.0 - v, k.top, lv),
                }
            }
        }
        next
    }

    pub(super) fn picture_controls(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        picture: Rect,
    ) -> bool {
        let tool = self.timeline.picture_grab.tool;
        let shape = tool.and_then(|_| self.picture_shape(picture));
        let (Some(tool), Some(s)) = (tool, shape) else {
            self.timeline.picture_grab.held = None;
            self.end_picture_wheel();
            self.sync_picture_preview();
            return false;
        };
        let elsewhere = |w: &Wheeled| w.clip != s.clip;
        if tool != PictureTool::Transform
            || resp.drag_started()
            || self
                .timeline
                .picture_grab
                .wheel
                .as_ref()
                .is_some_and(elsewhere)
        {
            self.end_picture_wheel();
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let mods = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let held = self.timeline.picture_grab.held.clone();
        let hover = pointer
            .filter(|_| resp.hovered())
            .and_then(|p| s.grip(p, tool).map(|h| (h, p)));
        let cursor = match &held {
            Some(h) => pointer.map(|p| s.cursor(h.hold, p)),
            None => hover.map(|(h, p)| s.cursor(h, p)),
        };
        if let Some(c) = cursor {
            ui.ctx().set_cursor_icon(c);
        }
        if resp.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some(hold) = s.grip(press, tool)
        {
            let start = s.picture.clone();
            self.timeline.picture_grab.held = Some(Held {
                clip: s.clip.clone(),
                hold,
                from: press,
                now: start.clone(),
                start,
                guides: (None, None),
            });
        }
        if let (Some(held), Some(to), Some(p)) = (
            self.timeline.picture_grab.held.clone(),
            pointer,
            self.project_shared(),
        ) && resp.dragged()
            && let Some(s0) = p
                .clip(&held.clip)
                .and_then(|c| shape_of(&p, c, held.start.clone(), picture))
        {
            self.timeline.picture_grab.held =
                Some(self.picture_moved(&held, &s0, to, picture, mods));
        }
        if tool == PictureTool::Transform
            && self.timeline.picture_grab.held.is_none()
            && resp.hovered()
            && pointer.is_some_and(|p| picture.contains(p))
        {
            self.picture_wheel(ui, &s, mods);
        }
        let mut busy = held.is_some() || hover.is_some();
        if resp.drag_stopped()
            && let Some(held) = self.timeline.picture_grab.held.take()
        {
            busy = true;
            self.land_picture(&held.clip, &held.start, &held.now);
        }
        let g = &self.timeline.picture_grab;
        let zooming = match (&g.held, &g.wheel) {
            (Some(h), _) => matches!(h.hold, Hold::Corner | Hold::Stretch(_)).then_some(&h.now),
            (None, Some(w)) => (w.now.scale() != w.start.scale()).then_some(&w.now),
            (None, None) => None,
        };
        if let Some(now) = zooming {
            let [zx, zy] = now.scale();
            let zoom = if zx == zy {
                format!("{:.0}%", zx * 100.0)
            } else {
                format!("{:.0}% × {:.0}%", zx * 100.0, zy * 100.0)
            };
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                resp.id,
                egui::PopupAnchor::Pointer,
            )
            .gap(12.0)
            .show(|ui| ui.add(egui::Label::new(zoom).extend()));
        }
        self.sync_picture_preview();
        let shown = self.picture_shape(picture).unwrap_or(s);
        self.paint_picture_controls(ui, picture, &shown, tool);
        busy
    }

    fn paint_picture_controls(&self, ui: &egui::Ui, picture: Rect, s: &Shown, tool: PictureTool) {
        let painter = ui.painter_at(picture);
        let accent = super::theme::colors().accent;
        let stroke = Stroke::new(1.5, accent);
        let corners = s.corners();
        for i in 0..4 {
            painter.line_segment([corners[i], corners[(i + 1) % 4]], stroke);
        }
        match tool {
            PictureTool::Transform => {
                for c in corners {
                    painter.rect_filled(
                        Rect::from_center_size(c, Vec2::splat(GRAB * 1.4)),
                        1.0,
                        accent,
                    );
                }
                for side in Side::ALL {
                    painter.circle_filled(s.side(side), GRAB * 0.7, accent);
                }
                let top = if s.picture.flip_vertical {
                    Side::Bottom
                } else {
                    Side::Top
                };
                painter.line_segment([s.side(top), s.turn_at()], stroke);
                painter.circle_filled(s.turn_at(), GRAB * 0.9, accent);
            }
            PictureTool::Crop => {
                let whole = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|uv| s.screen(uv));
                let dashed = Stroke::new(1.0, accent);
                for i in 0..4 {
                    painter.extend(egui::Shape::dashed_line(
                        &[whole[i], whole[(i + 1) % 4]],
                        dashed,
                        6.0,
                        4.0,
                    ));
                }
                for side in Side::ALL {
                    let at = s.side(side);
                    let along = s.axis(if side.across() { Side::Top } else { Side::Left });
                    let half = along * GRAB * 2.5;
                    painter.line_segment([at - half, at + half], Stroke::new(GRAB, accent));
                }
            }
        }
        if let Some(h) = &self.timeline.picture_grab.held {
            let guide = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 140));
            if let Some(x) = h.guides.0 {
                let gx = picture.min.x + x as f32 * picture.width();
                painter.line_segment([pos2(gx, picture.min.y), pos2(gx, picture.max.y)], guide);
            }
            if let Some(y) = h.guides.1 {
                let gy = picture.min.y + y as f32 * picture.height();
                painter.line_segment([pos2(picture.min.x, gy), pos2(picture.max.x, gy)], guide);
            }
        }
    }
}

fn changed(clip: &str, a: &Picture, b: &Picture) -> Option<Value> {
    let mut params = json!({ "clips": clip });
    if a.place() != b.place() {
        params["position"] = json!(b.place());
    }
    if a.scale() != b.scale() {
        params["zoom"] = json!(b.scale());
    }
    if a.turn() != b.turn() {
        params["rotation"] = json!(b.turn());
    }
    for (key, x, y) in [
        ("crop_left", a.crop.left, b.crop.left),
        ("crop_right", a.crop.right, b.crop.right),
        ("crop_top", a.crop.top, b.crop.top),
        ("crop_bottom", a.crop.bottom, b.crop.bottom),
    ] {
        if x != y {
            params[key] = json!(y);
        }
    }
    params
        .as_object()
        .is_some_and(|o| o.len() > 1)
        .then_some(params)
}
