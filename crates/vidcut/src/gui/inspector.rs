use std::ops::RangeInclusive;
use std::time::Instant;

use eframe::egui::{self, Align, Layout, RichText, vec2};
use egui_phosphor::regular::{
    ARROW_COUNTER_CLOCKWISE, CARET_LEFT, CARET_RIGHT, DIAMOND, FLIP_HORIZONTAL, FLIP_VERTICAL,
    LINK_SIMPLE, LINK_SIMPLE_BREAK,
};
use serde_json::{Value, json};
use vidcut_core::keyframes::PictureValue;
use vidcut_core::model::{
    Clip, ClipRetime, CompositeMode, DynamicZoom, Ease, Keyframe, Picture, PictureTool, Project,
    Retime, Speed, TrackKind, Upscaler, ZoomBox,
};
use vidcut_core::ops::picture::{cleared, has_picture};
use vidcut_core::place::upright_fit;
use vidcut_core::time::TimeSpec;
use vidcut_core::validate::GAIN;

use super::dialog;
use super::isolation::{Isolate, heard};
use super::placing::{changed, ease_label, mode_label, target};
use super::theme;
use super::timeline::SpeedDialog;
use super::timeline::tools::landing;
use super::viewer::timecode_input;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

const REACH: f64 = 100.0;
const ZOOM: (f64, f64) = (0.01, 100.0);
const BOX: (f64, f64) = (0.01, 100.0);
const TURNS: f64 = 3600.0;
const LEAN: f64 = 1.5;
const SOFT: f64 = 100.0;
const LEFT_SHOWING: f64 = 1.0;
const SAME: f64 = 1e-9;
const FIELD_WIDTH: f32 = 76.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Field {
    Zoom,
    ZoomX,
    ZoomY,
    X,
    Y,
    Rotation,
    AnchorX,
    AnchorY,
    Pitch,
    Yaw,
    Left,
    Right,
    Top,
    Bottom,
    Softness,
    Opacity,
    StartZoom,
    StartX,
    StartY,
    EndZoom,
    EndX,
    EndY,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Picture(Field),
    Volume,
    Speed,
    Rate,
    Length,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Reverse,
    Freeze,
}

impl Direction {
    const ALL: [Direction; 3] = [Direction::Forward, Direction::Reverse, Direction::Freeze];

    fn of(speed: f64) -> Self {
        if speed == 0.0 {
            Direction::Freeze
        } else if speed < 0.0 {
            Direction::Reverse
        } else {
            Direction::Forward
        }
    }

    fn label(self) -> &'static str {
        match self {
            Direction::Forward => "Forward",
            Direction::Reverse => "Reverse",
            Direction::Freeze => "Freeze",
        }
    }

    fn turn(self, d: &mut SpeedDialog) {
        d.freeze = self == Direction::Freeze;
        if !d.freeze {
            d.reverse = self == Direction::Reverse;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Shape {
    pub(super) frame: [f64; 2],
    pub(super) fitted: [f64; 2],
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Zoom => "Zoom",
            Field::ZoomX => "Zoom X",
            Field::ZoomY => "Zoom Y",
            Field::X => "Position X",
            Field::Y => "Position Y",
            Field::Rotation => "Rotation",
            Field::AnchorX => "Anchor X",
            Field::AnchorY => "Anchor Y",
            Field::Pitch => "Pitch",
            Field::Yaw => "Yaw",
            Field::Left => "Left",
            Field::Right => "Right",
            Field::Top => "Top",
            Field::Bottom => "Bottom",
            Field::Softness => "Softness",
            Field::Opacity => "Opacity",
            Field::StartZoom => "Start Zoom",
            Field::StartX => "Start X",
            Field::StartY => "Start Y",
            Field::EndZoom => "End Zoom",
            Field::EndX => "End X",
            Field::EndY => "End Y",
        }
    }

    fn starts(self) -> Option<bool> {
        match self {
            Field::StartZoom | Field::StartX | Field::StartY => Some(true),
            Field::EndZoom | Field::EndX | Field::EndY => Some(false),
            _ => None,
        }
    }

    fn zoom_box(self, z: &DynamicZoom) -> ZoomBox {
        if self.starts() == Some(false) {
            z.end
        } else {
            z.start
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Field::Zoom
            | Field::ZoomX
            | Field::ZoomY
            | Field::StartZoom
            | Field::EndZoom
            | Field::Opacity => "%",
            Field::Rotation => "°",
            _ => "",
        }
    }

    fn speed(self) -> f64 {
        match self {
            Field::Zoom
            | Field::ZoomX
            | Field::ZoomY
            | Field::Rotation
            | Field::StartZoom
            | Field::EndZoom
            | Field::Softness
            | Field::Opacity => 0.5,
            Field::Pitch | Field::Yaw => 0.01,
            _ => 1.0,
        }
    }

    fn usual(self) -> f64 {
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY | Field::EndZoom | Field::Opacity => 100.0,
            Field::StartZoom => 100.0 / DynamicZoom::default().start.size,
            _ => 0.0,
        }
    }

    fn channel(self) -> Option<PictureValue> {
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY => Some(PictureValue::Zoom),
            Field::X | Field::Y => Some(PictureValue::Position),
            Field::Rotation => Some(PictureValue::Rotation),
            Field::AnchorX | Field::AnchorY => Some(PictureValue::Anchor),
            Field::Pitch => Some(PictureValue::Pitch),
            Field::Yaw => Some(PictureValue::Yaw),
            Field::Left => Some(PictureValue::CropLeft),
            Field::Right => Some(PictureValue::CropRight),
            Field::Top => Some(PictureValue::CropTop),
            Field::Bottom => Some(PictureValue::CropBottom),
            Field::Softness => Some(PictureValue::CropSoftness),
            Field::Opacity => Some(PictureValue::Opacity),
            _ => None,
        }
    }

    fn read(self, p: &Picture, s: &Shape, frame: Option<i64>) -> f64 {
        let owned = frame
            .filter(|_| self.channel().is_some_and(|v| p.has_keys(v)))
            .map(|f| p.sampled(f as f64));
        let p = owned.as_ref().unwrap_or(p);
        let ([x, y], [zx, zy], c) = (p.place(), p.scale(), p.crop);
        let ([w, h], [fw, fh], [ax, ay]) = (s.frame, s.fitted, p.pivot());
        let b = self.zoom_box(&p.dynamic_zoom.unwrap_or_default());
        match self {
            Field::Zoom | Field::ZoomX => zx * 100.0,
            Field::ZoomY => zy * 100.0,
            Field::X => (x - 0.5) * w,
            Field::Y => (0.5 - y) * h,
            Field::Rotation => p.turn(),
            Field::AnchorX => (ax - 0.5) * fw,
            Field::AnchorY => (0.5 - ay) * fh,
            Field::Pitch => p.pitch(),
            Field::Yaw => p.yaw(),
            Field::Left => c.left * fw,
            Field::Right => c.right * fw,
            Field::Top => c.top * fh,
            Field::Bottom => c.bottom * fh,
            Field::Softness => p.softness(),
            Field::Opacity => p.opaque() * 100.0,
            Field::StartZoom | Field::EndZoom => 100.0 / b.size,
            Field::StartX | Field::EndX => (0.5 - b.center[0]) / b.size * fw,
            Field::StartY | Field::EndY => (b.center[1] - 0.5) / b.size * fh,
        }
    }

    fn is_usual(self, p: &Picture) -> bool {
        let ([x, y], [zx, zy], [ax, ay], c) = (p.place(), p.scale(), p.pivot(), p.crop);
        let usual = self.zoom_box(&DynamicZoom::default());
        let b = p.dynamic_zoom.map_or(usual, |z| self.zoom_box(&z));
        match self {
            Field::Zoom => zx == 1.0 && zy == 1.0,
            Field::ZoomX => zx == 1.0,
            Field::ZoomY => zy == 1.0,
            Field::X => x == 0.5,
            Field::Y => y == 0.5,
            Field::Rotation => p.turn() == 0.0,
            Field::AnchorX => ax == 0.5,
            Field::AnchorY => ay == 0.5,
            Field::Pitch => p.pitch() == 0.0,
            Field::Yaw => p.yaw() == 0.0,
            Field::Left => c.left == 0.0,
            Field::Right => c.right == 0.0,
            Field::Top => c.top == 0.0,
            Field::Bottom => c.bottom == 0.0,
            Field::Softness => p.softness() == 0.0,
            Field::Opacity => p.opaque() == 1.0,
            Field::StartZoom | Field::EndZoom => b.size == usual.size,
            Field::StartX | Field::EndX => b.center[0] == usual.center[0],
            Field::StartY | Field::EndY => b.center[1] == usual.center[1],
        }
    }

    fn range(self, p: &Picture, s: &Shape) -> RangeInclusive<f64> {
        let ([w, h], [fw, fh], c) = (s.frame, s.fitted, p.crop);
        let side = |other: f64, span: f64| 0.0..=((1.0 - other) * span - LEFT_SHOWING).max(0.0);
        match self {
            Field::Zoom | Field::ZoomX | Field::ZoomY => ZOOM.0 * 100.0..=ZOOM.1 * 100.0,
            Field::X => (-REACH - 0.5) * w..=(REACH - 0.5) * w,
            Field::Y => (0.5 - REACH) * h..=(0.5 + REACH) * h,
            Field::Rotation => -TURNS..=TURNS,
            Field::AnchorX => (-REACH - 0.5) * fw..=(REACH - 0.5) * fw,
            Field::AnchorY => (0.5 - REACH) * fh..=(0.5 + REACH) * fh,
            Field::Pitch | Field::Yaw => -LEAN..=LEAN,
            Field::Left => side(c.right, fw),
            Field::Right => side(c.left, fw),
            Field::Top => side(c.bottom, fh),
            Field::Bottom => side(c.top, fh),
            Field::Softness => -SOFT..=SOFT,
            Field::Opacity => 0.0..=100.0,
            Field::StartZoom | Field::EndZoom => 100.0 / BOX.1..=100.0 / BOX.0,
            Field::StartX | Field::EndX => -REACH * fw / BOX.0..=REACH * fw / BOX.0,
            Field::StartY | Field::EndY => -REACH * fh / BOX.0..=REACH * fh / BOX.0,
        }
    }

    fn put_at(self, p: &Picture, s: &Shape, v: f64, frame: Option<i64>) -> Picture {
        let r = self.range(p, s);
        let v = v.clamp(*r.start(), *r.end());
        if (self.read(p, s, frame) - v).abs() < SAME {
            return p.clone();
        }
        let at = frame.unwrap_or(0) as f64;
        let place = frame
            .filter(|_| p.has_keys(PictureValue::Position))
            .map(|_| p.place_at(at))
            .unwrap_or_else(|| p.place());
        let scale = frame
            .filter(|_| p.has_keys(PictureValue::Zoom))
            .map(|_| p.scale_at(at))
            .unwrap_or_else(|| p.scale());
        let pivot = frame
            .filter(|_| p.has_keys(PictureValue::Anchor))
            .map(|_| p.pivot_at(at))
            .unwrap_or_else(|| p.pivot());
        let ([x, y], [zx, zy], [ax, ay]) = (place, scale, pivot);
        let ([w, h], [fw, fh]) = (s.frame, s.fitted);
        let zoom = |v: f64| (v / 100.0).clamp(ZOOM.0, ZOOM.1);
        let at = |v: f64| v.clamp(-REACH, REACH);
        let mut out = p.clone();
        match self {
            Field::Zoom => out.zoom = Some([zoom(v); 2]),
            Field::ZoomX => out.zoom = Some([zoom(v), zy]),
            Field::ZoomY => out.zoom = Some([zx, zoom(v)]),
            Field::X => out.position = Some([at(0.5 + v / w), y]),
            Field::Y => out.position = Some([x, at(0.5 - v / h)]),
            Field::Rotation => out.rotation = Some(v),
            Field::AnchorX => out.anchor = Some([at(0.5 + v / fw), ay]),
            Field::AnchorY => out.anchor = Some([ax, at(0.5 - v / fh)]),
            Field::Pitch => out.pitch = Some(v),
            Field::Yaw => out.yaw = Some(v),
            Field::Left => out.crop.left = v / fw,
            Field::Right => out.crop.right = v / fw,
            Field::Top => out.crop.top = v / fh,
            Field::Bottom => out.crop.bottom = v / fh,
            Field::Softness => out.crop_softness = Some(v),
            Field::Opacity => out.opacity = Some(v / 100.0),
            Field::StartZoom
            | Field::StartX
            | Field::StartY
            | Field::EndZoom
            | Field::EndX
            | Field::EndY => {
                let mut z = p.dynamic_zoom.unwrap_or_default();
                let b = if self.starts() == Some(true) {
                    &mut z.start
                } else {
                    &mut z.end
                };
                match self {
                    Field::StartZoom | Field::EndZoom => {
                        let size = (100.0 / v).clamp(BOX.0, BOX.1);
                        let keep = |c: f64| at(0.5 - (0.5 - c) / b.size * size);
                        b.center = [keep(b.center[0]), keep(b.center[1])];
                        b.size = size;
                    }
                    Field::StartX | Field::EndX => b.center[0] = at(0.5 - v / fw * b.size),
                    _ => b.center[1] = at(0.5 + v / fh * b.size),
                }
                out.dynamic_zoom = Some(z);
            }
        }
        out
    }
}

