use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use serde_json::{Value, json};
use vidcut_core::model::{
    AreaShape, Clip, CompositeMode, DynamicZoom, Ease, EffectArea, EffectType, Picture,
    PictureTool, Project, Subject, TrackKind, ZoomBox,
};
use vidcut_core::motion::Area;
use vidcut_core::ops::picture::{has_picture, zoom_shown};
use vidcut_core::place::{Level, Placing, anchored, apply, flipped};
use vidcut_core::render::{Placed, Run};

use super::timeline::tools::landing;
use super::title_browser::LeftTab;
use super::workspace::Workspace;

const GRAB: f32 = 6.0;
const SNAP: f32 = 8.0;
const LIFT: f32 = 26.0;
const LINES: [f64; 3] = [1.0 / 3.0, 0.5, 2.0 / 3.0];
const ZOOM: (f64, f64) = (0.01, 100.0);
const START: Color32 = Color32::from_rgb(70, 200, 90);
const END: Color32 = Color32::from_rgb(230, 70, 60);
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
    pub(crate) area: Option<usize>,
    blur: bool,
    shape: AreaShape,
    blurred: Option<String>,
    drawn: Option<(String, Vec<[f64; 2]>)>,
    area_held: Option<AreaHeld>,
    held: Option<Held>,
    wheel: Option<Wheeled>,
    landing: Option<(String, Picture, Instant, Picture)>,
    sent: Vec<(String, Picture)>,
}

impl Grab {
    pub(super) fn cancel_landing(&mut self) {
        self.landing = None;
    }

