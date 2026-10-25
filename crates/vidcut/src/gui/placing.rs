use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use serde_json::{Value, json};
use vidcut_core::model::{
    Clip, DynamicZoom, Ease, Picture, PictureTool, Project, TrackKind, ZoomBox,
};
use vidcut_core::ops::picture::{has_picture, zoom_shown};
use vidcut_core::place::{Level, Placing, anchored, apply, flipped};
use vidcut_core::render::{Placed, Run};

use super::timeline::tools::landing;
use super::workspace::Workspace;

const GRAB: f32 = 6.0;
const SNAP: f32 = 8.0;
const LIFT: f32 = 26.0;
const LINES: [f64; 3] = [1.0 / 3.0, 0.5, 2.0 / 3.0];
const ZOOM: (f64, f64) = (0.01, 100.0);
const REACH: f64 = 100.0;
const START: Color32 = Color32::from_rgb(70, 200, 90);
const END: Color32 = Color32::from_rgb(230, 70, 60);
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
enum Boxed {
    Start,
    End,
}

impl Boxed {
    fn of(self, z: &DynamicZoom) -> ZoomBox {
        match self {
            Boxed::Start => z.start,
            Boxed::End => z.end,
        }
    }

    fn set(self, z: &mut DynamicZoom, b: ZoomBox) {
        match self {
            Boxed::Start => z.start = b,
            Boxed::End => z.end = b,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    Move,
    Corner,
    Stretch(Side),
    Turn,
    Crop(Side),
    ZoomMove(Boxed),
    ZoomSize(Boxed),
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
    pub(crate) row: bool,
    held: Option<Held>,
    wheel: Option<Wheeled>,
    landing: Option<(String, Picture, Instant, Picture)>,
    sent: Vec<(String, Picture)>,
}

impl Grab {
    pub(super) fn cancel_landing(&mut self) {
        self.landing = None;
    }
}

fn slopes(back: &[f64; 8], [x, y]: [f64; 2]) -> [f64; 2] {
    let [u, v] = apply(back, [x, y]);
    let [a, b, _, c, d, _, g, h] = *back;
    let w = (g * x + h * y + 1.0).abs().max(1e-9);
    [
        (a - u * g).hypot(b - u * h) / w,
        (c - v * g).hypot(d - v * h) / w,
    ]
}

struct Shown {
    clip: String,
    picture: Picture,
    level: Level,
    middle: [f64; 2],
    anchor: [f64; 2],
    origin: Pos2,
    scale: Vec2,
    plain: Level,
    ends: [Level; 2],
}

impl Shown {
    fn at(&self, [x, y]: [f64; 2]) -> Pos2 {
        self.origin + vec2(x as f32 * self.scale.x, y as f32 * self.scale.y)
    }

    fn screen(&self, uv: [f64; 2]) -> Pos2 {
        self.at(self.level.forward(uv))
    }

    fn frame_at(&self, p: Pos2) -> [f64; 2] {
        [
            f64::from((p.x - self.origin.x) / self.scale.x),
            f64::from((p.y - self.origin.y) / self.scale.y),
        ]
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
        self.at(self.anchor)
    }

    fn zoom(&self) -> DynamicZoom {
        self.picture.dynamic_zoom.unwrap_or_default()
    }

    fn on(&self, l: &Level, uv: [f64; 2]) -> Pos2 {
        self.at(l.forward(uv))
    }

    fn back(&self, l: &Level, p: Pos2) -> [f64; 2] {
        apply(&l.back, self.frame_at(p))
    }

    fn end(&self, e: Boxed) -> &Level {
        match e {
            Boxed::Start => &self.ends[0],
            Boxed::End => &self.ends[1],
        }
    }

    fn end_corners(&self, e: Boxed) -> [Pos2; 4] {
        let l = self.end(e);
        let [u0, v0, u1, v1] = l.shown;
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]].map(|uv| self.on(l, uv))
    }