impl Key {
    fn label(self) -> &'static str {
        match self {
            Key::Picture(f) => f.label(),
            Key::Volume => "Volume",
            Key::Speed => "Speed",
            Key::Rate => "Frames per Second",
            Key::Length => "Duration",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Key::Picture(f) => f.suffix(),
            Key::Volume => " dB",
            Key::Speed => "%",
            Key::Rate => " fps",
            Key::Length => "",
        }
    }

    fn speed(self) -> f64 {
        match self {
            Key::Picture(f) => f.speed(),
            Key::Volume => 0.1,
            Key::Speed | Key::Length => 1.0,
            Key::Rate => 0.1,
        }
    }

    fn decimals(self) -> (usize, usize) {
        match self {
            Key::Picture(Field::Pitch | Field::Yaw) => (0, 3),
            Key::Picture(_) => (0, 2),
            Key::Volume => (1, 1),
            Key::Speed => (0, 2),
            Key::Rate => (0, 3),
            Key::Length => (0, 0),
        }
    }

    fn usual(self) -> f64 {
        match self {
            Key::Picture(f) => f.usual(),
            Key::Volume => 0.0,
            Key::Speed => 100.0,
            Key::Rate | Key::Length => 0.0,
        }
    }
}

fn typed(text: &str) -> Option<f64> {
    let t = text
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '%' | '°'))
        .map(|c| if c == '−' { '-' } else { c })
        .collect::<String>()
        .to_ascii_lowercase();
    t.strip_suffix("db")
        .or_else(|| t.strip_suffix("fps"))
        .unwrap_or(&t)
        .parse()
        .ok()
}

fn frames(p: &Project, text: &str, dropped: i64) -> Option<f64> {
    let spec = TimeSpec::parse(&timecode_input(text, dropped)?).ok()?;
    let n = p.time_context(None).duration(&spec).ok()?;
    Some(n as f64)
}

fn rate(p: &Project, c: &Clip) -> f64 {
    p.media(&c.media)
        .and_then(|m| m.info.video.as_ref())
        .filter(|v| !v.still)
        .map_or(p.settings.fps, |v| v.fps)
        .as_f64()
}

pub(super) fn shape(p: &Project, c: &Clip) -> Shape {
    let (w, h) = (p.settings.width.max(1), p.settings.height.max(1));
    let frame = [f64::from(w), f64::from(h)];
    let fitted = p
        .media(&c.media)
        .and_then(|m| m.info.video.as_ref())
        .filter(|v| v.width > 0 && v.height > 0)
        .map_or(frame, |v| {
            let sar = (v.sar[0], v.sar[1]);
            let (fw, fh) = upright_fit((v.width, v.height), sar, v.rotation, (w, h));
            [f64::from(fw.max(1)), f64::from(fh.max(1))]
        });
    Shape { frame, fitted }
}

fn name(p: &Project, c: &Clip) -> String {
    match &c.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => p
            .media(&c.media)
            .map_or_else(|| c.id.clone(), |m| m.name.clone()),
    }
}

fn uses(p: &Picture, tool: PictureTool) -> bool {
    cleared(p, tool) != *p
}

fn merged(params: &[Value]) -> Option<Value> {
    let rest = |v: &Value| {
        let mut v = v.clone();
        v.as_object_mut()?.remove("clips");
        Some(v)
    };
    let first = rest(params.first()?)?;
    params
        .iter()
        .all(|p| rest(p).as_ref() == Some(&first))
        .then(|| {
            let mut one = first.clone();
            one["clips"] = json!(
                params
                    .iter()
                    .map(|p| p["clips"].clone())
                    .collect::<Vec<_>>()
            );
            one
        })
}

#[derive(Debug, Clone)]
struct Chosen {
    id: String,
    stored: Picture,
    shown: Picture,
    shape: Shape,
    frame: i64,
}

#[derive(Debug, Clone)]
struct Sound {
    id: String,
    stored: f64,
    gain: f64,
    muted: bool,
    keyed: bool,
}