    pub(crate) fn blurring(&self) -> bool {
        self.blur && self.tool.is_none()
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
    whole: f64,
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

    fn cropped(&self, side: Side) -> f64 {
        let k = &self.picture.crop;
        match side {
            Side::Left => k.left,
            Side::Right => k.right,
            Side::Top => k.top,
            Side::Bottom => k.bottom,
        }
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
                if self.corners().into_iter().any(near) {
                    Some(Hold::Corner)
                } else if near(self.turn_at()) {
                    Some(Hold::Turn)
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
                .rev()
                .filter(|s| near(self.side(*s)))
                .max_by(|a, b| self.cropped(*a).total_cmp(&self.cropped(*b)))
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

fn pixels_of(p: &Project, c: &Clip, frame: (f64, f64)) -> (f64, f64) {
    p.media(&c.media)
        .and_then(|m| m.info.video.as_ref())
        .filter(|v| v.width > 0 && v.height > 0)
        .map_or(frame, |v| {
            vidcut_core::place::upright_pixels((v.width, v.height), v.rotation)
        })
}

fn shape_of(p: &Project, c: &Clip, shown: Picture, picture: Rect, at: i64) -> Option<Shown> {
    let (w, h) = (f64::from(p.settings.width), f64::from(p.settings.height));
    let run = Run {
        at: at - c.start,
        of: c.duration,
        held: false,
    };
    let fit = content(p, c, (w, h));
    let pixels = pixels_of(p, c, (w, h));
    let level_of =
        |x: Picture| Placing::new(&[Placed::at(c, x, run, at)], (w, h), fit, pixels).level;
    let placed = [Placed::at(c, shown.clone(), run, at)];
    let middle = placed[0]
        .zoomed()
        .map_or(Picture::MIDDLE, |b| flipped(&shown, b.center));
    let level = Placing::new(&placed, (w, h), fit, pixels).level;
    let plain = level_of(Picture {
        dynamic_zoom: None,
        ..shown.clone()
    });
    let z = shown.dynamic_zoom.unwrap_or_default();
    let scale = vec2(picture.width() / w as f32, picture.height() / h as f32);
    let whole = (fit[2] * f64::from(scale.x)).hypot(fit[3] * f64::from(scale.y)) / 2.0;
    let fixed = |b: ZoomBox| Picture {
        dynamic_zoom: Some(DynamicZoom {
            start: b,
            end: b,
            ..z
        }),
        ..shown.clone()
    };
    let ends = [level_of(fixed(z.start)), level_of(fixed(z.end))];
    Some(Shown {
        clip: c.id.clone(),
        anchor: anchored(&shown.sampled(c.content_at(at) as f64), (w, h), fit),
        picture: shown,
        level,
        middle,
        origin: picture.min,
        scale,
        whole,
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

pub(super) fn mode_label(m: CompositeMode) -> &'static str {
    use CompositeMode as M;
    match m {
        M::Normal => "Normal",
        M::Add => "Add",
        M::Subtract => "Subtract",
        M::Difference => "Difference",
        M::Multiply => "Multiply",
        M::Screen => "Screen",
        M::Overlay => "Overlay",
        M::Hardlight => "Hardlight",
        M::Softlight => "Softlight",
        M::Darken => "Darken",
        M::Lighten => "Lighten",
        M::ColorDodge => "Color Dodge",
        M::ColorBurn => "Color Burn",
        M::Exclusion => "Exclusion",
        M::Hue => "Hue",
        M::Saturation => "Saturation",
        M::Color => "Color",
        M::Luminosity => "Luminosity",
        M::Divide => "Divide",
        M::LinearDodge => "Linear Dodge",
        M::LinearBurn => "Linear Burn",
        M::LinearLight => "Linear Light",
        M::VividLight => "Vivid Light",
        M::PinLight => "Pin Light",
        M::HardMix => "Hard Mix",
        M::LighterColor => "Lighter Color",
        M::DarkerColor => "Darker Color",
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
    fn grab_wanted(&self) -> Option<(String, Picture, Picture)> {
        let g = &self.timeline.picture_grab;
        if let Some(h) = &g.area_held {
            return Some((h.clip.clone(), h.now.clone(), h.start.clone()));
        }
        if let Some(h) = &g.held {
            return Some((h.clip.clone(), h.now.clone(), h.start.clone()));
        }
        if let Some(w) = &g.wheel {
            return Some((w.clip.clone(), w.now.clone(), w.start.clone()));
        }
        let (clip, to, since, from) = g.landing.as_ref()?;
        let stored = self
            .project()?
            .clip(clip)?
            .picture
            .clone()
            .unwrap_or_default();
        let landed = landing(to, since.elapsed(), from, &stored)?;
        Some((clip.clone(), landed.clone(), from.clone()))
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
            let at = c.content_at(frame);
            (id.to_string(), stored.preview(baseline, picture, at))
        };
        match self.grab_wanted() {
            Some((id, picture, baseline)) => vec![preview(&id, &baseline, &picture)],
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
            .and_then(|p| p.clip(&s.clip).map(|c| c.content_at(at)));
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
            if zx.min(zy) > 0.0 {
                w.zoom = (w.zoom * k).clamp(ZOOM.0 / zx.min(zy), ZOOM.1 / zx.max(zy));
                w.now.zoom = Some([rounded(zx * w.zoom, 4), rounded(zy * w.zoom, 4)]);
            }
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
        let g = &mut self.timeline.picture_grab;
        g.tool = Some(tool);
        g.area = None;
        g.blur = false;
        g.drawn = None;
        self.select_shown();
    }

    fn select_shown(&mut self) {
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

    pub(super) fn blur_tool(&mut self, on: bool) {
        let g = &mut self.timeline.picture_grab;
        g.blur = on;
        g.drawn = None;
        g.blurred = None;
        g.area_held = None;
        if on {
            g.tool = None;
            g.row = true;
            self.select_shown();
        } else {
            g.area = None;
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
        let blurs = self.timeline.picture_grab.blurring();
        let b = egui::Button::selectable(blurs, "Blur");
        if ui.add_enabled(found || blurs, b).clicked() {
            self.blur_tool(!blurs);
        }
        if self.timeline.picture_grab.blurring() {
            ui.separator();
            self.blur_tools(ui, clip.as_ref());
            return;
        }
        let Some(tool) = self.timeline.picture_grab.tool else {
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

    fn blur_tools(&mut self, ui: &mut egui::Ui, clip: Option<&(String, Picture)>) {
        let now = self.timeline.picture_grab.shape;
        for (label, shape) in [
            ("Ellipse", AreaShape::Ellipse),
            ("Box", AreaShape::Box),
            ("Free Shape", AreaShape::Polygon),
        ] {
            if ui.selectable_label(now == shape, label).clicked() {
                let g = &mut self.timeline.picture_grab;
                g.shape = shape;
                g.drawn = None;
            }
        }
        ui.separator();
        for subject in Subject::ALL {
            let request = clip.and_then(|(id, _)| self.subject_blur(id, subject));
            let b = egui::Button::new(super::inspector::all_of(subject));
            if ui.add_enabled(request.is_some(), b).clicked()
                && let Some((method, params)) = request
            {
                self.find(method, params, subject);
            }
        }
        ui.separator();
        let chosen = clip
            .zip(self.timeline.picture_grab.area)
            .filter(|((_, pic), n)| {
                let effect = pic.effects.get(*n);
                effect.is_some_and(|e| e.area.is_some())
            });
        if ui
            .add_enabled(chosen.is_some(), egui::Button::new("Delete"))
            .clicked()
            && let Some(((id, _), n)) = chosen
        {
            self.timeline.picture_grab.area = None;
            self.send("effects.remove", json!({ "clip": id, "effect": n }));
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
                let at = |v: f64| rounded(v.clamp(0.5 - Picture::REACH, 0.5 + Picture::REACH), 4);
                next.now.position = Some([at(x), at(y)]);
                next.guides = guides;
            }
            Hold::Corner => {
                let (d0, d1) = (held.from.distance(pivot), to.distance(pivot));
                let d1 = f64::from(if d1 < 1.0 { 0.0 } else { d1 });
                let [zx, zy] = p0.scale();
                let most = zx.max(zy);
                let size = if most * s.whole <= f64::from(GRAB * 3.2) {
                    d1 / s.whole
                } else if d0 > 1.0 {
                    most * d1 / f64::from(d0)
                } else {
                    most
                };
                let size = size.min(ZOOM.1);
                let [rx, ry] = if most > 0.0 {
                    [zx / most, zy / most]
                } else {
                    [1.0, 1.0]
                };
                next.now.zoom = Some([rounded(rx * size, 4), rounded(ry * size, 4)]);
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
                let k = &mut next.now.crop;
                let side_of = |v: f64| rounded(v.clamp(0.0, 1.0), 4);
                match side {
                    Side::Left => k.left = side_of(u),
                    Side::Right => k.right = side_of(1.0 - u),
                    Side::Top => k.top = side_of(v),
                    Side::Bottom => k.bottom = side_of(1.0 - v),
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
                let at = |v: f64| rounded(v.clamp(-DynamicZoom::REACH, DynamicZoom::REACH), 4);
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
                    let size = (b.size * f64::from(d0 / d1.max(0.01)))
                        .clamp(DynamicZoom::SIZE.0, DynamicZoom::SIZE.1);
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
                .and_then(|p| p.clip(&s.clip).map(|c| c.content_at(at)));
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
    if a.mode() != b.mode() {
        params["composite_mode"] = json!(b.mode());
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

fn area_corners(a: &Area, [fw, fh]: [f64; 2]) -> [[f64; 2]; 4] {
    let (sin, cos) = a.rotation.to_radians().sin_cos();
    let (hx, hy) = (a.size[0] * fw / 2.0, a.size[1] * fh / 2.0);
    [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]].map(|[x, y]| {
        let (rx, ry) = (x * cos + y * sin, y * cos - x * sin);
        [a.center[0] + rx / fw, a.center[1] + ry / fh]
    })
}

impl Workspace {
    pub(super) fn paint_motion(&self, ui: &egui::Ui, picture: Rect) {
        if self.left != LeftTab::Inspector {
            return;
        }
        let Some(p) = self.project_shared() else {
            return;
        };
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let Some(c) = target(&p, &self.snap.selection.clips, at) else {
            return;
        };
        if c.motion.is_empty() {
            return;
        }
        let shown = c.picture.clone().unwrap_or_default();
        let Some(s) = shape_of(&p, c, shown, picture, at) else {
            return;
        };
        let frame = (f64::from(p.settings.width), f64::from(p.settings.height));
        let [_, _, fw, fh] = content(&p, c, frame);
        let painter = ui.painter_at(picture);
        let color = super::theme::colors().accent;
        let source = c.source_at(at) as f64;
        for m in &c.motion {
            let Some(area) = m.area_at(source) else {
                continue;
            };
            let corners = area_corners(&area, [fw, fh]).map(|uv| s.screen(uv));
            for i in 0..4 {
                let edge = [corners[i], corners[(i + 1) % 4]];
                painter.line_segment(edge, Stroke::new(1.5, color));
            }
            painter.text(
                corners[0] - vec2(0.0, 3.0),
                egui::Align2::LEFT_BOTTOM,
                m.name.as_deref().unwrap_or(&m.id),
                egui::FontId::proportional(12.0),
                color,
            );
        }
    }
}

const AREA_GRIP: f32 = 9.0;
const AREA_TURN: f32 = 24.0;
const AREA_ROUND: usize = 64;
const DRAW_LEAST: f32 = 4.0;
const DRAW_SAME: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum AreaHold {
    Move,
    Corner,
    Side(bool),
    Turn,
    Point(usize),
    Draw,
}

#[derive(Debug, Clone)]
pub(super) struct AreaHeld {
    clip: String,
    effect: usize,
    hold: AreaHold,
    from: Pos2,
    start: Picture,
    now: Picture,
}

#[derive(Debug, Clone, Copy)]
struct AreaBox {
    center: [f64; 2],
    half: [f64; 2],
    turn: f64,
    base: Area,
    fit: [f64; 2],
}

fn turned([x, y]: [f64; 2], degrees: f64) -> [f64; 2] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    [x * cos + y * sin, y * cos - x * sin]
}

impl AreaBox {
    const LEAST: f64 = 1e-6;

    fn of(a: &EffectArea, base: Area, [fw, fh]: [f64; 2]) -> Self {
        let [bw, bh] = [base.size[0] * fw, base.size[1] * fh];
        let [cx, cy] = a.middle();
        let [dx, dy] = turned([(cx - 0.5) * bw, (cy - 0.5) * bh], base.rotation);
        let [sx, sy] = a.extent();
        Self {
            center: [base.center[0] * fw + dx, base.center[1] * fh + dy],
            half: [sx * bw / 2.0, sy * bh / 2.0].map(|h| h.max(Self::LEAST)),
            turn: base.rotation + a.turn(),
            base,
            fit: [fw, fh],
        }
    }

    fn uv(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [dx, dy] = turned([x * self.half[0], y * self.half[1]], self.turn);
        [
            (self.center[0] + dx) / self.fit[0],
            (self.center[1] + dy) / self.fit[1],
        ]
    }

    fn local(&self, [u, v]: [f64; 2]) -> [f64; 2] {
        let off = [
            u * self.fit[0] - self.center[0],
            v * self.fit[1] - self.center[1],
        ];
        let [x, y] = turned(off, -self.turn);
        [x / self.half[0], y / self.half[1]]
    }

    fn written(&self, a: &EffectArea) -> EffectArea {
        let [bw, bh] = [
            (self.base.size[0] * self.fit[0]).max(Self::LEAST),
            (self.base.size[1] * self.fit[1]).max(Self::LEAST),
        ];
        let off = [
            self.center[0] - self.base.center[0] * self.fit[0],
            self.center[1] - self.base.center[1] * self.fit[1],
        ];
        let [dx, dy] = turned(off, -self.base.rotation);
        EffectArea {
            center: Some([0.5 + dx / bw, 0.5 + dy / bh]),
            size: Some([2.0 * self.half[0] / bw, 2.0 * self.half[1] / bh]),
            rotation: Some(self.turn - self.base.rotation),
            ..a.clone()
        }
    }
}

fn area_outline(a: &EffectArea) -> Vec<[f64; 2]> {
    match a.shape {
        AreaShape::Ellipse => (0..AREA_ROUND)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / AREA_ROUND as f64;
                [t.cos(), t.sin()]
            })
            .collect(),
        AreaShape::Box => vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
        AreaShape::Polygon => a
            .corners()
            .iter()
            .map(|[x, y]| [x * 2.0 - 1.0, y * 2.0 - 1.0])
            .collect(),
    }
}

fn area_holds(a: &EffectArea, [x, y]: [f64; 2]) -> bool {
    match a.shape {
        AreaShape::Ellipse => x.hypot(y) <= 1.0,
        AreaShape::Box => x.abs().max(y.abs()) <= 1.0,
        AreaShape::Polygon => {
            let points = area_outline(a);
            let mut inside = false;
            let mut b = points[points.len() - 1];
            for a in points {
                let crosses = (a[1] > y) != (b[1] > y);
                if crosses && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
                    inside = !inside;
                }
                b = a;
            }
            inside
        }
    }
}

fn near_side(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    let (e, w) = (b - a, p - a);
    let along = (w.dot(e) / e.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (w - e * along).length()
}

fn area_drawn(shape: AreaShape, a: [f64; 2], b: [f64; 2]) -> EffectArea {
    let middle = |i: usize| rounded((a[i] + b[i]) / 2.0, 4);
    let span = |i: usize| rounded((a[i] - b[i]).abs(), 4);
    EffectArea {
        shape,
        center: Some([middle(0), middle(1)]),
        size: Some([span(0), span(1)]),
        ..EffectArea::default()
    }
}

fn area_through(points: &[[f64; 2]]) -> Option<EffectArea> {
    let span = |i: usize| {
        let low = points.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min);
        let high = points
            .iter()
            .map(|p| p[i])
            .fold(f64::NEG_INFINITY, f64::max);
        (low, rounded(high - low, 4))
    };
    let ((x, w), (y, h)) = (span(0), span(1));
    if points.len() < 3 || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let inside = |p: &[f64; 2]| [rounded((p[0] - x) / w, 4), rounded((p[1] - y) / h, 4)];
    Some(EffectArea {
        shape: AreaShape::Polygon,
        center: Some([rounded(x + w / 2.0, 4), rounded(y + h / 2.0, 4)]),
        size: Some([w, h]),
        points: points.iter().map(inside).collect(),
        ..EffectArea::default()
    })
}

fn drawn_request(clip: &str, effect: usize, area: &EffectArea) -> Value {
    let mut change = json!({
        "shape": area.shape,
        "center": area.middle(),
        "size": area.extent(),
    });
    if !area.points.is_empty() {
        change["points"] = json!(area.points);
    }
    let add = json!({ "clips": [clip], "kind": EffectType::MosaicBlur });
    let set = json!({ "clip": clip, "effect": effect, "area": change });
    json!({
        "commands": [
            { "method": "effects.add", "params": add },
            { "method": "effects.set", "params": set },
        ],
        "label": "Blur Area",
    })
}

fn squared(from: Pos2, to: Pos2) -> Pos2 {
    let d = to - from;
    let side = d.x.abs().max(d.y.abs());
    from + vec2(side.copysign(d.x), side.copysign(d.y))
}

fn outline_on(s: &Shown, frame: &AreaBox, area: &EffectArea) -> Vec<Pos2> {
    area_outline(area)
        .into_iter()
        .map(|l| s.screen(frame.uv(l)))
        .collect()
}

fn paint_outline(painter: &egui::Painter, outline: &[Pos2], stroke: Stroke) {
    for i in 0..outline.len() {
        painter.line_segment([outline[i], outline[(i + 1) % outline.len()]], stroke);
    }
}

struct AreaShown {
    s: Shown,
    area: EffectArea,
    frame: AreaBox,
}

impl AreaShown {
    fn screen(&self, local: [f64; 2]) -> Pos2 {
        self.s.screen(self.frame.uv(local))
    }

    fn on(&self, p: Pos2) -> bool {
        self.grip(p).is_some() || (!self.sized() && self.side_at(p).is_some())
    }

    fn cursor(&self, hold: AreaHold, p: Pos2) -> CursorIcon {
        let middle = self.screen([0.0, 0.0]);
        match hold {
            AreaHold::Move => CursorIcon::Move,
            AreaHold::Draw => CursorIcon::Crosshair,
            AreaHold::Turn | AreaHold::Point(_) => CursorIcon::PointingHand,
            AreaHold::Corner => {
                let d = p - middle;
                if d.x * d.y > 0.0 {
                    CursorIcon::ResizeNwSe
                } else {
                    CursorIcon::ResizeNeSw
                }
            }
            AreaHold::Side(across) => {
                let side = if across { [1.0, 0.0] } else { [0.0, 1.0] };
                let n = self.screen(side) - middle;
                if n.x.abs() >= n.y.abs() {
                    CursorIcon::ResizeHorizontal
                } else {
                    CursorIcon::ResizeVertical
                }
            }
        }
    }

    fn local(&self, p: Pos2) -> [f64; 2] {
        self.frame.local(self.s.back(&self.s.level, p))
    }

    fn turn_at(&self) -> Pos2 {
        let (top, middle) = (self.screen([0.0, -1.0]), self.screen([0.0, 0.0]));
        top + (top - middle).normalized() * AREA_TURN
    }

    fn sized(&self) -> bool {
        self.area.shape != AreaShape::Polygon
    }

    fn grip(&self, p: Pos2) -> Option<AreaHold> {
        let near = |local: [f64; 2]| (self.screen(local) - p).length() <= AREA_GRIP;
        if self.sized() {
            if (self.turn_at() - p).length() <= AREA_GRIP {
                return Some(AreaHold::Turn);
            }
            let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
            if corners.into_iter().any(near) {
                return Some(AreaHold::Corner);
            }
            for (local, across) in [
                ([1.0, 0.0], true),
                ([-1.0, 0.0], true),
                ([0.0, 1.0], false),
                ([0.0, -1.0], false),
            ] {
                if near(local) {
                    return Some(AreaHold::Side(across));
                }
            }
        } else if let Some(i) = area_outline(&self.area).into_iter().position(near) {
            return Some(AreaHold::Point(i));
        }
        area_holds(&self.area, self.local(p)).then_some(AreaHold::Move)
    }

    fn side_at(&self, p: Pos2) -> Option<(usize, [f64; 2])> {
        let points = area_outline(&self.area);
        let n = points.len();
        let far = |i: usize| near_side(self.screen(points[i]), self.screen(points[(i + 1) % n]), p);
        let nearest = (0..n).min_by(|a, b| far(*a).total_cmp(&far(*b)))?;
        (far(nearest) <= AREA_GRIP).then(|| (nearest, self.local(p)))
    }
}

fn area_moved(held: &AreaHeld, shown: &AreaShown, to: Pos2) -> Option<EffectArea> {
    let start = &shown.area;
    let mut frame = shown.frame;
    let (from, now) = (shown.local(held.from), shown.local(to));
    match held.hold {
        AreaHold::Move => {
            let back = |p: Pos2| shown.s.back(&shown.s.level, p);
            let (a, b) = (back(held.from), back(to));
            frame.center[0] += (b[0] - a[0]) * frame.fit[0];
            frame.center[1] += (b[1] - a[1]) * frame.fit[1];
        }
        AreaHold::Corner => {
            let by = now[0].hypot(now[1]) / from[0].hypot(from[1]).max(AreaBox::LEAST);
            frame.half = frame.half.map(|h| (h * by).max(AreaBox::LEAST));
        }
        AreaHold::Side(across) => {
            let i = usize::from(!across);
            let side = from[i].signum();
            let by = (((now[i] - from[i]) * side + 2.0) / 2.0).max(AreaBox::LEAST);
            let mut shift = [0.0, 0.0];
            shift[i] = side * (by - 1.0) * frame.half[i];
            let [dx, dy] = turned(shift, frame.turn);
            frame.center = [frame.center[0] + dx, frame.center[1] + dy];
            frame.half[i] = (frame.half[i] * by).max(AreaBox::LEAST);
        }
        AreaHold::Turn => {
            let uv = shown.s.back(&shown.s.level, to);
            let off = [
                uv[0] * frame.fit[0] - frame.center[0],
                uv[1] * frame.fit[1] - frame.center[1],
            ];
            frame.turn = (-off[0]).atan2(-off[1]).to_degrees();
        }
        AreaHold::Point(i) => {
            let mut points = start.corners().to_vec();
            *points.get_mut(i)? = [(now[0] + 1.0) / 2.0, (now[1] + 1.0) / 2.0];
            return Some(EffectArea {
                points,
                ..start.clone()
            });
        }
        AreaHold::Draw => return None,
    }
    Some(frame.written(start))
}

fn area_change(from: &EffectArea, to: &EffectArea) -> Option<Value> {
    let mut change = json!({});
    if from.middle() != to.middle() {
        change["center"] = json!(to.middle());
    }
    if from.extent() != to.extent() {
        change["size"] = json!(to.extent());
    }
    if from.turn() != to.turn() {
        change["rotation"] = json!(to.turn());
    }
    if from.points != to.points {
        change["points"] = json!(to.points);
    }
    change
        .as_object()
        .is_some_and(|o| !o.is_empty())
        .then_some(change)
}

impl Workspace {
    pub(super) fn show_effect_area(&mut self) {
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(c) = target(&p, &self.snap.selection.clips, at) else {
            return;
        };
        let effects = c.picture.iter().flat_map(|x| &x.effects);
        let Some(n) = effects.into_iter().position(|e| e.area.is_some()) else {
            return;
        };
        self.timeline.picture_grab.area = Some(n);
        self.timeline.picture_grab.tool = None;
    }

    fn area_shown(&self, picture: Rect, held: Option<&AreaHeld>) -> Option<AreaShown> {
        self.area_at(picture, self.timeline.picture_grab.area?, held)
    }

    fn area_clip<'a>(&self, p: &'a Project, at: i64) -> Option<&'a Clip> {
        let chosen = target(p, &self.snap.selection.clips, at);
        if chosen.is_some() || !self.timeline.picture_grab.blurring() {
            return chosen;
        }
        p.clip(&topmost(p, at)?)
    }

    fn area_at(&self, picture: Rect, n: usize, held: Option<&AreaHeld>) -> Option<AreaShown> {
        let p = self.project_shared()?;
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let c = self.area_clip(&p, at)?;
        let shown = match held {
            Some(h) if h.clip == c.id => h.start.clone(),
            _ => c.picture.clone().unwrap_or_default(),
        };
        let area = shown.effects.get(n)?.area.clone()?;
        let source = c.source_at(at) as f64;
        let base = match area.follows.as_deref() {
            None => Area::WHOLE,
            Some(named) => {
                let followed = c.motion.iter().filter(|m| m.follows(named));
                followed.filter_map(|m| m.area_at(source)).next()?
            }
        };
        let frame = (f64::from(p.settings.width), f64::from(p.settings.height));
        let [_, _, fw, fh] = content(&p, c, frame);
        let s = shape_of(&p, c, shown, picture, at)?;
        Some(AreaShown {
            s,
            frame: AreaBox::of(&area, base, [fw, fh]),
            area,
        })
    }

    fn land_area(&mut self, held: &AreaHeld) {
        let area = |p: &Picture| p.effects.get(held.effect).and_then(|e| e.area.clone());
        let (Some(from), Some(to)) = (area(&held.start), area(&held.now)) else {
            return;
        };
        if let Some(change) = area_change(&from, &to) {
            let params = json!({ "clip": held.clip, "effect": held.effect, "area": change });
            self.send("effects.set", params);
            self.timeline.picture_grab.landing = Some((
                held.clip.clone(),
                held.now.clone(),
                Instant::now(),
                held.start.clone(),
            ));
        }
    }

    fn area_points(&mut self, ui: &egui::Ui, resp: &egui::Response, shown: &AreaShown) {
        if shown.sized() {
            return;
        }
        let Some(at) = ui.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        let n = self.timeline.picture_grab.area.unwrap_or(0);
        let mut points = shown.area.corners().to_vec();
        let held = shown.grip(at);
        let alt = resp.clicked() && ui.input(|i| i.modifiers.alt);
        if alt || resp.clicked_by(egui::PointerButton::Middle) {
            let Some(AreaHold::Point(i)) = held else {
                return;
            };
            if points.len() <= 3 {
                return;
            }
            points.remove(i);
        } else if resp.clicked() && !matches!(held, Some(AreaHold::Point(_))) {
            let Some((i, [x, y])) = shown.side_at(at) else {
                return;
            };
            if points.len() >= EffectArea::POINTS_KEPT {
                return;
            }
            points.insert(i + 1, [(x + 1.0) / 2.0, (y + 1.0) / 2.0]);
        } else {
            return;
        }
        let area = json!({ "points": points });
        let params = json!({ "clip": shown.s.clip, "effect": n, "area": area });
        self.send("effects.set", params);
    }

    fn land_drawn(&mut self, held: &AreaHeld) {
        let drawn = held.now.effects.get(held.effect);
        let Some(area) = drawn.and_then(|e| e.area.as_ref()) else {
            return;
        };
        self.send("apply", drawn_request(&held.clip, held.effect, area));
        let g = &mut self.timeline.picture_grab;
        g.area = Some(held.effect);
        g.landing = Some((
            held.clip.clone(),
            held.now.clone(),
            Instant::now(),
            held.start.clone(),
        ));
    }

    fn blur_drag(&mut self, ui: &egui::Ui, resp: &egui::Response, s: &Shown, fit: [f64; 2]) {
        let Some(mut held) = self.timeline.picture_grab.area_held.clone() else {
            return;
        };
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if let Some(to) = pointer.filter(|_| resp.dragged()) {
            let to = if ui.input(|i| i.modifiers.shift) {
                squared(held.from, to)
            } else {
                to
            };
            let d = to - held.from;
            held.now = held.start.clone();
            if d.x.abs().min(d.y.abs()) >= DRAW_LEAST {
                let ends = (s.back(&s.level, held.from), s.back(&s.level, to));
                let shape = self.timeline.picture_grab.shape;
                let mut effect = EffectType::MosaicBlur.made();
                effect.area = Some(area_drawn(shape, ends.0, ends.1));
                held.now.effects.push(effect);
            }
            self.timeline.picture_grab.area_held = Some(held.clone());
        }
        let drawn = held.now.effects.get(held.effect);
        if let Some(area) = drawn.and_then(|e| e.area.as_ref()) {
            let frame = AreaBox::of(area, Area::WHOLE, fit);
            let stroke = Stroke::new(1.5, super::theme::colors().accent);
            let painter = ui.painter_at(ui.clip_rect());
            paint_outline(&painter, &outline_on(s, &frame, area), stroke);
        }
        if resp.drag_stopped() {
            self.timeline.picture_grab.area_held = None;
            self.land_drawn(&held);
        } else if !resp.dragged() {
            self.timeline.picture_grab.area_held = None;
        }
    }

    fn blur_points(&mut self, ui: &egui::Ui, resp: &egui::Response, s: &Shown, effect: usize) {
        let Some((clip, mut points)) = self.timeline.picture_grab.drawn.clone() else {
            return;
        };
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.timeline.picture_grab.drawn = None;
            return;
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let at: Vec<Pos2> = points.iter().map(|uv| s.screen(*uv)).collect();
        let within = |q: Pos2, reach: f32| pointer.is_some_and(|p| p.distance(q) <= reach);
        let closes = points.len() >= 3 && within(at[0], AREA_GRIP);
        let mut done = resp.double_clicked() && points.len() >= 3;
        if resp.clicked()
            && let Some(p) = pointer
        {
            let same = at.last().is_some_and(|q| within(*q, DRAW_SAME));
            if closes {
                done = true;
            } else if !same && points.len() < EffectArea::POINTS_KEPT {
                points.push(s.back(&s.level, p));
                self.timeline.picture_grab.drawn = Some((clip.clone(), points.clone()));
            }
        }
        if done {
            self.timeline.picture_grab.drawn = None;
            if let Some(area) = area_through(&points) {
                self.send("apply", drawn_request(&clip, effect, &area));
                self.timeline.picture_grab.area = Some(effect);
            }
            return;
        }
        let accent = super::theme::colors().accent;
        let painter = ui.painter_at(ui.clip_rect());
        let at: Vec<Pos2> = points.iter().map(|uv| s.screen(*uv)).collect();
        for pair in at.windows(2) {
            painter.line_segment([pair[0], pair[1]], Stroke::new(1.5, accent));
        }
        if let (Some(last), Some(p)) = (at.last(), pointer) {
            let to = if closes { at[0] } else { p };
            painter.line_segment([*last, to], Stroke::new(1.0, accent));
        }
        for q in &at {
            painter.rect_filled(
                Rect::from_center_size(*q, Vec2::splat(GRAB * 1.4)),
                1.0,
                accent,
            );
        }
        if closes {
            painter.circle_stroke(at[0], GRAB * 1.6, Stroke::new(2.0, Color32::WHITE));
        }
    }

    fn blur_controls(&mut self, ui: &egui::Ui, resp: &egui::Response, picture: Rect) -> bool {
        if !self.timeline.picture_grab.blurring() {
            return false;
        }
        let at = self.snap.player(crate::live::PlayerId::Timeline).position;
        let Some(p) = self.project_shared() else {
            return false;
        };
        let clip = self.area_clip(&p, at);
        let g = &mut self.timeline.picture_grab;
        if g.drawn.as_ref().map(|(id, _)| id) != clip.map(|c| &c.id) {
            g.drawn = None;
        }
        if g.blurred.as_ref() != clip.map(|c| &c.id) {
            if g.blurred.is_some() {
                g.area = None;
                g.area_held = None;
            }
            g.blurred = clip.map(|c| c.id.clone());
        }
        let Some(c) = clip else {
            return false;
        };
        let stored = c.picture.clone().unwrap_or_default();
        let Some(s) = shape_of(&p, c, stored.clone(), picture, at) else {
            return false;
        };
        let frame = (f64::from(p.settings.width), f64::from(p.settings.height));
        let [_, _, fw, fh] = content(&p, c, frame);
        let drawing = |h: &AreaHeld| h.hold == AreaHold::Draw;
        if g.area_held.as_ref().is_some_and(drawing) {
            self.blur_drag(ui, resp, &s, [fw, fh]);
            return true;
        }
        if g.drawn.is_some() {
            self.blur_points(ui, resp, &s, stored.effects.len());
            return true;
        }
        if g.area_held.is_some() {
            return false;
        }
        let active = g.area;
        let others: Vec<(usize, AreaShown)> = (0..stored.effects.len())
            .rev()
            .filter(|n| Some(*n) != active)
            .filter_map(|n| self.area_at(picture, n, None).map(|a| (n, a)))
            .collect();
        let accent = super::theme::colors().accent;
        let painter = ui.painter_at(picture);
        for (_, a) in &others {
            let outline = outline_on(&a.s, &a.frame, &a.area);
            paint_outline(
                &painter,
                &outline,
                Stroke::new(1.0, accent.gamma_multiply(0.6)),
            );
        }
        let over = if resp.drag_started() {
            ui.input(|i| i.pointer.press_origin())
        } else {
            ui.input(|i| i.pointer.hover_pos())
                .filter(|_| resp.hovered())
        };
        let Some(over) = over.filter(|p| picture.contains(*p)) else {
            return false;
        };
        let pressed = resp.clicked() || resp.drag_started();
        if pressed && !self.snap.selection.clips.contains(&c.id) {
            self.send("selection.set", json!({ "clips": [c.id] }));
        }
        let shown = active.and_then(|n| self.area_at(picture, n, None));
        if shown.is_some_and(|a| a.on(over)) {
            return false;
        }
        if let Some((n, _)) = others.iter().find(|(_, a)| a.on(over)) {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            if resp.drag_started() {
                self.timeline.picture_grab.area = Some(*n);
                return false;
            }
            if resp.clicked() {
                self.timeline.picture_grab.area = Some(*n);
            }
            return true;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        let g = &mut self.timeline.picture_grab;
        if g.shape == AreaShape::Polygon {
            if resp.clicked() {
                g.drawn = Some((c.id.clone(), vec![s.back(&s.level, over)]));
            }
        } else if resp.drag_started() {
            g.area_held = Some(AreaHeld {
                clip: c.id.clone(),
                effect: stored.effects.len(),
                hold: AreaHold::Draw,
                from: over,
                now: stored.clone(),
                start: stored,
            });
        }
        true
    }

    pub(super) fn area_controls(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        picture: Rect,
    ) -> bool {
        let drawing = self.blur_controls(ui, resp, picture);
        let editing = |h: &AreaHeld| h.hold != AreaHold::Draw;
        let held = self.timeline.picture_grab.area_held.clone();
        let held = held.filter(editing);
        let Some(shown) = self.area_shown(picture, held.as_ref()) else {
            if held.is_some() {
                self.timeline.picture_grab.area_held = None;
            }
            if drawing {
                self.sync_picture_preview();
            }
            return drawing;
        };
        let Some(n) = self.timeline.picture_grab.area else {
            return drawing;
        };
        if drawing {
            self.sync_picture_preview();
            paint_area(&ui.painter_at(picture), &shown, None, None);
            return true;
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let over = pointer.filter(|_| resp.hovered());
        let hover = over.and_then(|p| shown.grip(p));
        let point = matches!(hover, Some(AreaHold::Point(_)));
        let adds = over
            .filter(|_| held.is_none() && !point && !shown.sized())
            .filter(|p| shown.side_at(*p).is_some());
        if adds.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        } else if let (Some(hold), Some(p)) = (held.as_ref().map(|h| h.hold).or(hover), pointer) {
            ui.ctx().set_cursor_icon(shown.cursor(hold, p));
        }
        if resp.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
            && let Some(hold) = shown.grip(press)
        {
            let start = shown.s.picture.clone();
            self.timeline.picture_grab.area_held = Some(AreaHeld {
                clip: shown.s.clip.clone(),
                effect: n,
                hold,
                from: press,
                now: start.clone(),
                start,
            });
        }
        if let (Some(mut held), Some(to)) = (self.timeline.picture_grab.area_held.clone(), pointer)
            && resp.dragged()
            && let Some(area) = area_moved(&held, &shown, to)
            && let Some(e) = held.now.effects.get_mut(held.effect)
        {
            e.area = Some(area);
            self.timeline.picture_grab.area_held = Some(held);
        }
        let mut busy = held.is_some() || hover.is_some();
        if resp.drag_stopped()
            && let Some(held) = self.timeline.picture_grab.area_held.take()
        {
            busy = true;
            self.land_area(&held);
        }
        self.area_points(ui, resp, &shown);
        self.sync_picture_preview();
        let held = self.timeline.picture_grab.area_held.clone();
        let moved = held
            .as_ref()
            .and_then(|h| h.now.effects.get(h.effect))
            .and_then(|e| e.area.clone());
        let drawn = match moved {
            Some(area) => AreaShown {
                frame: AreaBox::of(&area, shown.frame.base, shown.frame.fit),
                area,
                s: shown.s,
            },
            None => shown,
        };
        let hot = pointer.filter(|_| held.is_some() || hover.is_some());
        paint_area(&ui.painter_at(picture), &drawn, hot, adds);
        busy || adds.is_some()
    }
}

fn paint_area(painter: &egui::Painter, shown: &AreaShown, hot: Option<Pos2>, adds: Option<Pos2>) {
    let accent = super::theme::colors().accent;
    let stroke = Stroke::new(1.5, accent);
    let outline = outline_on(&shown.s, &shown.frame, &shown.area);
    paint_outline(painter, &outline, stroke);
    let lit = |at: Pos2| hot.is_some_and(|p| p.distance(at) <= AREA_GRIP);
    let ring = |at: Pos2, reach: f32| {
        if lit(at) {
            painter.circle_stroke(at, reach + 3.0, Stroke::new(2.0, Color32::WHITE));
        }
    };
    let square = |at: Pos2| {
        painter.rect_filled(
            Rect::from_center_size(at, Vec2::splat(GRAB * 1.4)),
            1.0,
            accent,
        );
        ring(at, GRAB);
    };
    if let Some(at) = adds {
        painter.circle_filled(at, GRAB * 0.7, Color32::WHITE);
        painter.circle_stroke(at, GRAB * 0.7, stroke);
    }
    if !shown.sized() {
        outline.into_iter().for_each(square);
        return;
    }
    for corner in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
        square(shown.screen(corner));
    }
    for side in [[1.0, 0.0], [-1.0, 0.0], [0.0, 1.0], [0.0, -1.0]] {
        painter.circle_filled(shown.screen(side), GRAB * 0.7, accent);
        ring(shown.screen(side), GRAB * 0.7);
    }
    painter.line_segment([shown.screen([0.0, -1.0]), shown.turn_at()], stroke);
    painter.circle_filled(shown.turn_at(), GRAB * 0.9, accent);
    ring(shown.turn_at(), GRAB * 0.9);
}