    fn end_middle(&self, e: Boxed) -> Pos2 {
        let l = self.end(e);
        let [u0, v0, u1, v1] = l.shown;
        self.on(l, [(u0 + u1) / 2.0, (v0 + v1) / 2.0])
    }

    fn end_holds(&self, e: Boxed, p: Pos2) -> bool {
        let l = self.end(e);
        let [u, v] = self.back(l, p);
        let [u0, v0, u1, v1] = l.shown;
        u >= u0 && u <= u1 && v >= v0 && v <= v1
    }

    fn plain_span(&self) -> [f64; 2] {
        let l = &self.plain;
        let across = self.on(l, [1.0, 0.5]) - self.on(l, [0.0, 0.5]);
        let down = self.on(l, [0.5, 1.0]) - self.on(l, [0.5, 0.0]);
        [f64::from(across.length()), f64::from(down.length())]
    }

    fn flips(&self) -> [f64; 2] {
        let sign = |on: bool| if on { -1.0 } else { 1.0 };
        [
            sign(self.picture.flip_horizontal),
            sign(self.picture.flip_vertical),
        ]
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
        let at = self.frame_at(p);
        let [u, v] = apply(&self.level.back, at);
        let [du, dv] = slopes(&self.level.back, at);
        let per = |d: f64| f64::from(margin / self.scale.x) * d;
        let (mu, mv) = (per(du), per(dv));
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
            PictureTool::DynamicZoom => {
                let z = self.zoom();
                let ends = [Boxed::Start, Boxed::End];
                if let Some(e) = ends
                    .into_iter()
                    .find(|e| self.end_corners(*e).into_iter().any(near))
                {
                    return Some(Hold::ZoomSize(e));
                }
                [Boxed::End, Boxed::Start]
                    .into_iter()
                    .filter(|e| self.end_holds(*e, p))
                    .max_by(|a, b| a.of(&z).size.total_cmp(&b.of(&z).size))
                    .map(Hold::ZoomMove)
            }
            PictureTool::Composite => None,
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
        let diagonal = |d: Vec2| {
            if d.x * d.y > 0.0 {
                CursorIcon::ResizeNwSe
            } else {
                CursorIcon::ResizeNeSw
            }
        };
        match hold {
            Hold::Move | Hold::ZoomMove(_) => CursorIcon::Move,
            Hold::Turn => CursorIcon::PointingHand,
            Hold::Corner => diagonal(p - self.pivot()),
            Hold::ZoomSize(e) => diagonal(p - self.end_middle(e)),
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

fn shape_of(p: &Project, c: &Clip, shown: Picture, picture: Rect, at: i64) -> Option<Shown> {
    let (w, h) = (f64::from(p.settings.width), f64::from(p.settings.height));
    let run = Run {
        at: at - c.start,
        of: c.duration,
        held: false,
    };
    let fit = content(p, c, (w, h));
    let level_of = |x: Picture| {
        Placing::new(&[Placed::at(c, x, run, at)], (w, h), fit)
            .levels
            .pop()
    };
    let placed = [Placed::at(c, shown.clone(), run, at)];
    let middle = placed[0]
        .zoomed()
        .map_or(Picture::MIDDLE, |b| flipped(&shown, b.center));
    let level = Placing::new(&placed, (w, h), fit).levels.pop()?;
    let plain = level_of(Picture {
        dynamic_zoom: None,
        ..shown.clone()
    })?;
    let z = shown.dynamic_zoom.unwrap_or_default();
    let fixed = |b: ZoomBox| Picture {
        dynamic_zoom: Some(DynamicZoom {
            start: b,
            end: b,
            ..z
        }),
        ..shown.clone()
    };
    let ends = [level_of(fixed(z.start))?, level_of(fixed(z.end))?];
    Some(Shown {
        clip: c.id.clone(),
        anchor: anchored(&shown, (w, h), fit),
        picture: shown,
        level,
        middle,
        origin: picture.min,
        scale: vec2(picture.width() / w as f32, picture.height() / h as f32),
        plain,
        ends,
    })
}

pub(super) fn ease_label(e: Ease) -> &'static str {
    match e {
        Ease::Linear => "Linear",
        Ease::EaseIn => "Ease In",
        Ease::EaseOut => "Ease Out",
        Ease::EaseInOut => "Ease In and Out",
    }
}

fn rounded(v: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (v * k).round() / k
}

fn angle_of(v: Vec2) -> f32 {
    (-v.y).atan2(v.x)
}

fn zoom_text(p: &Picture) -> String {
    let [zx, zy] = p.scale();
    if zx == zy {
        format!("{:.0}%", zx * 100.0)
    } else {
        format!("{:.0}% × {:.0}%", zx * 100.0, zy * 100.0)
    }
}

fn turn_text(p: &Picture) -> String {
    let r = p.turn();
    if (r - r.round()).abs() < 0.05 {
        let whole = r.round() + 0.0;
        format!("{whole:.0}°")
    } else {
        format!("{r:.1}°")
    }
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
    fn grab_wanted(&self) -> Option<(String, Picture)> {
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

    pub(super) fn picture_wanted(&self) -> Vec<(String, Picture)> {
        let frame = self.snap.timeline.position;
        let preview = |id: &str, baseline: &Picture, picture: &Picture| {
            let Some(p) = self.project() else {
                return (id.to_string(), picture.clone());
            };
            let Some(c) = p.clip(id) else {
                return (id.to_string(), picture.clone());
            };
            let stored = c.picture.clone().unwrap_or_default();
            let at = c.source_at(frame);
            (id.to_string(), stored.preview(baseline, picture, at))
        };
        match self.grab_wanted() {
            Some((id, picture)) => {
                let baseline = self
                    .timeline
                    .picture_grab
                    .held
                    .as_ref()
                    .map(|h| h.start.clone())
                    .or_else(|| {
                        self.timeline
                            .picture_grab
                            .wheel
                            .as_ref()
                            .map(|w| w.start.clone())
                    })
                    .unwrap_or_else(|| picture.clone());
                vec![preview(&id, &baseline, &picture)]
            }
            None => self
                .inspector_wanted()
                .into_iter()
                .map(|(id, picture)| {
                    let stored = self
                        .project()
                        .and_then(|p| p.clip(&id))
                        .and_then(|c| c.picture.clone())
                        .unwrap_or_default();
                    preview(&id, &stored, &picture)
                })
                .collect(),
        }
    }

    pub(super) fn sync_picture_preview(&mut self) {
        if self.grab_wanted().is_none() {
            self.timeline.picture_grab.landing = None;
        }
        self.settle_inspector();
        let shown = self.picture_wanted();
        let g = &mut self.timeline.picture_grab;
        if shown != g.sent {
            self.live.preview_pictures(shown.clone());
            g.sent = shown;
        }
    }

    fn land_picture(&mut self, clip: &str, from: &Picture, to: &Picture) {
        if let Some(mut params) = changed(clip, from, to) {
            let keyed = self
                .project()
                .and_then(|p| p.clip(clip))
                .and_then(|c| c.picture.as_ref().filter(|pic| pic.is_keyed()).map(|_| ()));
            if keyed.is_some() {
                params["at"] = json!(self.snap.timeline.position);
            }
            self.send("picture.set", params);
            self.timeline.picture_grab.landing =
                Some((clip.to_string(), to.clone(), Instant::now(), from.clone()));
        }
    }

    pub(super) fn end_picture_wheel(&mut self) {
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
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let frame = self
            .project_shared()
            .and_then(|p| p.clip(&s.clip).map(|c| c.source_at(at)));
        let adopted = frame
            .map(|f| s.picture.adopt(f as f64))
            .unwrap_or_else(|| s.picture.clone());
        let turn0 = adopted.turn();
        let w = self
            .timeline
            .picture_grab
            .wheel
            .get_or_insert_with(|| Wheeled {
                clip: s.clip.clone(),
                now: adopted.clone(),
                start: adopted,
                zoom: 1.0,
                turn: turn0,
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

    fn tools_clip(&self) -> Option<(String, Picture)> {
        let p = self.project_shared()?;
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let c = match target(&p, &self.snap.selection.clips, at) {
            Some(c) => c,
            None => p.clip(&topmost(&p, at)?)?,
        };
        Some((c.id.clone(), c.picture.clone().unwrap_or_default()))
    }

    pub(super) fn picture_tools_row(&mut self, ui: &mut egui::Ui) {
        let tool = self.timeline.picture_grab.tool;
        let clip = self.tools_clip();
        let found = clip.is_some();
        for (label, t) in [
            ("Transform", PictureTool::Transform),
            ("Crop", PictureTool::Crop),
            ("Dynamic Zoom", PictureTool::DynamicZoom),
        ] {
            let lit = tool == Some(t);
            let b = egui::Button::selectable(lit, label);
            if ui.add_enabled(found || lit, b).clicked() {
                self.picture_tool(t);
            }
        }
        let Some(tool) = tool else {
            return;
        };
        ui.separator();
        let pic = clip.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
        if tool == PictureTool::Transform {
            use egui_phosphor::regular::{FLIP_HORIZONTAL, FLIP_VERTICAL};
            for (icon, label, key, on) in [
                (
                    FLIP_HORIZONTAL,
                    "Flip Horizontal",
                    "flip_horizontal",
                    pic.flip_horizontal,
                ),
                (
                    FLIP_VERTICAL,
                    "Flip Vertical",
                    "flip_vertical",
                    pic.flip_vertical,
                ),
            ] {
                let b = egui::Button::selectable(on, format!("{icon} {label}"));
                if ui.add_enabled(found, b).clicked()
                    && let Some((id, _)) = &clip
                {
                    let mut params = json!({ "clips": id });
                    params[key] = json!(!on);
                    self.end_picture_wheel();
                    self.send("picture.set", params);
                }
            }
        }
        if tool == PictureTool::DynamicZoom {
            self.zoom_tools(ui, clip.as_ref().map(|(id, _)| id.as_str()), &pic);
        }
        if ui.add_enabled(found, egui::Button::new("Reset")).clicked()
            && let Some((id, _)) = &clip
        {
            self.end_picture_wheel();
            self.timeline.picture_grab.landing = None;
            self.send("picture.clear", json!({ "clips": [id], "tool": tool }));
        }
    }

    fn zoom_tools(&mut self, ui: &mut egui::Ui, clip: Option<&str>, pic: &Picture) {
        let z = pic.dynamic_zoom.unwrap_or_default();
        let mut wanted = None;
        if ui
            .add_enabled(clip.is_some(), egui::Button::new("Swap"))
            .clicked()
        {
            wanted = Some(z.swapped());
        }
        ui.add_enabled_ui(clip.is_some(), |ui| {
            egui::ComboBox::from_id_salt("dynamic-zoom-ease")
                .selected_text(ease_label(z.ease))
                .show_ui(ui, |ui| {
                    for e in Ease::ALL {
                        if ui.selectable_label(e == z.ease, ease_label(e)).clicked() {
                            wanted = Some(DynamicZoom { ease: e, ..z });
                        }
                    }
                });
        });
        if let (Some(id), Some(z)) = (clip, wanted)
            && pic.dynamic_zoom != Some(z)
        {
            self.end_picture_wheel();
            self.timeline.picture_grab.landing = None;
            self.send(
                "picture.set",
                json!({ "clips": id, "dynamic_zoom": zoom_shown(&z) }),
            );
        }
    }

    fn picture_shape(&self, picture: Rect) -> Option<Shown> {
        let p = self.project_shared()?;
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let c = target(&p, &self.snap.selection.clips, at)?;
        self.timeline.picture_grab.tool?;
        let shown = match self
            .picture_wanted()
            .into_iter()
            .find(|(id, _)| *id == c.id)
        {
            Some((_, x)) => x,
            None => c.picture.clone().unwrap_or_default(),
        };
        shape_of(&p, c, shown, picture, at)
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
                let m = s.screen(s.middle);
                let (ex, ey) = (
                    f64::from((m.x - picture.min.x) / picture.width()) - x0,
                    f64::from((m.y - picture.min.y) / picture.height()) - y0,
                );
                let mut x = x0 + f64::from(d.x / picture.width());
                let mut y = y0 + f64::from(d.y / picture.height());
                let mut guides = (None, None);
                if !free {
                    let near = |v: f64, span: f32| {
                        LINES
                            .into_iter()
                            .find(|l| ((v - l).abs() as f32) * span <= SNAP)
                    };
                    if let Some(l) = near(x + ex, picture.width()).filter(|_| x_free) {
                        x = l - ex;
                        guides.0 = Some(l);
                    }
                    if let Some(l) = near(y + ey, picture.height()).filter(|_| y_free) {
                        y = l - ey;
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
                let at = s.frame_at(to);
                let [u, v] = apply(&s.level.back, at);
                let [du, dv] = slopes(&s.level.back, at);
                let least = |d: f64| f64::from(LEFT_SHOWING / s.scale.x) * d;
                let (lu, lv) = (least(du), least(dv));
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
            Hold::ZoomMove(e) => {
                let mut z = p0.dynamic_zoom.unwrap_or_default();
                let b = e.of(&z);
                let ([u0, v0], [u1, v1]) = (s.back(&s.plain, held.from), s.back(&s.plain, to));
                let [sx, sy] = s.flips();
                let (mut dx, mut dy) = (-sx * (u1 - u0) * b.size, -sy * (v1 - v0) * b.size);
                if shift {
                    let [across, down] = s.plain_span();
                    if (dx * across).abs() >= (dy * down).abs() {
                        dy = 0.0;
                    } else {
                        dx = 0.0;
                    }
                }
                let at = |v: f64| rounded(v.clamp(-REACH, REACH), 4);
                let center = [at(b.center[0] + dx), at(b.center[1] + dy)];
                e.set(&mut z, ZoomBox { center, ..b });
                next.now.dynamic_zoom = Some(z);
            }
            Hold::ZoomSize(e) => {
                let mut z = p0.dynamic_zoom.unwrap_or_default();
                let b = e.of(&z);
                let middle = s.end_middle(e);
                let (d0, d1) = (held.from.distance(middle), to.distance(middle));
                if d0 > 1.0 {
                    let size = (b.size * f64::from(d0 / d1.max(0.01))).clamp(ZOOM.0, ZOOM.1);
                    let keep = |c: f64| rounded(0.5 - (0.5 - c) / b.size * size, 4);
                    e.set(
                        &mut z,
                        ZoomBox {
                            center: [keep(b.center[0]), keep(b.center[1])],
                            size: rounded(size, 4),
                        },
                    );
                    next.now.dynamic_zoom = Some(z);
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
            let at = self.snap.player(crate::live::PlayerId::Timeline).position;
            let frame = self
                .project_shared()
                .and_then(|p| p.clip(&s.clip).map(|c| c.source_at(at)));
            let start = frame
                .map(|f| s.picture.adopt(f as f64))
                .unwrap_or_else(|| s.picture.clone());
            self.timeline.picture_grab.held = Some(Held {
                clip: s.clip.clone(),
                hold,
                from: press,
                now: start.clone(),
                start,
                guides: (None, None),
            });
        }
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        if let (Some(held), Some(to), Some(p)) = (
            self.timeline.picture_grab.held.clone(),
            pointer,
            self.project_shared(),
        ) && resp.dragged()
            && let Some(s0) = p
                .clip(&held.clip)
                .and_then(|c| shape_of(&p, c, held.start.clone(), picture, at))
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
        let readout = match (&g.held, &g.wheel) {
            (Some(h), _) => match h.hold {
                Hold::Corner | Hold::Stretch(_) => Some(zoom_text(&h.now)),
                Hold::Turn => Some(turn_text(&h.now)),
                _ => None,
            },
            (None, Some(w)) if w.now.turn() != w.start.turn() => Some(turn_text(&w.now)),
            (None, Some(w)) if w.now.scale() != w.start.scale() => Some(zoom_text(&w.now)),
            (None, _) => None,
        };
        if let Some(text) = readout {
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                resp.id,
                egui::PopupAnchor::Pointer,
            )
            .gap(12.0)
            .show(|ui| ui.add(egui::Label::new(text).extend()));
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
                let anchor = s.pivot();
                for arm in [vec2(GRAB, 0.0), vec2(0.0, GRAB)] {
                    painter.line_segment([anchor - arm, anchor + arm], stroke);
                }
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
            PictureTool::DynamicZoom => paint_boxes(&painter, s),
            PictureTool::Composite => {}
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

fn paint_boxes(painter: &egui::Painter, s: &Shown) {
    let on = s.picture.dynamic_zoom.is_some();
    let path = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 160));
    painter.line_segment([s.end_middle(Boxed::Start), s.end_middle(Boxed::End)], path);
    for (e, color) in [(Boxed::End, END), (Boxed::Start, START)] {
        let corners = s.end_corners(e);
        let line = Stroke::new(1.5, color);
        for i in 0..4 {
            let edge = [corners[i], corners[(i + 1) % 4]];
            if on {
                painter.line_segment(edge, line);
            } else {
                painter.extend(egui::Shape::dashed_line(&edge, line, 6.0, 4.0));
            }
        }
        for c in corners {
            painter.rect_filled(
                Rect::from_center_size(c, Vec2::splat(GRAB * 1.4)),
                1.0,
                color,
            );
        }
        painter.circle_filled(s.end_middle(e), GRAB * 0.5, color);
    }
}

pub(super) fn changed(clip: &str, a: &Picture, b: &Picture) -> Option<Value> {
    let mut params = json!({ "clips": clip });
    if a.dynamic_zoom != b.dynamic_zoom {
        params["dynamic_zoom"] = b.dynamic_zoom.as_ref().map_or(Value::Null, zoom_shown);
    }
    if a.place() != b.place() {
        params["position"] = json!(b.place());
    }
    if a.scale() != b.scale() {
        params["zoom"] = json!(b.scale());
    }
    if a.turn() != b.turn() {
        params["rotation"] = json!(b.turn());
    }
    if a.pivot() != b.pivot() {
        params["anchor"] = json!(b.pivot());
    }
    if a.pitch() != b.pitch() {
        params["pitch"] = json!(b.pitch());
    }
    if a.yaw() != b.yaw() {
        params["yaw"] = json!(b.yaw());
    }
    if a.flip_horizontal != b.flip_horizontal {
        params["flip_horizontal"] = json!(b.flip_horizontal);
    }
    if a.flip_vertical != b.flip_vertical {
        params["flip_vertical"] = json!(b.flip_vertical);
    }
    for (key, x, y) in [
        ("crop_left", a.crop.left, b.crop.left),
        ("crop_right", a.crop.right, b.crop.right),
        ("crop_top", a.crop.top, b.crop.top),
        ("crop_bottom", a.crop.bottom, b.crop.bottom),
        ("crop_softness", a.softness(), b.softness()),
        ("opacity", a.opaque(), b.opaque()),
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