type Lists<'a> = (&'a [Chosen], &'a [Sound], &'a [Paced]);

type Pace = (f64, i64);

#[derive(Debug, Clone)]
struct Paced {
    id: String,
    stored: Pace,
    shown: Pace,
    rate: f64,
    retime: ClipRetime,
    project: Retime,
    hdr: bool,
    at: i64,
    ramped: bool,
}

#[derive(Debug, Clone)]
struct Retimed {
    since: Instant,
    clips: Vec<(String, Pace, Pace)>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Look<'a> {
    off: bool,
    why: Option<&'static str>,
    timecode: Option<(&'a Project, i64)>,
}

#[derive(Debug, Clone)]
struct Held {
    field: Key,
    value: f64,
    from: f64,
    start: Vec<Chosen>,
    dragging: bool,
}

#[derive(Debug, Clone)]
struct Landing {
    since: Instant,
    pictures: Vec<(String, Picture, Picture)>,
}

#[derive(Debug, Clone)]
struct Gained {
    since: Instant,
    from: Vec<(String, f64)>,
    to: f64,
}

#[derive(Debug, Default)]
pub struct Inspector {
    unlinked: bool,
    held: Option<Held>,
    landing: Option<Landing>,
    gained: Option<Gained>,
    retimed: Option<Retimed>,
    ripple: Option<bool>,
    seen: Option<(Vec<String>, Vec<String>)>,
    focus: Option<egui::Id>,
    moved: bool,
    commit: bool,
}

enum Act {
    Hold(Key, f64),
    Drag(Key, f64),
    Land(Key),
    Put(Key, f64),
    Edit(Key, f64),
    Release(Key),
    Direction(Direction),
    SpeedEase(Ease),
    Ripple(bool),
    FullSpeed,
    Retime(ClipRetime),
    Mute(bool),
    Link(bool),
    Flips([Option<bool>; 2]),
    Zooming(bool),
    Boxes,
    Ease(Ease),
    Mode(CompositeMode),
    Swap,
    Clear(PictureTool),
    Unkey,
    Step(Key, i64),
    Toggle(Key),
    KeyEase(Key, Ease),
    ResetChannel(Field),
}

fn param_name(key: Key) -> Option<&'static str> {
    match key {
        Key::Volume => Some("volume"),
        Key::Picture(f) => f.channel().map(PictureValue::name),
        Key::Speed => Some("speed"),
        _ => None,
    }
}

fn key_clips(key: Key, (chosen, sounds, paced): Lists<'_>) -> Vec<String> {
    match key {
        Key::Volume => sounds.iter().map(|s| s.id.clone()).collect(),
        Key::Picture(_) => chosen.iter().map(|c| c.id.clone()).collect(),
        Key::Speed => paced.iter().map(|c| c.id.clone()).collect(),
        _ => Vec::new(),
    }
}

fn key_times(c: &Clip, key: Key) -> Vec<i64> {
    let keys = match key {
        Key::Volume => c.audio.as_ref().map(|a| a.gain_keys.clone()),
        Key::Picture(f) => f
            .channel()
            .and_then(|v| c.picture.as_ref().map(|pic| pic.keys_of(v).to_vec())),
        Key::Speed => c.speed.as_ref().map(Speed::keyframes),
        _ => None,
    };
    keys.unwrap_or_default()
        .iter()
        .map(|k| c.key_time(k.frame).round() as i64)
        .collect()
}

fn label(ui: &mut egui::Ui, text: &str, set: bool) {
    let c = theme::colors();
    ui.label(RichText::new(text).color(if set { c.accent } else { c.text_dim }));
}

fn reset(ui: &mut egui::Ui, shown: bool) -> bool {
    let r = ui.add_visible(
        shown,
        egui::Button::new(ARROW_COUNTER_CLOCKWISE).frame(false),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, shown, "Reset"));
    r.clicked()
}

fn number(
    ui: &mut egui::Ui,
    (f, link): (Field, Option<bool>),
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let first = &chosen[0];
    let set = chosen
        .iter()
        .any(|c| !f.is_usual(&c.stored) || f.channel().is_some_and(|v| c.stored.has_keys(v)));
    let shown = f.read(&first.shown, &first.shape, Some(first.frame));
    let range = f.range(&first.shown, &first.shape);
    let keys = f.channel().and_then(|v| {
        let c = p.clip(&first.id)?;
        let picture = c.picture.clone().unwrap_or_default();
        Some(mark(c, picture.keys_of(v), at))
    });
    entry(
        ui,
        (Key::Picture(f), link),
        i,
        (set, shown, range),
        keys,
        acts,
    );
}

struct KeyMark {
    on: bool,
    prev: bool,
    next: bool,
}

fn mark(c: &Clip, keys: &[Keyframe], at: i64) -> KeyMark {
    let time = |k: &Keyframe| c.key_time(k.frame).round() as i64;
    let inside = |t: i64| c.start <= t && t < c.end();
    KeyMark {
        on: keys.iter().any(|k| time(k) == at),
        prev: keys.iter().any(|k| {
            let t = time(k);
            inside(t) && t < at
        }),
        next: keys.iter().any(|k| {
            let t = time(k);
            inside(t) && t > at
        }),
    }
}

fn entry(
    ui: &mut egui::Ui,
    key: (Key, Option<bool>),
    i: &Inspector,
    values: (bool, f64, RangeInclusive<f64>),
    keys: Option<KeyMark>,
    acts: &mut Vec<Act>,
) {
    field(ui, key, i, values, Look::default(), keys, acts);
}

fn field(
    ui: &mut egui::Ui,
    (f, link): (Key, Option<bool>),
    i: &Inspector,
    (set, shown, range): (bool, f64, RangeInclusive<f64>),
    look: Look<'_>,
    keys: Option<KeyMark>,
    acts: &mut Vec<Act>,
) {
    label(ui, f.label(), set);
    let held = i.held.as_ref().filter(|h| h.field == f);
    let mut v = held.map_or(shown, |h| h.value);
    let before = v;
    let (fewest, most) = f.decimals();
    ui.horizontal(|ui| {
        if look.off {
            ui.disable();
        }
        let r = ui
            .push_id(f, |ui| {
                let mut drag = egui::DragValue::new(&mut v)
                    .update_while_editing(false)
                    .clamp_existing_to_range(false)
                    .range(range)
                    .speed(f.speed())
                    .min_decimals(fewest)
                    .max_decimals(most)
                    .suffix(f.suffix())
                    .custom_parser(typed)
                    .min_size(vec2(FIELD_WIDTH, ui.spacing().interact_size.y));
                if let Some((p, dropped)) = look.timecode {
                    drag = drag
                        .custom_formatter(|n, _| p.settings.length_timecode(n.round() as i64))
                        .custom_parser(move |t| frames(p, t, dropped));
                }
                ui.add(drag)
            })
            .inner;
        let r = match look.why {
            Some(why) => r.on_disabled_hover_text(why),
            None => r,
        };
        if r.drag_started() {
            acts.push(Act::Hold(f, before));
        }
        if r.dragged() && r.changed() {
            acts.push(Act::Drag(f, v));
        }
        if r.drag_stopped() || (held.is_some_and(|h| h.dragging) && !r.dragged()) {
            acts.push(Act::Land(f));
        } else if r.changed() && !r.dragged() {
            acts.push(Act::Put(f, v));
        } else if r.has_focus() {
            acts.push(Act::Edit(f, v));
        } else if held.is_some() && !r.dragged() {
            acts.push(Act::Release(f));
        }
        if let Some(keys) = keys {
            let prev = ui.add_enabled(keys.prev, egui::Button::new(CARET_LEFT).frame(false));
            if prev.clicked() {
                acts.push(Act::Step(f, -1));
            }
            let diamond = ui.add(egui::Button::selectable(keys.on, DIAMOND).frame(false));
            if diamond.clicked() {
                acts.push(Act::Toggle(f));
            }
            let linear = matches!(f, Key::Picture(Field::AnchorX | Field::AnchorY));
            if keys.on && !linear {
                diamond.context_menu(|ui| {
                    for e in Ease::ALL {
                        if ui.button(ease_label(e)).clicked() {
                            acts.push(Act::KeyEase(f, e));
                        }
                    }
                });
            }
            let next = ui.add_enabled(keys.next, egui::Button::new(CARET_RIGHT).frame(false));
            if next.clicked() {
                acts.push(Act::Step(f, 1));
            }
        }
        if let Some(lit) = link {
            let icon = if lit { LINK_SIMPLE } else { LINK_SIMPLE_BREAK };
            let b = ui.add(egui::Button::selectable(lit, icon));
            b.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, lit, "Link Zoom")
            });
            if b.clicked() {
                acts.push(Act::Link(!lit));
            }
        }
    });
    if reset(ui, set) {
        acts.push(match f {
            Key::Volume => Act::Unkey,
            Key::Picture(field) if field.channel().is_some() => Act::ResetChannel(field),
            f => Act::Put(f, f.usual()),
        });
    }
    ui.end_row();
}

fn group<R>(
    ui: &mut egui::Ui,
    name: &str,
    header: impl FnOnce(&mut egui::Ui) -> R,
    rows: impl FnOnce(&mut egui::Ui),
) -> R {
    let id = ui.make_persistent_id(("inspector", name));
    let (_, header, _) =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.label(RichText::new(name).strong());
                header(ui)
            })
            .body(|ui| {
                egui::Grid::new(("inspector-rows", name))
                    .num_columns(3)
                    .min_col_width(84.0)
                    .spacing([8.0, 6.0])
                    .show(ui, rows);
            });
    header.inner
}

fn section(
    ui: &mut egui::Ui,
    (name, tool): (&str, PictureTool),
    set: bool,
    acts: &mut Vec<Act>,
    rows: impl FnOnce(&mut egui::Ui, &mut Vec<Act>),
) {
    let reset = |ui: &mut egui::Ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled(set, egui::Button::new("Reset")).clicked()
        })
        .inner
    };
    if group(ui, name, reset, |ui| rows(ui, acts)) {
        acts.push(Act::Clear(tool));
    }
}

fn transform(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let first = &chosen[0];
    let set = chosen
        .iter()
        .any(|c| uses(&c.shown, PictureTool::Transform));
    section(
        ui,
        ("Transform", PictureTool::Transform),
        set,
        acts,
        |ui, acts| {
            let [zx, zy] = first.shown.scale();
            if !i.unlinked && zx == zy {
                number(ui, (Field::Zoom, Some(true)), i, p, at, chosen, acts);
            } else {
                number(ui, (Field::ZoomX, Some(false)), i, p, at, chosen, acts);
                number(ui, (Field::ZoomY, None), i, p, at, chosen, acts);
            }
            number(ui, (Field::X, None), i, p, at, chosen, acts);
            number(ui, (Field::Y, None), i, p, at, chosen, acts);
            number(ui, (Field::Rotation, None), i, p, at, chosen, acts);
            number(ui, (Field::AnchorX, None), i, p, at, chosen, acts);
            number(ui, (Field::AnchorY, None), i, p, at, chosen, acts);
            number(ui, (Field::Pitch, None), i, p, at, chosen, acts);
            number(ui, (Field::Yaw, None), i, p, at, chosen, acts);
            let flipped = chosen
                .iter()
                .any(|c| c.shown.flip_horizontal || c.shown.flip_vertical);
            label(ui, "Flip", flipped);
            ui.horizontal(|ui| {
                let p = &first.shown;
                for (k, icon, text, on) in [
                    (0, FLIP_HORIZONTAL, "Horizontal", p.flip_horizontal),
                    (1, FLIP_VERTICAL, "Vertical", p.flip_vertical),
                ] {
                    let b = egui::Button::selectable(on, format!("{icon} {text}"));
                    if ui.add(b).clicked() {
                        let mut to = [None; 2];
                        to[k] = Some(!on);
                        acts.push(Act::Flips(to));
                    }
                }
            });
            if reset(ui, flipped) {
                acts.push(Act::Flips([Some(false); 2]));
            }
            ui.end_row();
        },
    );
}

fn crop(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let set = chosen.iter().any(|c| uses(&c.shown, PictureTool::Crop));
    section(ui, ("Crop", PictureTool::Crop), set, acts, |ui, acts| {
        for f in [
            Field::Left,
            Field::Right,
            Field::Top,
            Field::Bottom,
            Field::Softness,
        ] {
            number(ui, (f, None), i, p, at, chosen, acts);
        }
    });
}

fn composite(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    acts: &mut Vec<Act>,
) {
    let set = chosen
        .iter()
        .any(|c| uses(&c.shown, PictureTool::Composite));
    let section_name = ("Composite", PictureTool::Composite);
    section(ui, section_name, set, acts, |ui, acts| {
        let now = chosen[0].shown.mode();
        let mixed = chosen
            .iter()
            .any(|c| c.shown.mode() != CompositeMode::Normal);
        label(ui, "Composite Mode", mixed);
        egui::ComboBox::from_id_salt("inspector-mode")
            .selected_text(mode_label(now))
            .show_ui(ui, |ui| {
                for m in CompositeMode::ALL {
                    if ui.selectable_label(m == now, mode_label(m)).clicked() {
                        acts.push(Act::Mode(m));
                    }
                }
            });
        if reset(ui, mixed) {
            acts.push(Act::Mode(CompositeMode::Normal));
        }
        ui.end_row();
        number(ui, (Field::Opacity, None), i, p, at, chosen, acts);
    });
}

fn dynamic_zoom(
    ui: &mut egui::Ui,
    i: &Inspector,
    p: &Project,
    at: i64,
    chosen: &[Chosen],
    boxes: bool,
    acts: &mut Vec<Act>,
) {
    let zooms: Vec<DynamicZoom> = chosen.iter().filter_map(|c| c.shown.dynamic_zoom).collect();
    let on = !zooms.is_empty();
    let first = chosen[0].shown.dynamic_zoom;
    let section_name = ("Dynamic Zoom", PictureTool::DynamicZoom);
    section(ui, section_name, on, acts, |ui, acts| {
        label(ui, "Dynamic Zoom", on);
        dialog::switch(ui, |ui| {
            if ui
                .add(egui::Button::selectable(first.is_none(), "Off"))
                .clicked()
                && on
            {
                acts.push(Act::Zooming(false));
            }
            if ui
                .add(egui::Button::selectable(first.is_some(), "On"))
                .clicked()
                && zooms.len() < chosen.len()
            {
                acts.push(Act::Zooming(true));
            }
        });
        if reset(ui, on) {
            acts.push(Act::Zooming(false));
        }
        ui.end_row();
        for f in [
            Field::StartZoom,
            Field::StartX,
            Field::StartY,
            Field::EndZoom,
            Field::EndX,
            Field::EndY,
        ] {
            number(ui, (f, None), i, p, at, chosen, acts);
        }
        let eased = zooms.iter().any(|z| z.ease != Ease::Linear);
        label(ui, "Ease", eased);
        ui.add_enabled_ui(first.is_some(), |ui| {
            let now = first.map_or(Ease::Linear, |z| z.ease);
            egui::ComboBox::from_id_salt("inspector-ease")
                .selected_text(ease_label(now))
                .show_ui(ui, |ui| {
                    for e in Ease::ALL {
                        if ui.selectable_label(e == now, ease_label(e)).clicked() {
                            acts.push(Act::Ease(e));
                        }
                    }
                });
        });
        if reset(ui, eased) {
            acts.push(Act::Ease(Ease::Linear));
        }
        ui.end_row();
        label(ui, "", false);
        ui.horizontal(|ui| {
            let show = egui::Button::selectable(boxes, "Show Boxes");
            if ui.add(show).clicked() {
                acts.push(Act::Boxes);
            }
            if ui.add_enabled(on, egui::Button::new("Swap")).clicked() {
                acts.push(Act::Swap);
            }
        });
        ui.end_row();
    });
}

fn section_speed(c: &Paced, to: f64, ripple: bool) -> Option<(&'static str, Value)> {
    let (from, clips, at) = (c.shown.0, [&c.id], c.at);
    if to == 0.0 {
        let params = json!({ "clips": clips, "at": at, "ripple": ripple });
        return Some(("speed.freeze", params));
    }
    if (to < 0.0) != (from < 0.0) && from != 0.0 {
        let params = json!({ "clips": clips, "at": at, "backward": to < 0.0 });
        return Some(("speed.reverse", params));
    }
    let params = json!({ "clips": clips, "at": at, "speed": to, "ripple": ripple });
    (from > 0.0).then_some(("speed.set", params))
}

fn speed_keys(ui: &mut egui::Ui, (keys, inside): (KeyMark, bool)) -> Option<Act> {
    let mut act = None;
    let prev = ui.add_enabled(keys.prev, egui::Button::new(CARET_LEFT).frame(false));
    if prev.clicked() {
        act = Some(Act::Step(Key::Speed, -1));
    }
    let diamond = egui::Button::selectable(keys.on, DIAMOND).frame(false);
    if ui.add_enabled(inside || keys.on, diamond).clicked() {
        act = Some(Act::Toggle(Key::Speed));
    }
    let next = ui.add_enabled(keys.next, egui::Button::new(CARET_RIGHT).frame(false));
    if next.clicked() {
        act = Some(Act::Step(Key::Speed, 1));
    }
    act
}

fn speed_section(
    ui: &mut egui::Ui,
    (i, p, dropped): (&Inspector, &Project, i64),
    (paced, at): (&[Paced], i64),
    ripple: bool,
    acts: &mut Vec<Act>,
) {
    let (speed, length) = paced[0].shown;
    let rate = paced[0].rate;
    let frozen = speed == 0.0;
    let count = paced.len();
    let turned = paced.iter().any(|c| c.shown.0 <= 0.0);
    let sped = paced
        .iter()
        .any(|c| c.shown.0 != 0.0 && c.shown.0.abs() != 1.0);
    let ramped = paced.iter().any(|c| c.ramped);
    let stuck = paced.iter().any(|c| c.ramped && c.shown.0 == 0.0);
    let back = paced.iter().any(|c| c.ramped && c.shown.0 < 0.0);
    let keys = p
        .clip(&paced[0].id)
        .filter(|c| c.compound.is_none())
        .map(|c| {
            let keys = c.speed.as_ref().map(Speed::keyframes).unwrap_or_default();
            (mark(c, &keys, at), c.start < at && at < c.end())
        });
    let header = |ui: &mut egui::Ui| {
        let mut act = keys.and_then(|k| speed_keys(ui, k));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let on = turned || sped || ramped;
            if ui.add_enabled(on, egui::Button::new("Reset")).clicked() {
                act = Some(Act::FullSpeed);
            }
            if count > 1 {
                ui.label(dim(format!("{count} clips")));
            }
        });
        act
    };
    let picked = group(ui, "Speed", header, |ui| {
        label(ui, "Direction", turned);
        dialog::switch(ui, |ui| {
            for d in Direction::ALL {
                let on = Direction::of(speed) == d;
                let free = !stuck || d == Direction::Freeze;
                let b = ui
                    .add_enabled(free, egui::Button::selectable(on, d.label()))
                    .on_disabled_hover_text("A freeze between speed keyframes has no direction");
                if b.clicked() && !on {
                    acts.push(Act::Direction(d));
                }
            }
        });
        if reset(ui, turned && !stuck) {
            acts.push(Act::Direction(Direction::Forward));
        }
        ui.end_row();
        let eases = p
            .clip(&paced[0].id)
            .filter(|c| c.compound.is_none())
            .and_then(|c| vidcut_core::ops::speed::section_eases(c, paced[0].at));
        if let Some((now, options)) = eases {
            label(ui, "Ease", !now.is_linear());
            ui.add_enabled_ui(options.len() > 1, |ui| {
                egui::ComboBox::from_id_salt("inspector-speed-ease")
                    .selected_text(ease_label(now))
                    .show_ui(ui, |ui| {
                        for &e in &options {
                            if ui.selectable_label(e == now, ease_label(e)).clicked() && e != now {
                                acts.push(Act::SpeedEase(e));
                            }
                        }
                    });
            });
            if reset(ui, !now.is_linear()) {
                acts.push(Act::SpeedEase(Ease::Linear));
            }
            ui.end_row();
        }
        let moving = Look {
            off: frozen || back,
            why: (back && !frozen).then_some("Only a forward section takes a speed"),
            ..Look::default()
        };
        let (least, most) = (
            *SpeedDialog::PERCENT.start() / 100.0,
            *SpeedDialog::PERCENT.end() / 100.0,
        );
        let percent = (sped, speed.abs() * 100.0, SpeedDialog::PERCENT);
        field(ui, (Key::Speed, None), i, percent, moving, None, acts);
        let fps = (false, speed.abs() * rate, least * rate..=most * rate);
        field(ui, (Key::Rate, None), i, fps, moving, None, acts);
        let span = length as f64 * speed.abs();
        let lengths = (span / most).ceil().max(1.0)..=(span / least).floor().max(1.0);
        let timed = Look {
            off: frozen || !ripple || ramped,
            why: if ramped {
                Some("Off on a clip with speed keyframes")
            } else {
                (!frozen && !ripple).then_some("Needs Ripple Timeline")
            },
            timecode: Some((p, dropped)),
        };
        field(
            ui,
            (Key::Length, None),
            i,
            (false, length as f64, lengths),
            timed,
            None,
            acts,
        );
        label(ui, "Ripple Timeline", false);
        let mut on = ripple;
        if ui.checkbox(&mut on, "").clicked() {
            acts.push(Act::Ripple(on));
        }
        ui.end_row();
        let (retime, project) = (paced[0].retime, paced[0].project);
        let text = |r| super::retime_process::clip_text(r, project);
        label(ui, "Retime Process", !retime.is_project());
        egui::ComboBox::from_id_salt("inspector-retime")
            .selected_text(text(retime))
            .show_ui(ui, |ui| {
                for r in super::retime_process::PROCESSES {
                    if ui.selectable_label(retime == r, text(r)).clicked() && r != retime {
                        acts.push(Act::Retime(r));
                    }
                }
            });
        if reset(ui, !retime.is_project()) {
            acts.push(Act::Retime(ClipRetime::Project));
        }
        ui.end_row();
        let chosen = retime.process().or(p.settings.retime.process());
        if chosen.is_some_and(|r| !r.is_nearest()) && paced.iter().any(|c| c.hdr) {
            ui.label("");
            dialog::warn(ui, "Retime Process takes SDR pictures for now");
            ui.end_row();
        }
    });
    acts.extend(picked);
}

impl Workspace {
    fn chosen(&self) -> Vec<Chosen> {
        let Some(p) = self.project_shared() else {
            return Vec::new();
        };
        let wanted = self.picture_wanted();
        self.snap
            .selection
            .clips
            .iter()
            .filter_map(|id| {
                let (ti, ci) = p.find_clip(id)?;
                let c = &p.tracks[ti].clips[ci];
                (p.tracks[ti].kind == TrackKind::Video && has_picture(c)).then(|| {
                    let stored = c.picture.clone().unwrap_or_default();
                    let shown = wanted
                        .iter()
                        .find(|(w, _)| *w == c.id)
                        .map_or_else(|| stored.clone(), |(_, x)| x.clone());
                    Chosen {
                        id: c.id.clone(),
                        stored,
                        shown,
                        shape: shape(&p, c),
                        frame: c.content_at(self.snap.timeline.position),
                    }
                })
            })
            .collect()
    }

    fn inspector_landed(&self) -> Vec<(String, Picture)> {
        let (Some(l), Some(p)) = (&self.inspector.landing, self.project()) else {
            return Vec::new();
        };
        l.pictures
            .iter()
            .filter_map(|(id, from, to)| {
                let stored = p.clip(id)?.picture.clone().unwrap_or_default();
                landing(to, l.since.elapsed(), from, &stored).map(|t| (id.clone(), t.clone()))
            })
            .collect()
    }

    fn sounds(&self) -> Vec<Sound> {
        let Some(p) = self.project() else {
            return Vec::new();
        };
        let gained = self.inspector.gained.as_ref();
        let at = self.snap.timeline.position as f64;
        self.snap
            .selection
            .clips
            .iter()
            .filter_map(|id| {
                let c = p.clip(id)?;
                let a = c.audio.as_ref()?;
                let stored = c.gain_at(at)?;
                let gain = gained
                    .and_then(|g| {
                        let (_, from) = g.from.iter().find(|(f, _)| f == id)?;
                        landing(g.to, g.since.elapsed(), *from, stored)
                    })
                    .unwrap_or(stored);
                Some(Sound {
                    id: c.id.clone(),
                    stored,
                    gain,
                    muted: a.muted,
                    keyed: a.is_keyed(),
                })
            })
            .collect()
    }

    fn paced(&self) -> Vec<Paced> {
        let Some(p) = self.project() else {
            return Vec::new();
        };
        let retimed = self.inspector.retimed.as_ref();
        let position = self.snap.timeline.position;
        self.snap
            .selection
            .clips
            .iter()
            .filter_map(|id| {
                let c = p
                    .clip(id)
                    .filter(|c| !c.is_generator() && c.title.is_none())?;
                let at = position.clamp(c.start, c.end() - 1);
                let stored = (c.speed_at(at), c.duration);
                let shown = retimed
                    .and_then(|r| {
                        let (_, from, to) = r.clips.iter().find(|(f, _, _)| f == id)?;
                        landing(*to, r.since.elapsed(), *from, stored)
                    })
                    .unwrap_or(stored);
                Some(Paced {
                    id: c.id.clone(),
                    stored,
                    shown,
                    rate: rate(p, c),
                    retime: c.retime,
                    project: p.media(&c.media).map_or(Retime::Nearest, |m| {
                        vidcut_core::render::retime_at(
                            ClipRetime::Project,
                            c.speed_at(at),
                            m,
                            &p.settings,
                        )
                    }),
                    hdr: p
                        .media(&c.media)
                        .and_then(|m| m.video_as_read())
                        .is_some_and(|v| v.hdr.is_some()),
                    at,
                    ramped: c.speed.as_ref().is_some_and(|s| s.sections.len() > 1),
                })
            })
            .collect()
    }

    fn ripple(&self) -> bool {
        self.inspector
            .ripple
            .unwrap_or_else(|| self.project().is_some_and(|p| p.settings.ripple_on))
    }

    fn send_speeds(
        &mut self,
        (paced, whole): (&[Paced], bool),
        length: Option<i64>,
        change: impl Fn(&mut SpeedDialog, &Paced),
    ) {
        let ripple = self.ripple();
        let mut groups: Vec<(SpeedDialog, Vec<String>)> = Vec::new();
        let mut sections = Vec::new();
        let mut clips = Vec::new();
        for c in paced {
            let mut d = SpeedDialog::of(c.shown.0, ripple);
            change(&mut d, c);
            let to = d.speed();
            let section = c.ramped && !whole;
            if (to - c.shown.0).abs() < SAME && (section || !c.ramped) {
                continue;
            }
            let (from, frames) = c.shown;
            let length = length.unwrap_or(if ripple && to != 0.0 && from != 0.0 && !c.ramped {
                ((frames as f64 * from.abs() / to.abs()) + 1e-6)
                    .floor()
                    .max(1.0) as i64
            } else {
                frames
            });
            if section {
                let Some(command) = section_speed(c, to, ripple) else {
                    continue;
                };
                sections.push(command);
            } else {
                match groups.iter_mut().find(|(g, _)| g.speed() == to) {
                    Some((_, ids)) => ids.push(c.id.clone()),
                    None => groups.push((d, vec![c.id.clone()])),
                }
            }
            clips.push((c.id.clone(), c.stored, (to, length)));
        }
        let mut commands: Vec<(&'static str, Value)> = groups
            .iter()
            .map(|(d, ids)| ("edit.change_speed", d.params(ids)))
            .chain(sections)
            .collect();
        match commands.len() {
            0 => return,
            1 => {
                let (method, params) = commands.remove(0);
                self.send_asking(method, params);
            }
            _ => {
                let commands: Vec<Value> = commands
                    .into_iter()
                    .map(|(m, p)| json!({ "method": m, "params": p }))
                    .collect();
                self.send_asking("apply", json!({ "commands": commands }));
            }
        }
        self.inspector.retimed = Some(Retimed {
            since: Instant::now(),
            clips,
        });
    }

    fn ease_sections(&mut self, paced: &[Paced], ease: Ease) {
        let mut commands: Vec<Value> = paced
            .iter()
            .filter(|c| c.ramped)
            .map(|c| {
                let params = json!({ "clips": [c.id], "at": c.at, "ease": ease });
                json!({ "method": "speed.ease", "params": params })
            })
            .collect();
        match commands.len() {
            0 => {}
            1 => {
                let c = commands.remove(0);
                self.send("speed.ease", c["params"].clone());
            }
            _ => self.send("apply", json!({ "commands": commands })),
        }
    }

    fn send_percent(
        &mut self,
        paced: &[Paced],
        length: Option<i64>,
        percent: impl Fn(&Paced) -> f64,
    ) {
        self.send_speeds((paced, false), length, |d, c| {
            if !d.freeze {
                d.set_percent(percent(c));
            }
        });
    }

    pub(super) fn inspector_wanted(&self) -> Vec<(String, Picture)> {
        match self.inspector.held.as_ref().filter(|h| h.dragging) {
            Some(Held {
                field: Key::Picture(f),
                value,
                start,
                ..
            }) => start
                .iter()
                .map(|c| {
                    (
                        c.id.clone(),
                        f.put_at(&c.shown, &c.shape, *value, Some(c.frame)),
                    )
                })
                .collect(),
            _ => self.inspector_landed(),
        }
    }

    pub(super) fn settle_inspector(&mut self) {
        if self.inspector.landing.is_some() && self.inspector_landed().is_empty() {
            self.inspector.landing = None;
        }
    }

    pub(super) fn leave_inspector(&mut self) {
        self.inspector.held = None;
        self.inspector.seen = None;
        self.close_forms();
    }

    pub(super) fn close_forms(&mut self) {
        self.close_title_form();
        self.close_cue_form();
        self.close_track_form();
    }

    fn send_pictures(&mut self, chosen: &[Chosen], to: impl Fn(&Chosen) -> Option<Picture>) {
        let list: Vec<(String, Picture, Picture)> = chosen
            .iter()
            .filter_map(|c| {
                let p = to(c).filter(|p| *p != c.shown)?;
                Some((c.id.clone(), c.stored.clone(), p))
            })
            .collect();
        let at = self.snap.timeline.position;
        let params: Vec<Value> = list
            .iter()
            .filter_map(|(id, from, to)| {
                changed(id, from, to).map(|mut p| {
                    p["at"] = json!(at);
                    p
                })
            })
            .collect();
        if params.is_empty() {
            return;
        }
        self.end_picture_wheel();
        match merged(&params) {
            Some(one) => self.send("picture.set", one),
            None => {
                let commands: Vec<Value> = params
                    .into_iter()
                    .map(|p| json!({ "method": "picture.set", "params": p }))
                    .collect();
                self.send("apply", json!({ "commands": commands }));
            }
        }
        self.inspector.landing = Some(Landing {
            since: Instant::now(),
            pictures: list,
        });
    }

    fn clear_pictures(&mut self, chosen: &[Chosen], tool: PictureTool) {
        let used: Vec<&Chosen> = chosen.iter().filter(|c| uses(&c.shown, tool)).collect();
        if used.is_empty() {
            return;
        }
        let ids: Vec<&str> = used.iter().map(|c| c.id.as_str()).collect();
        self.end_picture_wheel();
        self.timeline.picture_grab.cancel_landing();
        self.send("picture.clear", json!({ "clips": ids, "tool": tool }));
        self.inspector.landing = Some(Landing {
            since: Instant::now(),
            pictures: used
                .iter()
                .map(|c| (c.id.clone(), c.stored.clone(), cleared(&c.shown, tool)))
                .collect(),
        });
    }

    fn send_volume(&mut self, sounds: &[Sound], to: f64) {
        let to = to.clamp(*GAIN.start(), *GAIN.end());
        let from: Vec<(String, f64)> = sounds
            .iter()
            .filter(|s| s.gain != to)
            .map(|s| (s.id.clone(), s.stored))
            .collect();
        if from.is_empty() {
            return;
        }
        let ids: Vec<&str> = from.iter().map(|(id, _)| id.as_str()).collect();
        let at = self.snap.timeline.position;
        self.send("clip.set", json!({ "clips": ids, "gain_db": to, "at": at }));
        self.inspector.gained = Some(Gained {
            since: Instant::now(),
            from,
            to,
        });
    }

    fn step_key(&mut self, key: Key, dir: i64, lists: Lists<'_>) {
        let at = self.snap.timeline.position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(c) = key_clips(key, lists).first().and_then(|id| p.clip(id)) else {
            return;
        };
        let inside = |t: &i64| c.start <= *t && *t < c.end();
        let times = key_times(c, key).into_iter().filter(inside);
        let target = if dir < 0 {
            times.filter(|t| *t < at).max()
        } else {
            times.filter(|t| *t > at).min()
        };
        if let Some(t) = target {
            self.seek(PlayerId::Timeline, t, false);
        }
    }

    fn toggle_key(&mut self, key: Key, lists: Lists<'_>) {
        let Some(param) = param_name(key) else {
            return;
        };
        let at = self.snap.timeline.position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let ids = key_clips(key, lists);
        let times: Vec<(&str, Vec<i64>)> = ids
            .iter()
            .filter_map(|id| Some((id.as_str(), key_times(p.clip(id)?, key))))
            .collect();
        if times.iter().any(|(_, t)| t.contains(&at)) {
            let keyed: Vec<&str> = times
                .iter()
                .filter(|(_, t)| !t.is_empty())
                .map(|(id, _)| *id)
                .collect();
            self.send(
                "keyframes.remove",
                json!({ "clips": keyed, "param": param, "at": at }),
            );
        } else if !ids.is_empty() {
            self.send(
                "keyframes.add",
                json!({ "clips": ids, "param": param, "at": at }),
            );
        }
    }

    fn ease_key(&mut self, key: Key, ease: Ease, chosen: &[Chosen], sounds: &[Sound]) {
        let Some(param) = param_name(key) else {
            return;
        };
        let at = self.snap.timeline.position;
        let Some(p) = self.project_shared() else {
            return;
        };
        let ids: Vec<&str> = match key {
            Key::Volume => sounds.iter().map(|s| s.id.as_str()).collect(),
            Key::Picture(_) => chosen.iter().map(|c| c.id.as_str()).collect(),
            _ => return,
        };
        let ids: Vec<&str> = ids
            .into_iter()
            .filter(|id| {
                p.clip(id).is_some_and(|c| match key {
                    Key::Volume => c.audio.as_ref().is_some_and(|a| {
                        a.gain_keys
                            .iter()
                            .any(|k| c.key_time(k.frame).round() as i64 == at)
                    }),
                    Key::Picture(f) => f.channel().is_some_and(|v| {
                        c.picture.as_ref().is_some_and(|pic| {
                            pic.keys_of(v)
                                .iter()
                                .any(|k| c.key_time(k.frame).round() as i64 == at)
                        })
                    }),
                    _ => false,
                })
            })
            .collect();
        if !ids.is_empty() {
            self.send(
                "keyframes.set",
                json!({ "clips": ids, "param": param, "at": at, "ease": ease }),
            );
        }
    }

    fn reset_channel(&mut self, field: Field, chosen: &[Chosen]) {
        let Some(v) = field.channel() else {
            return;
        };
        let keyed: Vec<&str> = chosen
            .iter()
            .filter(|c| c.stored.has_keys(v))
            .map(|c| c.id.as_str())
            .collect();
        let at = self.snap.timeline.position;
        let sets: Vec<Value> = chosen
            .iter()
            .filter_map(|c| {
                let base = c.stored.adopt(c.frame as f64);
                let to = field.put_at(&base, &c.shape, field.usual(), Some(c.frame));
                changed(&c.id, &base, &to).map(|mut p| {
                    p["at"] = json!(at);
                    p
                })
            })
            .collect();
        let mut commands = Vec::new();
        if !keyed.is_empty() {
            commands.push(json!({
                "method": "keyframes.remove",
                "params": { "clips": keyed, "param": v.name() }
            }));
        }
        for params in sets {
            commands.push(json!({ "method": "picture.set", "params": params }));
        }
        match commands.len() {
            0 => {}
            1 => {
                let one = commands.remove(0);
                self.send(
                    one["method"].as_str().unwrap_or("picture.set"),
                    one["params"].clone(),
                );
            }
            _ => self.send("apply", json!({ "commands": commands })),
        }
    }

    fn put(&mut self, key: Key, value: f64, (chosen, sounds, paced): Lists<'_>) {
        match key {
            Key::Picture(f) => {
                self.send_pictures(chosen, |c| {
                    Some(f.put_at(&c.shown, &c.shape, value, Some(c.frame)))
                });
            }
            Key::Volume => self.send_volume(sounds, value),
            Key::Speed => self.send_percent(paced, None, |_| value),
            Key::Rate => self.send_percent(paced, None, |c| value / c.rate * 100.0),
            Key::Length => {
                let frames = value.round().max(1.0);
                self.send_percent(paced, Some(frames as i64), |c| {
                    c.shown.1 as f64 * c.shown.0.abs() / frames * 100.0
                });
            }
        }
    }

    fn super_scale_section(&mut self, ui: &mut egui::Ui, p: &Project, chosen: &[Chosen]) {
        use super::super_scale::{SHARPNESS, UPSCALERS, label_of, unfit, upscaler_text};
        let mut media: Vec<String> = chosen
            .iter()
            .filter_map(|c| p.clip(&c.id))
            .map(|c| c.media.clone())
            .collect();
        media.sort_unstable();
        media.dedup();
        let Some(now) = media
            .first()
            .and_then(|id| p.media(id))
            .map(|m| m.super_scale)
        else {
            return;
        };
        let mut put = None;
        let header = |ui: &mut egui::Ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled(!now.is_off(), egui::Button::new("Reset"))
                    .clicked()
            })
            .inner
        };
        let shown = vidcut_core::render::super_scale_of(now, &p.settings);
        let cleared = group(ui, "Super Scale", header, |ui| {
            let on = !now.is_off();
            label(ui, "Super Scale", on);
            dialog::switch(ui, |ui| {
                for (value, text) in [(false, "Off"), (true, "On")] {
                    if ui.selectable_label(on == value, text).clicked() && on != value {
                        put = Some(json!({ "super_scale": value }));
                    }
                }
            });
            ui.end_row();
            label(ui, "Upscaler", on);
            ui.add_enabled_ui(on, |ui| {
                let project = p.settings.upscaler;
                let text = |u, l| upscaler_text(u, l, project);
                egui::ComboBox::from_id_salt("inspector-upscaler")
                    .selected_text(text(now.upscaler, label_of(UPSCALERS, now.upscaler)))
                    .show_ui(ui, |ui| {
                        for &(upscaler, l) in UPSCALERS {
                            if ui
                                .selectable_label(now.upscaler == upscaler, text(upscaler, l))
                                .clicked()
                            {
                                put = Some(json!({ "upscaler": upscaler }));
                            }
                        }
                    });
            });
            ui.end_row();
            let classic = on && shown.upscaler == Upscaler::Classic;
            let row = |ui: &mut egui::Ui,
                       name: &str,
                       enabled: bool,
                       pick: &mut dyn FnMut(&mut egui::Ui)| {
                label(ui, name, enabled);
                ui.add_enabled_ui(enabled, |ui| dialog::switch(ui, |ui| pick(ui)));
                ui.end_row();
            };
            if let Some(text) = unfit(shown, media.iter().filter_map(|id| p.media(id))) {
                ui.label("");
                dialog::warn(ui, text);
                ui.end_row();
            }
            row(ui, "Sharpness", classic, &mut |ui| {
                for (sharpness, text) in SHARPNESS {
                    if ui
                        .selectable_label(now.sharpness == sharpness, text)
                        .clicked()
                    {
                        put = Some(json!({ "sharpness": sharpness }));
                    }
                }
            });
        });
        if cleared {
            put = Some(json!({ "super_scale": false }));
        }
        if let Some(params) = put {
            self.set_super_scale(&media, params);
        }
    }

    fn audio_section(
        &mut self,
        ui: &mut egui::Ui,
        p: &Project,
        sounds: &[Sound],
        acts: &mut Vec<Act>,
    ) {
        let count = sounds.len();
        let header = |ui: &mut egui::Ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let normalize = ui.button("Normalize…").clicked();
                if count > 1 {
                    ui.label(dim(format!("{count} clips")));
                }
                normalize
            })
            .inner
        };
        let normalize = group(ui, "Audio", header, |ui| {
            let first = &sounds[0];
            let set = sounds
                .iter()
                .any(|s| s.keyed || s.gain != Key::Volume.usual());
            let volume = (set, first.gain, *GAIN.start()..=*GAIN.end());
            let keys = p.clip(&first.id).and_then(|c| {
                c.audio
                    .as_ref()
                    .map(|a| mark(c, &a.gain_keys, self.snap.timeline.position))
            });
            entry(ui, (Key::Volume, None), &self.inspector, volume, keys, acts);
            label(ui, "Mute", sounds.iter().any(|s| s.muted));
            let mut on = first.muted;
            if ui.checkbox(&mut on, "").clicked() {
                acts.push(Act::Mute(on));
            }
            ui.end_row();
            let isolated = sounds.iter().any(|s| heard(p, &s.id).flatten().is_some());
            label(ui, "Voice Isolation", isolated);
            ui.horizontal(|ui| {
                let on = Isolate::Clips(sounds.iter().map(|s| s.id.clone()).collect());
                self.isolation_controls(ui, on, heard(p, &first.id).flatten(), "");
            });
            ui.end_row();
        });
        if normalize {
            self.open_normalize(sounds.iter().map(|s| s.id.clone()).collect());
        }
    }

    fn show_boxes(&mut self, chosen: &[Chosen]) {
        self.timeline.picture_grab.tool = Some(PictureTool::DynamicZoom);
        let Some(p) = self.project_shared() else {
            return;
        };
        let at = self.snap.player(PlayerId::Timeline).position;
        let ids: Vec<String> = chosen.iter().map(|c| c.id.clone()).collect();
        if target(&p, &ids, at).is_none()
            && let Some(c) = chosen.first().and_then(|c| p.clip(&c.id))
        {
            self.seek(PlayerId::Timeline, c.start, false);
        }
    }

    fn inspect(&mut self, act: Act, (chosen, sounds, paced): Lists<'_>) {
        let held_on = |i: &Inspector, f: Key| i.held.as_ref().is_some_and(|h| h.field == f);
        match act {
            Act::Hold(field, value) => {
                self.inspector.held = Some(Held {
                    field,
                    value,
                    from: value,
                    start: chosen.to_vec(),
                    dragging: true,
                });
            }
            Act::Drag(field, value) => {
                if let Some(h) = self.inspector.held.as_mut().filter(|h| h.field == field) {
                    h.value = value;
                }
            }
            Act::Land(field) => {
                let Some(h) = self
                    .inspector
                    .held
                    .take_if(|h| h.field == field && h.dragging)
                else {
                    return;
                };
                if h.value != h.from {
                    self.put(field, h.value, (&h.start, sounds, paced));
                }
            }
            Act::Put(field, value) => {
                if held_on(&self.inspector, field) {
                    self.inspector.held = None;
                }
                self.put(field, value, (chosen, sounds, paced));
            }
            Act::Edit(field, value) => {
                if !held_on(&self.inspector, field) {
                    self.inspector.held = Some(Held {
                        field,
                        value,
                        from: value,
                        start: Vec::new(),
                        dragging: false,
                    });
                }
            }
            Act::Release(field) => {
                if self
                    .inspector
                    .held
                    .as_ref()
                    .is_some_and(|h| h.field == field && !h.dragging)
                {
                    self.inspector.held = None;
                }
            }
            Act::Direction(to) => self.send_speeds((paced, false), None, |d, _| to.turn(d)),
            Act::SpeedEase(ease) => self.ease_sections(paced, ease),
            Act::Ripple(on) => self.inspector.ripple = Some(on),
            Act::Retime(r) => {
                let ids: Vec<String> = paced.iter().map(|c| c.id.clone()).collect();
                self.set_retime(&ids, r);
            }
            Act::FullSpeed => self.send_speeds((paced, true), None, |d, _| {
                Direction::Forward.turn(d);
                d.set_percent(Key::Speed.usual());
            }),
            Act::Link(on) => {
                self.inspector.unlinked = !on;
                if on {
                    self.send_pictures(chosen, |c| {
                        let [zx, zy] = c.shown.scale();
                        (zx != zy).then(|| Picture {
                            zoom: Some([zx, zx]),
                            ..c.shown.clone()
                        })
                    });
                }
            }
            Act::Flips([across, down]) => self.send_pictures(chosen, |c| {
                Some(Picture {
                    flip_horizontal: across.unwrap_or(c.shown.flip_horizontal),
                    flip_vertical: down.unwrap_or(c.shown.flip_vertical),
                    ..c.shown.clone()
                })
            }),
            Act::Zooming(on) => {
                self.send_pictures(chosen, |c| {
                    let z = c.shown.dynamic_zoom;
                    Some(Picture {
                        dynamic_zoom: if on {
                            z.or(Some(DynamicZoom::default()))
                        } else {
                            None
                        },
                        ..c.shown.clone()
                    })
                });
                if on {
                    self.show_boxes(chosen);
                }
            }
            Act::Boxes => {
                if self.timeline.picture_grab.tool == Some(PictureTool::DynamicZoom) {
                    self.timeline.picture_grab.tool = None;
                } else {
                    self.show_boxes(chosen);
                }
            }
            Act::Ease(ease) => self.send_pictures(chosen, |c| {
                let z = c.shown.dynamic_zoom?;
                Some(Picture {
                    dynamic_zoom: Some(DynamicZoom { ease, ..z }),
                    ..c.shown.clone()
                })
            }),
            Act::Mode(mode) => self.send_pictures(chosen, |c| {
                Some(Picture {
                    composite_mode: Some(mode).filter(|m| *m != CompositeMode::Normal),
                    ..c.shown.clone()
                })
            }),
            Act::Swap => self.send_pictures(chosen, |c| {
                let z = c.shown.dynamic_zoom?;
                Some(Picture {
                    dynamic_zoom: Some(z.swapped()),
                    ..c.shown.clone()
                })
            }),
            Act::Clear(tool) => self.clear_pictures(chosen, tool),
            Act::Mute(on) => {
                let ids: Vec<&str> = sounds
                    .iter()
                    .filter(|s| s.muted != on)
                    .map(|s| s.id.as_str())
                    .collect();
                if !ids.is_empty() {
                    self.send("clip.set", json!({ "clips": ids, "muted": on }));
                }
            }
            Act::Step(key, dir) => self.step_key(key, dir, (chosen, sounds, paced)),
            Act::Toggle(key) => self.toggle_key(key, (chosen, sounds, paced)),
            Act::KeyEase(key, ease) => self.ease_key(key, ease, chosen, sounds),
            Act::ResetChannel(field) => self.reset_channel(field, chosen),
            Act::Unkey => {
                let keyed: Vec<&str> = sounds
                    .iter()
                    .filter(|s| s.keyed)
                    .map(|s| s.id.as_str())
                    .collect();
                if keyed.is_empty() {
                    self.send_volume(sounds, Key::Volume.usual());
                    return;
                }
                let ids: Vec<&str> = sounds.iter().map(|s| s.id.as_str()).collect();
                let commands = json!([
                    { "method": "keyframes.remove", "params": { "clips": keyed } },
                    { "method": "clip.set", "params": { "clips": ids, "gain_db": 0.0 } },
                ]);
                self.send("apply", json!({ "commands": commands }));
            }
        }
    }

    pub(super) fn inspector_saw_selection(&mut self) {
        let s = &self.snap.selection;
        self.inspector.seen = Some((s.clips.clone(), s.cues.clone()));
    }

    fn follow_selection(&mut self) {
        let s = &self.snap.selection;
        let now = (s.clips.clone(), s.cues.clone());
        if self.inspector.seen.as_ref() == Some(&now) {
            return;
        }
        let title = self.project_shared().and_then(|p| {
            now.0
                .iter()
                .find(|id| p.clip(id).is_some_and(|c| c.title.is_some()))
                .cloned()
        });
        if let Some(id) = title {
            self.inspect_title(&id, false);
        } else if let Some(id) = now.1.first().cloned() {
            self.inspect_cue(&id, false);
        } else {
            self.close_forms();
        }
        self.inspector.seen = Some(now);
    }

    pub(super) fn inspector_commits(&self) -> bool {
        self.inspector.commit
    }

    pub(super) fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let focus = ui.ctx().memory(|m| m.focused());
        let i = &mut self.inspector;
        i.moved |= focus != i.focus;
        i.focus = focus;
        i.commit = (focus.is_none() || i.moved) && !ui.input(|i| i.pointer.any_down());
        i.moved &= !i.commit;
        self.follow_selection();
        let chosen = self.chosen();
        let sounds = self.sounds();
        if sounds.iter().all(|s| s.gain == s.stored) {
            self.inspector.gained = None;
        }
        let paced = self.paced();
        if paced.iter().all(|c| c.shown == c.stored) {
            self.inspector.retimed = None;
        }
        let ripple = self.ripple();
        let dropped = self.dropped();
        let gone = |k: Key| match k {
            Key::Picture(_) => chosen.is_empty(),
            Key::Volume => sounds.is_empty(),
            Key::Speed | Key::Rate | Key::Length => paced.is_empty(),
        };
        if self.inspector.held.as_ref().is_some_and(|h| gone(h.field)) {
            self.inspector.held = None;
        }
        let mut acts = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - dialog::scroll_room(ui));
                let forms = self.title_form(ui) | self.cue_form(ui) | self.track_form(ui);
                let Some(p) = self.project_shared() else {
                    return;
                };
                let top = chosen.first().map(|c| &c.id);
                let Some(first) = top
                    .or(sounds.first().map(|s| &s.id))
                    .or(paced.first().map(|c| &c.id))
                    .and_then(|id| p.clip(id))
                else {
                    if !forms {
                        ui.label(dim("Select a clip with a picture or sound"));
                    }
                    return;
                };
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if chosen.len() > 1 {
                            ui.label(dim(format!("{} clips selected", chosen.len())));
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let text = RichText::new(name(&p, first)).strong();
                            ui.add(egui::Label::new(text).truncate());
                        });
                    });
                });
                if !chosen.is_empty() {
                    let boxes = self.timeline.picture_grab.tool == Some(PictureTool::DynamicZoom);
                    let i = &self.inspector;
                    let at = self.snap.timeline.position;
                    transform(ui, i, &p, at, &chosen, &mut acts);
                    crop(ui, i, &p, at, &chosen, &mut acts);
                    dynamic_zoom(ui, i, &p, at, &chosen, boxes, &mut acts);
                    composite(ui, i, &p, at, &chosen, &mut acts);
                    self.super_scale_section(ui, &p, &chosen);
                }
                if !paced.is_empty() {
                    let state = (&self.inspector, &*p, dropped);
                    let at = self.snap.timeline.position;
                    speed_section(ui, state, (&paced, at), ripple, &mut acts);
                }
                if !sounds.is_empty() {
                    self.audio_section(ui, &p, &sounds, &mut acts);
                }
            });
        for act in acts {
            self.inspect(act, (&chosen, &sounds, &paced));
        }
        self.sync_picture_preview();
    }
}
