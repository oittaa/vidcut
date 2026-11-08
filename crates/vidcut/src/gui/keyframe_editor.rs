use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, Layout, Pos2, Rect, RichText, Sense, Shape, Stroke, TextStyle,
    Vec2, pos2, vec2,
};
use egui_phosphor::regular::{
    ARROW_COUNTER_CLOCKWISE, ARROWS_OUT_LINE_HORIZONTAL, CARET_DOWN, CARET_LEFT, CARET_RIGHT,
    CHART_LINE, DOTS_THREE, MAGNIFYING_GLASS_PLUS, WAVE_SINE,
};
use serde_json::{Value, json};
use vidcut_core::generators::{self, CENTER};
use vidcut_core::keyframes::{
    PictureValue, bent, ease_of, eases, handle_point, value_mapped, value_paired,
};
use vidcut_core::model::{
    Clip, Ease, EffectValue, Generator, Handle, Keyframe, LowerThirdLine, Picture, Project, Speed,
    Title,
};
use vidcut_core::ops::picture::has_picture;
use vidcut_core::request::{EffectParam, KeyParam};
use vidcut_core::validate::GAIN;

use super::inspector::{Shape as Frame, key_diamond, shape};
use super::placing::ease_label;
use super::theme;
use super::timeline::geometry::SNAP_PX;
use super::title_browser::LeftTab;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

const LABEL: f32 = 182.0;
const RULER_H: f32 = 20.0;
const GROUP_H: f32 = 22.0;
const TRACK_H: f32 = 26.0;
const CURVE_H: f32 = 110.0;
const GRAPH_H: f32 = 280.0;
const GRIP_H: f32 = 6.0;
const TRACKS_KEPT: f32 = 3.0;
const DIMMED: f32 = 0.65;
const HUES: [Color32; 6] = [
    Color32::from_rgb(240, 140, 60),
    Color32::from_rgb(90, 170, 250),
    Color32::from_rgb(110, 205, 120),
    Color32::from_rgb(235, 110, 170),
    Color32::from_rgb(225, 205, 80),
    Color32::from_rgb(90, 210, 210),
];
const BAR_H: f32 = 68.0;
const SCROLL_H: f32 = 12.0;
const ZOOM_W: f32 = 72.0;
const LEAST_FRAMES: f64 = 4.0;
const DETAIL: f64 = 4.0;
const HIT: f32 = 8.0;
const LINE_HIT: f32 = 6.0;
const TICK_GAP: f32 = 96.0;
const FIELD_W: f32 = 72.0;
const TRAY_ROWS: usize = 4;
const SELECT_ONE: &str = "Select a clip or a subtitle.";
const GROUPS: [Group; 6] = [
    Group::Generator,
    Group::Transform,
    Group::Crop,
    Group::Composite,
    Group::Speed,
    Group::Audio,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Generator,
    Transform,
    Crop,
    Composite,
    Effect(usize),
    Speed,
    Audio,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Mark {
    param: KeyParam,
    frame: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Axis {
    None,
    X,
    Y,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lock {
    Time,
    Value,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Keys,
    Group,
    Handle,
    Bend,
}

#[derive(Clone, Copy, Debug)]
struct HandleEdit {
    mark: Mark,
    out: bool,
    handle: Handle,
}

#[derive(Clone, Debug)]
struct Shift {
    marks: Vec<Mark>,
    dt: i64,
    dv: f64,
    axis: Axis,
    handles: Vec<HandleEdit>,
    snap: Option<i64>,
}

#[derive(Clone, Debug)]
struct Drag {
    clip: String,
    kind: Kind,
    origin: Pos2,
    ppf: f32,
    v_span: f64,
    v_height: f32,
    marks: Vec<Mark>,
    axis: Axis,
    fix: Option<HandleFix>,
    bend: Option<BendFix>,
    frozen: Option<(KeyParam, f64, f64)>,
    apart: bool,
    lock: Option<Lock>,
    snap: Option<i64>,
}

impl Drag {
    fn new(clip: &Clip, kind: Kind, origin: Pos2, map: TimeMap, marks: Vec<Mark>) -> Self {
        Self {
            clip: clip.id.clone(),
            kind,
            origin,
            ppf: map.ppf(),
            v_span: 0.0,
            v_height: 1.0,
            marks,
            axis: Axis::None,
            fix: None,
            bend: None,
            frozen: None,
            apart: false,
            lock: None,
            snap: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HandleFix {
    mark: Mark,
    out: bool,
    frames: f64,
    value: f64,
    key_value: f64,
    neighbor: i64,
    across: Option<(Handle, f64)>,
}

#[derive(Clone, Copy, Debug)]
struct BendFix {
    param: KeyParam,
    a: Keyframe,
    b: Keyframe,
    frame: f64,
    before: Option<(Handle, f64)>,
    after: Option<(Handle, f64)>,
}

#[derive(Clone, Debug)]
struct Pending {
    clip: String,
    rev: u64,
    since: Instant,
    shift: Shift,
}

#[derive(Clone, Debug)]
struct Lane {
    param: KeyParam,
    keys: Vec<Keyframe>,
    extra: Vec<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Shown {
    from: i64,
    key: Keyframe,
    y: f64,
    paired: bool,
}

#[derive(Clone, Copy)]
struct Plot {
    lo: f64,
    hi: f64,
    label: f32,
    width: f32,
    play: i64,
    snap: Option<i64>,
    tray: bool,
}

#[derive(Clone, Copy, Debug)]
struct TimeMap {
    lo: f64,
    hi: f64,
    left: f32,
    width: f32,
}

#[derive(Clone, Copy, Debug)]
struct ValueMap {
    lo: f64,
    hi: f64,
    top: f32,
    height: f32,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    mark: Mark,
    y: bool,
    value: f64,
    from: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowAct {
    Curve,
    Step(i64),
    Toggle,
    Reset,
}

#[derive(Clone, Debug)]
struct Boxing {
    clip: String,
    origin: Pos2,
    tray: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Nudge {
    Left,
    Right,
    Up,
    Down,
}

impl Nudge {
    pub(super) const ALL: [Nudge; 4] = [Nudge::Left, Nudge::Right, Nudge::Up, Nudge::Down];

    pub(super) fn label(self) -> &'static str {
        match self {
            Nudge::Left => "Move Selected Keyframes Left",
            Nudge::Right => "Move Selected Keyframes Right",
            Nudge::Up => "Move Selected Keyframes Up",
            Nudge::Down => "Move Selected Keyframes Down",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum KeyCmd {
    Add,
    Delete,
    DeleteAll,
}

impl KeyCmd {
    pub(super) const ALL: [KeyCmd; 3] = [KeyCmd::Add, KeyCmd::Delete, KeyCmd::DeleteAll];

    pub(super) fn label(self) -> &'static str {
        match self {
            KeyCmd::Add => "Add Keyframe",
            KeyCmd::Delete => "Delete Keyframe",
            KeyCmd::DeleteAll => "Delete All Keyframes",
        }
    }

    pub(super) fn action(self) -> Option<vidcut_core::shortcuts::Action> {
        match self {
            KeyCmd::Add => Some(vidcut_core::shortcuts::Action::AddKeyframe),
            KeyCmd::Delete => Some(vidcut_core::shortcuts::Action::DeleteKeyframe),
            KeyCmd::DeleteAll => None,
        }
    }
}

#[derive(Clone, Debug)]
struct Deleting {
    clips: Vec<String>,
    params: Vec<KeyParam>,
    question: String,
    fresh: bool,
}

pub(super) fn takes_speed(c: &Clip) -> bool {
    c.title.is_none() && c.compound.is_none() && !c.is_generator() && !is_cue(c)
}

fn is_cue(c: &Clip) -> bool {
    c.id.starts_with('q')
}

fn speed_keys(c: &Clip) -> Vec<Keyframe> {
    c.speed.as_ref().map(Speed::keyframes).unwrap_or_default()
}

fn speed_key_upto(c: &Clip, play: i64) -> Option<i64> {
    let u = c.content_at(play);
    speed_keys(c)
        .into_iter()
        .map(|k| k.frame)
        .filter(|&f| f <= u)
        .max()
}

fn speed_key_at(c: &Clip, play: i64) -> bool {
    let u = c.content_at(play);
    speed_keys(c).iter().any(|k| k.frame == u)
}

fn speed_key_count(c: &Clip) -> usize {
    speed_keys(c).len()
}

pub(super) fn add_speed_key_why(clips: &[&Clip], play: i64) -> Option<&'static str> {
    let under: Vec<&&Clip> = clips
        .iter()
        .filter(|c| (c.start..c.end()).contains(&play))
        .collect();
    if under.is_empty() {
        return Some("Move the playhead over the clip");
    }
    if under.iter().all(|c| c.start == play) {
        return Some("Move the playhead past the clip's first frame");
    }
    if under
        .iter()
        .all(|c| c.start == play || speed_key_at(c, play))
    {
        return Some("Already a speed keyframe here. Move the playhead.");
    }
    None
}

pub(super) fn adds_speed_key(c: &Clip, play: i64) -> bool {
    takes_speed(c) && c.start < play && play < c.end() && !speed_key_at(c, play)
}

fn fixed(param: KeyParam) -> bool {
    param == KeyParam::Speed
}

fn key_eases(clip: &Clip, lane: &Lane, i: usize) -> (Option<Ease>, Vec<Ease>) {
    if !fixed(lane.param) {
        return (ease_of(&lane.keys, i), eases(&lane.keys, i));
    }
    match vidcut_core::ops::speed::key_ease(clip, lane.keys[i].frame) {
        Some(now) => (Some(now), vec![Ease::Linear, Ease::EaseInOut]),
        None => (None, Vec::new()),
    }
}

struct Target {
    clip: Clip,
    marks: Vec<Mark>,
    rows: Vec<KeyParam>,
    lanes: Vec<Lane>,
    play: i64,
}

impl Target {
    fn takes(&self, param: KeyParam) -> bool {
        match param {
            KeyParam::Volume => self.clip.audio.is_some(),
            KeyParam::Speed => self.clip.start < self.play && self.play < self.clip.end(),
            _ => true,
        }
    }

    fn keyed_here(&self, param: KeyParam) -> bool {
        self.lane(param).is_some_and(|l| {
            l.keys
                .iter()
                .any(|k| self.clip.key_time(k.frame).round() as i64 == self.play)
        })
    }

    fn count(&self, param: KeyParam) -> usize {
        self.lane(param).map_or(0, |l| l.keys.len())
    }

    fn lane(&self, param: KeyParam) -> Option<&Lane> {
        self.lanes.iter().find(|l| l.param == param)
    }
}

#[derive(Default)]
pub struct Editor {
    clip: Option<String>,
    selected: Vec<Mark>,
    rows: Vec<KeyParam>,
    deleting: Option<Deleting>,
    boxing: Option<Boxing>,
    seen: Vec<(Pos2, Mark)>,
    tray: bool,
    drag: Option<Drag>,
    pending: Option<Pending>,
    menu: Option<Mark>,
    menu_curve: bool,
    menu_tray: bool,
    keyed_only: bool,
    snapping: bool,
    closed: Vec<Group>,
    curves: Vec<KeyParam>,
    graph: bool,
    graph_off: Vec<KeyParam>,
    graph_active: Option<KeyParam>,
    graph_h: Option<f32>,
    view: Option<(f64, f64)>,
    held: Option<Held>,
    fresh: Option<(KeyParam, i64, Instant)>,
    grab: Option<(f64, (f64, f64))>,
    tray_area: Option<Rect>,
    panel_area: Option<Rect>,
    focused: bool,
}

impl Editor {
    pub(super) fn leave(&mut self) {
        self.drag = None;
        self.boxing = None;
    }

    pub(super) fn has_selection(&self) -> bool {
        !self.selected.is_empty()
    }

    pub(super) fn asking(&self) -> bool {
        self.deleting.is_some()
    }
}

fn group_of(param: KeyParam) -> Group {
    match param {
        KeyParam::Position
        | KeyParam::UpperPosition
        | KeyParam::LowerPosition
        | KeyParam::Zoom
        | KeyParam::UpperZoom
        | KeyParam::LowerZoom
        | KeyParam::Rotation
        | KeyParam::UpperRotation
        | KeyParam::LowerRotation
        | KeyParam::Anchor
        | KeyParam::Pitch
        | KeyParam::Yaw => Group::Transform,
        KeyParam::Volume => Group::Audio,
        KeyParam::CropLeft
        | KeyParam::CropRight
        | KeyParam::CropTop
        | KeyParam::CropBottom
        | KeyParam::CropSoftness => Group::Crop,
        KeyParam::Opacity => Group::Composite,
        KeyParam::Speed => Group::Speed,
        KeyParam::CenterX | KeyParam::CenterY => Group::Generator,
        KeyParam::Effect(q) => Group::Effect(q.effect),
    }
}

fn groups_of(clip: &Clip) -> Vec<Group> {
    let effects = clip.picture.as_ref().map_or(0, |p| p.effects.len());
    let mut out = GROUPS.to_vec();
    let at = out
        .iter()
        .position(|g| *g == Group::Speed)
        .unwrap_or(out.len());
    out.splice(at..at, (0..effects).map(Group::Effect));
    out
}

fn group_name(group: Group, clip: &Clip) -> String {
    match group {
        Group::Generator => "Generator".to_string(),
        Group::Transform => "Transform".to_string(),
        Group::Crop => "Crop".to_string(),
        Group::Composite => "Composite".to_string(),
        Group::Effect(n) => effect_name(clip, n),
        Group::Speed => "Speed".to_string(),
        Group::Audio => "Audio".to_string(),
    }
}

fn effect_name(clip: &Clip, n: usize) -> String {
    let effects = clip.picture.as_ref().map_or(&[][..], |p| &p.effects);
    let Some(e) = effects.get(n) else {
        return String::new();
    };
    let same = effects.iter().filter(|x| x.kind() == e.kind()).count();
    if same > 1 {
        format!("{} {}", e.kind().name(), n + 1)
    } else {
        e.kind().name().to_string()
    }
}

fn effect_value_name(value: EffectValue) -> String {
    if value == EffectValue::HvRatio {
        return "H vs V Ratio".to_string();
    }
    value
        .name()
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn lane_name(param: KeyParam) -> String {
    if let KeyParam::Effect(q) = param {
        return effect_value_name(q.value);
    }
    plain_lane_name(param).to_string()
}

fn plain_lane_name(param: KeyParam) -> &'static str {
    match param {
        KeyParam::Effect(_) => "",
        KeyParam::Volume => "Volume",
        KeyParam::Position => "Position",
        KeyParam::Zoom => "Zoom",
        KeyParam::Rotation => "Rotation",
        KeyParam::UpperPosition => "Upper Position",
        KeyParam::UpperZoom => "Upper Zoom",
        KeyParam::UpperRotation => "Upper Rotation",
        KeyParam::LowerPosition => "Lower Position",
        KeyParam::LowerZoom => "Lower Zoom",
        KeyParam::LowerRotation => "Lower Rotation",
        KeyParam::Anchor => "Anchor Point",
        KeyParam::Pitch => "Pitch",
        KeyParam::Yaw => "Yaw",
        KeyParam::CropLeft => "Crop Left",
        KeyParam::CropRight => "Crop Right",
        KeyParam::CropTop => "Crop Top",
        KeyParam::CropBottom => "Crop Bottom",
        KeyParam::CropSoftness => "Crop Softness",
        KeyParam::Opacity => "Opacity",
        KeyParam::Speed => "Speed",
        KeyParam::CenterX => "Center X",
        KeyParam::CenterY => "Center Y",
    }
}

fn limits(param: KeyParam) -> (f64, f64) {
    match param {
        KeyParam::Volume => (*GAIN.start(), *GAIN.end()),
        KeyParam::Position
        | KeyParam::UpperPosition
        | KeyParam::LowerPosition
        | KeyParam::Anchor => (0.5 - Picture::REACH, 0.5 + Picture::REACH),
        KeyParam::Zoom | KeyParam::UpperZoom | KeyParam::LowerZoom => Picture::ZOOM,
        KeyParam::Rotation | KeyParam::UpperRotation | KeyParam::LowerRotation => {
            (-Picture::TURNS, Picture::TURNS)
        }
        KeyParam::Pitch | KeyParam::Yaw => (-Picture::LEAN, Picture::LEAN),
        KeyParam::CropLeft | KeyParam::CropRight | KeyParam::CropTop | KeyParam::CropBottom => {
            (0.0, 1.0)
        }
        KeyParam::CropSoftness => (-Picture::SOFT, Picture::SOFT),
        KeyParam::Opacity => (0.0, 1.0),
        KeyParam::Speed => (*Speed::RANGE.start(), *Speed::RANGE.end()),
        KeyParam::CenterX | KeyParam::CenterY => (*CENTER.start(), *CENTER.end()),
        KeyParam::Effect(_) => (f64::MIN, f64::MAX),
    }
}

fn quantize(param: KeyParam, d: f64) -> f64 {
    let places = match param {
        KeyParam::Volume => 1,
        KeyParam::Rotation
        | KeyParam::UpperRotation
        | KeyParam::LowerRotation
        | KeyParam::CropSoftness => 2,
        _ => 6,
    };
    let k = 10f64.powi(places);
    (d * k).round() / k
}

fn least_span(param: KeyParam) -> f64 {
    match param {
        KeyParam::Volume => 6.0,
        KeyParam::Zoom | KeyParam::UpperZoom | KeyParam::LowerZoom => 0.1,
        KeyParam::Rotation
        | KeyParam::UpperRotation
        | KeyParam::LowerRotation
        | KeyParam::CropSoftness => 10.0,
        KeyParam::Pitch | KeyParam::Yaw => 0.1,
        _ => 0.05,
    }
}

fn shown_value(param: KeyParam, v: f64, y: bool, f: &Frame) -> f64 {
    let ([w, h], [fw, fh]) = (f.frame, f.fitted);
    match param {
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition if y => {
            (0.5 - v) * h
        }
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition => (v - 0.5) * w,
        KeyParam::Anchor if y => (0.5 - v) * fh,
        KeyParam::Anchor => (v - 0.5) * fw,
        KeyParam::Zoom
        | KeyParam::UpperZoom
        | KeyParam::LowerZoom
        | KeyParam::Opacity
        | KeyParam::Speed => v * 100.0,
        KeyParam::CropLeft | KeyParam::CropRight => v * fw,
        KeyParam::CropTop | KeyParam::CropBottom => v * fh,
        KeyParam::Volume
        | KeyParam::Rotation
        | KeyParam::UpperRotation
        | KeyParam::LowerRotation
        | KeyParam::Pitch
        | KeyParam::Yaw
        | KeyParam::CropSoftness
        | KeyParam::CenterX
        | KeyParam::CenterY
        | KeyParam::Effect(_) => v,
    }
}

fn stored_value(param: KeyParam, v: f64, y: bool, f: &Frame) -> f64 {
    let ([w, h], [fw, fh]) = (f.frame, f.fitted);
    match param {
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition if y => 0.5 - v / h,
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition => v / w + 0.5,
        KeyParam::Anchor if y => 0.5 - v / fh,
        KeyParam::Anchor => v / fw + 0.5,
        KeyParam::Zoom
        | KeyParam::UpperZoom
        | KeyParam::LowerZoom
        | KeyParam::Opacity
        | KeyParam::Speed => v / 100.0,
        KeyParam::CropLeft | KeyParam::CropRight => v / fw,
        KeyParam::CropTop | KeyParam::CropBottom => v / fh,
        KeyParam::Volume
        | KeyParam::Rotation
        | KeyParam::UpperRotation
        | KeyParam::LowerRotation
        | KeyParam::Pitch
        | KeyParam::Yaw
        | KeyParam::CropSoftness
        | KeyParam::CenterX
        | KeyParam::CenterY
        | KeyParam::Effect(_) => v,
    }
}

fn unit(param: KeyParam) -> &'static str {
    match param {
        KeyParam::Volume => " dB",
        KeyParam::Zoom
        | KeyParam::UpperZoom
        | KeyParam::LowerZoom
        | KeyParam::Opacity
        | KeyParam::Speed => "%",
        KeyParam::Rotation | KeyParam::UpperRotation | KeyParam::LowerRotation => "°",
        _ => "",
    }
}

fn places(param: KeyParam) -> usize {
    match param {
        KeyParam::Volume
        | KeyParam::Zoom
        | KeyParam::UpperZoom
        | KeyParam::LowerZoom
        | KeyParam::Opacity => 1,
        KeyParam::Rotation
        | KeyParam::UpperRotation
        | KeyParam::LowerRotation
        | KeyParam::CropSoftness
        | KeyParam::Speed => 2,
        KeyParam::Pitch
        | KeyParam::Yaw
        | KeyParam::CenterX
        | KeyParam::CenterY
        | KeyParam::Effect(_) => 3,
        _ => 0,
    }
}

fn value_text(param: KeyParam, v: f64, f: &Frame) -> String {
    let n = places(param);
    format!("{:.n$}{}", shown_value(param, v, false, f), unit(param))
}

fn tick_step(ppf: f32, fps: f64) -> i64 {
    let need = f64::from(TICK_GAP / ppf.max(1e-3));
    let fps = fps.max(1.0);
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .chain(
            [1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 3600.0]
                .into_iter()
                .map(|s| (s * fps).round()),
        )
        .find(|&s| s >= need)
        .unwrap_or(3600.0 * fps) as i64
}

fn paired(param: KeyParam) -> bool {
    matches!(
        param,
        KeyParam::Position
            | KeyParam::UpperPosition
            | KeyParam::LowerPosition
            | KeyParam::Zoom
            | KeyParam::UpperZoom
            | KeyParam::LowerZoom
            | KeyParam::Anchor
    )
}

fn linear(clip: &Clip, param: KeyParam) -> bool {
    param == KeyParam::Anchor
        || (matches!(
            param,
            KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition
        ) && clip.title.is_some())
}

fn limits_of(clip: &Clip, param: KeyParam, axis: Axis) -> (f64, f64) {
    match param {
        KeyParam::Zoom | KeyParam::UpperZoom | KeyParam::LowerZoom if clip.title.is_some() => {
            (0.25, 4.0)
        }
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition
            if clip.title.is_some() && axis == Axis::Y =>
        {
            Title::DOWN
        }
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition
            if clip.title.is_some() =>
        {
            Title::ACROSS
        }
        KeyParam::Effect(q) => clip
            .picture
            .as_ref()
            .and_then(|p| p.effects.get(q.effect))
            .and_then(|e| e.range_of(q.value))
            .unwrap_or_else(|| limits(param)),
        _ => limits(param),
    }
}

fn nudge_step(param: KeyParam, sign: f64, y: bool) -> f64 {
    let step = match param {
        KeyParam::Zoom
        | KeyParam::UpperZoom
        | KeyParam::LowerZoom
        | KeyParam::Volume
        | KeyParam::CropSoftness => 0.2,
        KeyParam::Rotation | KeyParam::UpperRotation | KeyParam::LowerRotation => 0.02,
        KeyParam::Opacity => 0.002,
        KeyParam::Speed => 0.01,
        KeyParam::Position
        | KeyParam::UpperPosition
        | KeyParam::LowerPosition
        | KeyParam::Anchor
        | KeyParam::Pitch
        | KeyParam::Yaw
        | KeyParam::CropLeft
        | KeyParam::CropRight
        | KeyParam::CropTop
        | KeyParam::CropBottom
        | KeyParam::CenterX
        | KeyParam::CenterY
        | KeyParam::Effect(_) => 0.001,
    };
    let down = matches!(
        param,
        KeyParam::Position | KeyParam::UpperPosition | KeyParam::LowerPosition | KeyParam::Anchor
    ) && y;
    if down { -step * sign } else { step * sign }
}

fn choose(selected: &[Mark], hit: Mark, toggle: bool) -> Vec<Mark> {
    if !toggle {
        return vec![hit];
    }
    let mut out = selected.to_vec();
    match out.iter().position(|m| *m == hit) {
        Some(i) => {
            out.remove(i);
        }
        None => out.push(hit),
    }
    out
}

fn dragged(selected: &[Mark], hit: Mark) -> Vec<Mark> {
    if selected.contains(&hit) {
        selected
            .iter()
            .copied()
            .filter(|m| m.param == hit.param)
            .collect()
    } else {
        vec![hit]
    }
}

fn frames_moved(dx: f32, ppf: f32) -> i64 {
    if ppf <= f32::EPSILON {
        0
    } else {
        (f64::from(dx) / f64::from(ppf)).round() as i64
    }
}

fn fits(keys: &[Keyframe], moving: &[i64], dt: i64) -> bool {
    let shifted = |f: i64| f + dt;
    let still: Vec<i64> = keys
        .iter()
        .map(|k| k.frame)
        .filter(|f| !moving.contains(f))
        .collect();
    let mut new: Vec<i64> = moving.iter().copied().map(shifted).collect();
    let ordered = moving.iter().zip(&new).all(|(&f, &n)| {
        let lo = still.iter().copied().filter(|&s| s < f).max();
        let hi = still.iter().copied().filter(|&s| s > f).min();
        lo.is_none_or(|l| l < n) && hi.is_none_or(|h| n < h)
    });
    new.sort_unstable();
    ordered && new.windows(2).all(|w| w[0] < w[1])
}

fn marks_fit(lanes: &[Lane], marks: &[Mark], dt: i64) -> bool {
    unique_params(marks).into_iter().all(|param| {
        let Some(lane) = lanes.iter().find(|l| l.param == param) else {
            return false;
        };
        let moving: Vec<i64> = marks
            .iter()
            .filter(|m| m.param == param)
            .map(|m| m.frame)
            .collect();
        fits(&lane.keys, &moving, dt)
    })
}

fn fitted(lanes: &[Lane], marks: &[Mark], by: i64) -> i64 {
    if by == 0 || marks_fit(lanes, marks, by) {
        return by;
    }
    let sign = by.signum();
    let (mut lo, mut hi) = (0, by.abs());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if marks_fit(lanes, marks, mid * sign) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo * sign
}

fn handle_of(frame: i64, value: f64, neighbor: i64, out: bool, source: f64, curve: f64) -> Handle {
    let len = (neighbor - frame).abs() as f64;
    let raw = source - frame as f64;
    let frames = if out {
        raw.clamp(0.0, len)
    } else {
        raw.clamp(-len, 0.0)
    };
    Handle {
        frames: (frames * 100.0).round() / 100.0,
        value: ((curve - value) * 1e6).round() / 1e6,
    }
}

fn group_at(items: &[(KeyParam, i64, i64)], at: i64) -> Vec<Mark> {
    items
        .iter()
        .filter(|(_, _, t)| *t == at)
        .map(|(param, frame, _)| Mark {
            param: *param,
            frame: *frame,
        })
        .collect()
}

fn unique_params(marks: &[Mark]) -> Vec<KeyParam> {
    let mut out = Vec::new();
    for m in marks {
        if !out.contains(&m.param) {
            out.push(m.param);
        }
    }
    out
}

fn frames_of(marks: &[Mark], param: KeyParam) -> Vec<i64> {
    marks
        .iter()
        .filter(|m| m.param == param)
        .map(|m| m.frame)
        .collect()
}

fn by_value(clip: &Clip, param: KeyParam, axis: Axis, d: f64) -> Value {
    match param {
        KeyParam::Zoom | KeyParam::UpperZoom | KeyParam::LowerZoom if clip.title.is_some() => {
            json!(d)
        }
        KeyParam::Position
        | KeyParam::UpperPosition
        | KeyParam::LowerPosition
        | KeyParam::Zoom
        | KeyParam::UpperZoom
        | KeyParam::LowerZoom
        | KeyParam::Anchor => match axis {
            Axis::Y => json!([0.0, d]),
            _ => json!([d, 0.0]),
        },
        _ => json!(d),
    }
}

fn by_body(id: &str, param: KeyParam, frames: &[i64], by: Value) -> Value {
    json!({ "clips": [id], "param": param, "frames": frames, "by": by })
}

fn move_body(id: &str, param: KeyParam, frames: &[i64], by: i64) -> Value {
    json!({ "clips": [id], "param": param, "frames": frames, "by": by })
}

fn ease_body(id: &str, param: KeyParam, frames: &[i64], ease: Ease) -> Value {
    json!({ "clips": [id], "param": param, "frames": frames, "ease": ease })
}

fn takes_ease(clip: &Clip, lane: &Lane, i: usize, ease: Ease) -> bool {
    if fixed(lane.param) {
        return key_eases(clip, lane, i).1.contains(&ease);
    }
    let (first, last) = (i == 0, i + 1 == lane.keys.len());
    match ease {
        Ease::Linear => true,
        Ease::EaseIn => !first,
        Ease::EaseOut => !last,
        Ease::EaseInOut => !first || !last,
    }
}

fn eased_keys<'a>(clip: &Clip, lanes: &'a [Lane], marks: &[Mark]) -> Vec<(&'a Lane, usize)> {
    marks
        .iter()
        .filter(|m| !linear(clip, m.param))
        .filter_map(|m| {
            let lane = lanes.iter().find(|l| l.param == m.param)?;
            Some((lane, lane.keys.iter().position(|k| k.frame == m.frame)?))
        })
        .collect()
}

fn marks_eases(clip: &Clip, lanes: &[Lane], marks: &[Mark]) -> (Option<Ease>, Vec<Ease>) {
    let keys = eased_keys(clip, lanes, marks);
    if let [(lane, i)] = keys[..] {
        return key_eases(clip, lane, i);
    }
    let mut nows = keys.iter().map(|(lane, i)| key_eases(clip, lane, *i).0);
    let first = nows.next().flatten();
    let now = first.filter(|_| nows.all(|e| e == first));
    let options = Ease::ALL
        .into_iter()
        .filter(|&e| keys.iter().any(|(lane, i)| takes_ease(clip, lane, *i, e)))
        .collect();
    (now, options)
}

fn ease_parts(
    clip: &Clip,
    lanes: &[Lane],
    marks: &[Mark],
    ease: Ease,
) -> Vec<(&'static str, Value)> {
    let keys = eased_keys(clip, lanes, marks);
    unique_params(marks)
        .into_iter()
        .filter_map(|param| {
            let frames: Vec<i64> = keys
                .iter()
                .filter(|(lane, i)| lane.param == param && takes_ease(clip, lane, *i, ease))
                .map(|(lane, i)| lane.keys[*i].frame)
                .collect();
            (!frames.is_empty())
                .then(|| ("keyframes.set", ease_body(&clip.id, param, &frames, ease)))
        })
        .collect()
}

fn locked(now: Option<Lock>, shift: bool, moved: Vec2) -> Option<Lock> {
    if !shift {
        return None;
    }
    now.or(
        (moved != Vec2::ZERO).then_some(if moved.x.abs() >= moved.y.abs() {
            Lock::Time
        } else {
            Lock::Value
        }),
    )
}

fn snapped(
    clip: &Clip,
    lanes: &[Lane],
    marks: &[Mark],
    by: i64,
    play: i64,
    ppf: f32,
) -> Option<(i64, i64)> {
    let reach = f64::from(SNAP_PX / ppf.max(1e-3));
    let mut best: Option<(i64, i64, i64)> = None;
    for m in marks {
        let from = clip.key_time(m.frame);
        let others = lanes
            .iter()
            .filter(|l| l.param != m.param)
            .flat_map(|l| l.keys.iter().map(move |k| (l.param, k.frame)))
            .filter(|(param, frame)| {
                !marks.contains(&Mark {
                    param: *param,
                    frame: *frame,
                })
            })
            .map(|(_, frame)| clip.key_time(frame));
        for to in std::iter::once(play as f64).chain(others) {
            let wanted = (to - from).round() as i64;
            let gap = (wanted - by).abs();
            if gap as f64 <= reach && best.is_none_or(|b| gap < b.0) {
                best = Some((gap, wanted, to.round() as i64));
            }
        }
    }
    best.map(|(_, by, at)| (by, at))
}

fn handle_body(id: &str, mark: Mark, out: bool, handle: Handle) -> Value {
    let mut v = json!({ "clips": [id], "param": mark.param, "frames": [mark.frame] });
    v[if out { "out" } else { "in" }] = json!(handle);
    v
}

fn d_value(span: f64, height: f32, dy: f32) -> f64 {
    if height <= 1.0 {
        0.0
    } else {
        -f64::from(dy) / f64::from(height) * span
    }
}

fn clamped_delta(clip: &Clip, lane: &Lane, marks: &[Mark], axis: Axis, raw: f64) -> f64 {
    if axis == Axis::None {
        return 0.0;
    }
    let (lo, hi) = limits_of(clip, lane.param, axis);
    let mut d = raw;
    for m in marks.iter().filter(|m| m.param == lane.param) {
        let Some(i) = lane.keys.iter().position(|k| k.frame == m.frame) else {
            continue;
        };
        let start = match axis {
            Axis::Y => lane.extra.get(i).copied().unwrap_or(0.0),
            _ => lane.keys[i].value,
        };
        d = d.clamp(lo - start, hi - start);
    }
    let d = quantize(lane.param, d);
    let mut out = d;
    for m in marks.iter().filter(|m| m.param == lane.param) {
        let Some(i) = lane.keys.iter().position(|k| k.frame == m.frame) else {
            continue;
        };
        let start = match axis {
            Axis::Y => lane.extra.get(i).copied().unwrap_or(0.0),
            _ => lane.keys[i].value,
        };
        out = out.clamp(lo - start, hi - start);
    }
    if out.abs() < 1e-9 { 0.0 } else { out }
}

fn moved_handle(
    clip: &Clip,
    fix: HandleFix,
    dx: f32,
    dy: f32,
    ppf: f32,
    span: f64,
    height: f32,
) -> Handle {
    let origin_t = clip.time_of(fix.mark.frame as f64 + fix.frames);
    let new_t = origin_t + f64::from(dx) / f64::from(ppf.max(0.001));
    let source = clip.key_at(new_t);
    let curve = fix.key_value + fix.value + d_value(span, height, dy);
    handle_of(
        fix.mark.frame,
        fix.key_value,
        fix.neighbor,
        fix.out,
        source,
        curve,
    )
}

fn retarget(marks: &mut [Mark], moved: &[Mark], dt: i64) {
    if dt == 0 {
        return;
    }
    for m in marks {
        if moved.contains(m) {
            m.frame += dt;
        }
    }
}

impl TimeMap {
    fn of(lo: f64, hi: f64, rect: Rect) -> Self {
        Self {
            lo,
            hi: hi.max(lo + 1.0),
            left: rect.left(),
            width: rect.width().max(1.0),
        }
    }

    fn x(&self, t: f64) -> f32 {
        let u = ((t - self.lo) / (self.hi - self.lo)) as f32;
        self.left + u * self.width
    }

    fn time(&self, x: f32) -> f64 {
        self.lo + f64::from((x - self.left) / self.width) * (self.hi - self.lo)
    }

    fn ppf(&self) -> f32 {
        self.width / (self.hi - self.lo) as f32
    }
}

impl ValueMap {
    fn of(lo: f64, hi: f64, least: f64, rect: Rect) -> Self {
        let mid = (lo + hi) / 2.0;
        let half = ((hi - lo).max(least) / 2.0) * 1.15;
        let (lo, hi) = (mid - half, mid + half);
        Self {
            lo,
            hi,
            top: rect.top(),
            height: rect.height().max(1.0),
        }
    }

    fn y(&self, v: f64) -> f32 {
        let u = ((self.hi - v) / (self.hi - self.lo)) as f32;
        self.top + u.clamp(0.0, 1.0) * self.height
    }

    fn span(&self) -> f64 {
        self.hi - self.lo
    }
}

fn value_ends(rows: &[Shown]) -> (f64, f64) {
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    let push = |lo: &mut f64, hi: &mut f64, v: f64| {
        *lo = lo.min(v);
        *hi = hi.max(v);
    };
    for row in rows {
        push(&mut lo, &mut hi, row.key.value);
        if row.paired {
            push(&mut lo, &mut hi, row.y);
        }
        for h in [row.key.in_handle, row.key.out_handle]
            .into_iter()
            .flatten()
        {
            push(&mut lo, &mut hi, row.key.value + h.value);
        }
    }
    if !lo.is_finite() {
        (0.0, 1.0)
    } else {
        (lo, hi)
    }
}

fn clip_span(clip: &Clip) -> (f64, f64) {
    let lo = clip.start as f64;
    (lo, (clip.end() as f64).max(lo + 1.0))
}

fn clip_band(map: TimeMap, clip: &Clip, rect: Rect) -> Rect {
    let a = map.x(clip.start as f64).max(rect.left());
    let b = map.x(clip.end() as f64).min(rect.right());
    Rect::from_x_y_ranges(a..=b.max(a), rect.y_range())
}

fn lanes_of(clip: &Clip, keyed_only: bool) -> Vec<Lane> {
    let mut out = Vec::new();
    if let Some(Generator::FourColorGradient(g)) = &clip.generator {
        for (param, axis) in [
            (KeyParam::CenterX, generators::Axis::X),
            (KeyParam::CenterY, generators::Axis::Y),
        ] {
            let keys = g.keys(axis);
            if !keyed_only || !keys.is_empty() {
                out.push(Lane {
                    param,
                    keys: keys.to_vec(),
                    extra: Vec::new(),
                });
            }
        }
    }
    if has_picture(clip) {
        let picture = clip.picture.clone().unwrap_or_default();
        for v in PictureValue::ALL {
            if v == PictureValue::Anchor && is_cue(clip) {
                continue;
            }
            if !keyed_only || picture.has_keys(v) {
                out.push(lane_from(&picture, v));
            }
        }
        for (effect, e) in picture.effects.iter().enumerate() {
            for value in e.values() {
                let keys = e.keys_of(value);
                if !keyed_only || !keys.is_empty() {
                    out.push(Lane {
                        param: KeyParam::Effect(EffectParam { effect, value }),
                        keys: keys.to_vec(),
                        extra: Vec::new(),
                    });
                }
            }
        }
    }
    if let Some(t) = &clip.title {
        let high = t.upper_view();
        let lines = match &t.upper {
            Some(_) => vec![
                (&high, Some(LowerThirdLine::Upper)),
                (t, Some(LowerThirdLine::Lower)),
            ],
            None => vec![(t, None)],
        };
        for (view, line) in lines {
            let picture = view.as_picture();
            for v in PictureValue::ALL {
                let whole = v == PictureValue::Opacity && line == Some(LowerThirdLine::Upper);
                if !whole && view.takes_keys(v) && (!keyed_only || picture.has_keys(v)) {
                    let mut lane = lane_from(&picture, v);
                    if v == PictureValue::Zoom {
                        lane.extra.clear();
                    }
                    if let Some(line) = line {
                        lane.param = lane.param.for_line(line);
                    }
                    out.push(lane);
                }
            }
        }
    }
    let speed = speed_keys(clip);
    if takes_speed(clip) && (!keyed_only || !speed.is_empty()) {
        out.push(Lane {
            param: KeyParam::Speed,
            keys: speed,
            extra: Vec::new(),
        });
    }
    if let Some(audio) = clip.audio.as_ref().filter(|a| !keyed_only || a.is_keyed()) {
        out.push(Lane {
            param: KeyParam::Volume,
            keys: audio.gain_keys.clone(),
            extra: Vec::new(),
        });
    }
    out
}

fn lane_from(picture: &Picture, v: PictureValue) -> Lane {
    let param = match v {
        PictureValue::Position => KeyParam::Position,
        PictureValue::Zoom => KeyParam::Zoom,
        PictureValue::Rotation => KeyParam::Rotation,
        PictureValue::Anchor => KeyParam::Anchor,
        PictureValue::Pitch => KeyParam::Pitch,
        PictureValue::Yaw => KeyParam::Yaw,
        PictureValue::CropLeft => KeyParam::CropLeft,
        PictureValue::CropRight => KeyParam::CropRight,
        PictureValue::CropTop => KeyParam::CropTop,
        PictureValue::CropBottom => KeyParam::CropBottom,
        PictureValue::CropSoftness => KeyParam::CropSoftness,
        PictureValue::Opacity => KeyParam::Opacity,
    };
    let extra = match v {
        PictureValue::Position => picture.position_y.clone(),
        PictureValue::Zoom => picture.zoom_y.clone(),
        PictureValue::Anchor => picture.anchor_y.clone(),
        _ => Vec::new(),
    };
    Lane {
        param,
        keys: picture.keys_of(v).to_vec(),
        extra,
    }
}

fn lane_items(clip: &Clip, lanes: &[&Lane]) -> Vec<(KeyParam, i64, i64)> {
    lanes
        .iter()
        .flat_map(|lane| {
            lane.keys
                .iter()
                .map(|k| (lane.param, k.frame, clip.key_time(k.frame).round() as i64))
        })
        .collect()
}

fn shown_lane(lane: &Lane, shift: Option<&Shift>) -> Vec<Shown> {
    let paired = paired(lane.param) && lane.extra.len() == lane.keys.len();
    let extra: Vec<f64> = if paired {
        lane.extra.clone()
    } else {
        vec![0.0; lane.keys.len()]
    };
    let mut rows: Vec<Shown> = lane
        .keys
        .iter()
        .zip(&extra)
        .map(|(k, y)| Shown {
            from: k.frame,
            key: *k,
            y: *y,
            paired,
        })
        .collect();
    let Some(shift) = shift else {
        return rows;
    };
    for edit in shift.handles.iter().filter(|e| e.mark.param == lane.param) {
        if let Some(row) = rows.iter_mut().find(|r| r.from == edit.mark.frame) {
            if edit.out {
                row.key.out_handle = Some(edit.handle);
            } else {
                row.key.in_handle = Some(edit.handle);
            }
        }
    }
    for row in &mut rows {
        let hit = shift
            .marks
            .iter()
            .any(|m| m.param == lane.param && m.frame == row.from);
        if !hit {
            continue;
        }
        match shift.axis {
            Axis::X => row.key.value += shift.dv,
            Axis::Y => row.y += shift.dv,
            Axis::None => {}
        }
        if shift.dt != 0 {
            row.key.frame += shift.dt;
        }
    }
    rows.sort_by_key(|r| r.key.frame);
    rows
}

fn pressed_at(ui: &egui::Ui, resp: &egui::Response, pos: Pos2) -> Pos2 {
    if resp.drag_started() {
        ui.input(|i| i.pointer.press_origin()).unwrap_or(pos)
    } else {
        pos
    }
}

fn near(a: Pos2, b: Pos2) -> bool {
    a.distance(b) <= HIT
}

fn closest<T>(items: &[T], pos: Pos2, at: impl Fn(&T) -> Pos2) -> Option<&T> {
    items
        .iter()
        .filter(|item| near(at(item), pos))
        .min_by(|a, b| at(a).distance(pos).total_cmp(&at(b).distance(pos)))
}

fn diamond(painter: &egui::Painter, at: Pos2, r: f32, color: Color32) {
    painter.add(Shape::convex_polygon(
        vec![
            pos2(at.x, at.y - r),
            pos2(at.x + r, at.y),
            pos2(at.x, at.y + r),
            pos2(at.x - r, at.y),
        ],
        color,
        Stroke::NONE,
    ));
}

fn dot(painter: &egui::Painter, at: Pos2, r: f32, color: Color32) {
    painter.circle_filled(at, r, color);
}

fn edge_arrows(painter: &egui::Painter, rect: Rect, times: &[f64], map: TimeMap) {
    let y = rect.center().y;
    let color = theme::colors().text_dim;
    if times.iter().any(|&t| t < map.lo) {
        let x = rect.left() + 2.0;
        painter.add(Shape::convex_polygon(
            vec![pos2(x, y), pos2(x + 6.0, y - 4.0), pos2(x + 6.0, y + 4.0)],
            color,
            Stroke::NONE,
        ));
    }
    if times.iter().any(|&t| t > map.hi) {
        let x = rect.right() - 2.0;
        painter.add(Shape::convex_polygon(
            vec![pos2(x, y), pos2(x - 6.0, y + 4.0), pos2(x - 6.0, y - 4.0)],
            color,
            Stroke::NONE,
        ));
    }
}

fn time_line(painter: &egui::Painter, map: TimeMap, rect: Rect, at: i64, color: Color32) {
    let t = at as f64;
    if (map.lo..=map.hi).contains(&t) {
        let x = map.x(t);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, color),
        );
    }
}

fn playhead(painter: &egui::Painter, map: TimeMap, rect: Rect, plot: Plot) {
    time_line(painter, map, rect, plot.play, theme::colors().playhead);
    if let Some(at) = plot.snap {
        time_line(painter, map, rect, at, Color32::WHITE);
    }
}

impl Workspace {
    fn first_clip(&self, p: &Project) -> Option<Clip> {
        let order = &self.snap.selection.order;
        order.iter().find_map(|id| p.key_holder(id))
    }

    fn editor_clip(&mut self, p: &Project, tray: bool) -> Option<Clip> {
        let clip = self.first_clip(p)?;
        self.key_editor.tray = tray;
        self.key_editor.seen.clear();
        if self.key_editor.clip.as_deref() != Some(clip.id.as_str()) {
            self.key_editor.clip = Some(clip.id.clone());
            self.key_editor.selected.clear();
            self.key_editor.rows.clear();
            self.key_editor.boxing = None;
            self.key_editor.drag = None;
            self.key_editor.pending = None;
            self.key_editor.menu = None;
            self.key_editor.menu_curve = false;
            self.key_editor.graph_active = None;
            self.key_editor.view = None;
            self.key_editor.held = None;
            self.key_editor.fresh = None;
            self.key_editor.grab = None;
        }
        Some(clip)
    }

    pub(super) fn tray_height(&self, ui: &egui::Ui) -> f32 {
        if !self.key_tray {
            return 0.0;
        }
        let rows = self
            .project_shared()
            .and_then(|p| self.first_clip(&p))
            .map_or(0, |c| lanes_of(&c, true).len());
        let gap = ui.spacing().item_spacing.y;
        rows.clamp(1, TRAY_ROWS) as f32 * (TRACK_H + gap) + gap
    }

    pub(super) fn keyframe_tray(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        label: f32,
        (lo, hi): (f64, f64),
    ) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let ui = &mut child;
        self.key_editor.tray_area = Some(rect);
        ui.painter()
            .rect_filled(rect, 2.0, theme::colors().panel_dark);
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(clip) = self.editor_clip(&p, true) else {
            ui.label(dim(SELECT_ONE));
            return;
        };
        let lanes = lanes_of(&clip, true);
        if lanes.is_empty() {
            ui.label(dim("No Keyframes"));
            return;
        }
        self.select_fresh(&clip, &lanes);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.key_editor.drag = None;
            self.key_editor.boxing = None;
        }
        self.expire_pending(&clip, p.rev);
        let shift = self.shown_shift(ui, &clip, &lanes);
        let gap = ui.spacing().item_spacing.x;
        let plot = Plot {
            lo,
            hi,
            label: label - gap,
            width: (rect.width() - label).max(48.0),
            play: self.snap.timeline.position,
            snap: shift.as_ref().and_then(|s| s.snap),
            tray: true,
        };
        egui::ScrollArea::vertical()
            .id_salt("keyframe-tray")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for lane in &lanes {
                    let rows = shown_lane(lane, shift.as_ref());
                    self.track_row(ui, &clip, lane, &rows, plot);
                }
            });
        self.box_select(ui, rect, &clip);
        let live = live_marks(&lanes, shift.as_ref());
        self.key_editor.selected.retain(|m| live.contains(m));
        self.finish_drag(ui, &clip, &lanes, p.rev);
    }

    pub(super) fn keyframe_panel(&mut self, ui: &mut egui::Ui) {
        self.key_editor.panel_area = Some(ui.max_rect());
        let Some(p) = self.project_shared() else {
            ui.label(dim(SELECT_ONE));
            return;
        };
        let Some(clip) = self.editor_clip(&p, false) else {
            ui.label(dim(SELECT_ONE));
            return;
        };
        let lanes = lanes_of(&clip, self.key_editor.keyed_only);
        self.select_fresh(&clip, &lanes);
        let rev = p.rev;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.key_editor.drag = None;
            self.key_editor.boxing = None;
        }
        self.expire_pending(&clip, rev);
        let play = self.snap.timeline.position;
        let view = self.key_editor.view.unwrap_or_else(|| clip_span(&clip));
        self.key_header(ui, &p, &clip, view, play);
        if lanes.is_empty() {
            ui.label(dim("No Keyframes"));
            return;
        }
        let shift = self.shown_shift(ui, &clip, &lanes);
        let (lo, hi) = self.key_editor.view.unwrap_or_else(|| clip_span(&clip));
        let frame = shape(&p, &clip);
        let area = ui.available_rect_before_wrap();
        let width = (area.width() - LABEL - ui.spacing().item_spacing.x).max(48.0);
        let plot = Plot {
            lo,
            hi,
            label: LABEL,
            width,
            play,
            snap: shift.as_ref().and_then(|s| s.snap),
            tray: false,
        };
        self.ruler(ui, &p, &clip, plot);
        let graph = self.key_editor.graph;
        if graph {
            self.curve_graph(ui, &clip, &lanes, plot, &frame, shift.as_ref());
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(
                (ui.available_height() - BAR_H - SCROLL_H - ui.spacing().item_spacing.y)
                    .max(TRACK_H),
            )
            .show(ui, |ui| {
                for group in groups_of(&clip) {
                    let members: Vec<&Lane> = lanes
                        .iter()
                        .filter(|l| group_of(l.param) == group)
                        .collect();
                    if members.is_empty() {
                        continue;
                    }
                    self.group_row(ui, &clip, &members, plot, shift.as_ref());
                    if self.key_editor.closed.contains(&group) {
                        continue;
                    }
                    for lane in &members {
                        let rows = shown_lane(lane, shift.as_ref());
                        self.track_row(ui, &clip, lane, &rows, plot);
                        if !graph
                            && !lane.keys.is_empty()
                            && self.key_editor.curves.contains(&lane.param)
                        {
                            self.curve_row(ui, &clip, lane, &rows, plot, &frame);
                        }
                    }
                }
            });
        self.box_select(ui, area, &clip);
        let live = live_marks(&lanes, shift.as_ref());
        self.key_editor.selected.retain(|m| live.contains(m));
        self.key_scroll(ui, &clip, &lanes, plot);
        self.key_bar(ui, &p, &clip, &lanes, &frame);
        self.zoom_view(ui, area, &clip, plot);
        self.finish_drag(ui, &clip, &lanes, rev);
    }

    fn select_fresh(&mut self, clip: &Clip, lanes: &[Lane]) {
        let Some((param, at, since)) = self.key_editor.fresh else {
            return;
        };
        let found = lanes
            .iter()
            .filter(|l| l.param == param)
            .flat_map(|l| &l.keys)
            .find(|k| clip.key_time(k.frame).round() as i64 == at);
        if let Some(k) = found {
            self.key_editor.selected = vec![Mark {
                param,
                frame: k.frame,
            }];
            self.key_editor.fresh = None;
        } else if since.elapsed() >= Duration::from_secs(2) {
            self.key_editor.fresh = None;
        }
    }

    fn keys_in_view(&self) -> bool {
        self.left == LeftTab::Keyframes || self.key_tray
    }

    pub(super) fn key_focus(&mut self, ctx: &egui::Context) {
        let Some(at) = ctx.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()))
        else {
            return;
        };
        let tray = self.key_editor.tray_area.filter(|_| self.key_tray);
        let panel = self
            .key_editor
            .panel_area
            .filter(|_| self.left == LeftTab::Keyframes);
        self.key_editor.focused =
            at.is_some_and(|at| tray.into_iter().chain(panel).any(|r| r.contains(at)));
    }

    pub(super) fn copy_editor_keys(&mut self, cut: bool) -> bool {
        if !self.key_editor.focused || self.key_editor.selected.is_empty() {
            return false;
        }
        let Some(id) = self.key_editor.clip.clone() else {
            return false;
        };
        let marks = &self.key_editor.selected;
        let keys: Vec<Value> = unique_params(marks)
            .into_iter()
            .filter(|&param| !fixed(param))
            .map(|param| json!({ "param": param, "frames": frames_of(marks, param) }))
            .collect();
        if keys.is_empty() {
            return true;
        }
        let method = if cut {
            self.key_editor.selected.retain(|m| fixed(m.param));
            "keyframes.cut"
        } else {
            "keyframes.copy"
        };
        self.send(method, json!({ "clip": id, "keys": keys }));
        true
    }

    pub(super) fn paste_editor_keys(&mut self) -> bool {
        if !self.key_editor.focused {
            return self.snap.key_clipboard > 0;
        }
        if self.snap.key_clipboard == 0 {
            return true;
        }
        let Some(p) = self.project_shared() else {
            return true;
        };
        let selection = &self.snap.selection;
        let clips: Vec<&String> = selection
            .clips
            .iter()
            .chain(&selection.cues)
            .filter(|id| p.key_holder(id).is_some())
            .collect();
        self.send("keyframes.paste", json!({ "clips": clips }));
        true
    }

    pub(super) fn nudge_keys(&mut self, step: Nudge) {
        let Some(p) = self.project_shared() else {
            return;
        };
        let Some(clip) = self
            .key_editor
            .clip
            .as_deref()
            .and_then(|id| p.key_holder(id))
        else {
            return;
        };
        let marks: Vec<Mark> = self
            .key_editor
            .selected
            .iter()
            .copied()
            .filter(|m| !fixed(m.param))
            .collect();
        if marks.is_empty() {
            return;
        }
        let lanes = lanes_of(&clip, false);
        let by = match step {
            Nudge::Left => -1,
            Nudge::Right => 1,
            Nudge::Up | Nudge::Down => 0,
        };
        if by != 0 {
            let dt = fitted(&lanes, &marks, by);
            if dt != 0 {
                let shift = Shift {
                    marks,
                    dt,
                    dv: 0.0,
                    axis: Axis::None,
                    handles: Vec::new(),
                    snap: None,
                };
                self.commit(&clip, p.rev, &shift);
            }
            return;
        }
        let sign = if step == Nudge::Up { 1.0 } else { -1.0 };
        let mut parts = Vec::new();
        for param in unique_params(&marks) {
            let Some(lane) = lanes.iter().find(|l| l.param == param) else {
                continue;
            };
            let d = |y: bool| {
                let axis = if y { Axis::Y } else { Axis::X };
                clamped_delta(&clip, lane, &marks, axis, nudge_step(param, sign, y))
            };
            let by = if paired(param) && lane.extra.len() == lane.keys.len() {
                let (x, y) = (d(false), d(true));
                if x == 0.0 && y == 0.0 {
                    continue;
                }
                json!([x, y])
            } else {
                let x = d(false);
                if x == 0.0 {
                    continue;
                }
                json!(x)
            };
            let frames = frames_of(&marks, param);
            parts.push(("keyframes.set", by_body(&clip.id, param, &frames, by)));
        }
        send_parts(self, parts);
    }

    fn start_box(&mut self, ui: &egui::Ui, clip: &Clip, pos: Pos2) {
        let origin = ui.input(|i| i.pointer.press_origin().unwrap_or(pos));
        self.key_editor.boxing = Some(Boxing {
            clip: clip.id.clone(),
            origin,
            tray: self.key_editor.tray,
        });
    }

    fn box_select(&mut self, ui: &egui::Ui, area: Rect, clip: &Clip) {
        let Some(b) = self.key_editor.boxing.clone() else {
            return;
        };
        if b.clip != clip.id {
            self.key_editor.boxing = None;
            return;
        }
        if b.tray != self.key_editor.tray {
            return;
        }
        let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
        let Some(pos) = pos else {
            return;
        };
        let rect = Rect::from_two_pos(b.origin, pos);
        let painter = ui.painter_at(area);
        let accent = theme::colors().accent;
        painter.rect_filled(rect, 0.0, accent.gamma_multiply(0.15));
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, accent),
            egui::StrokeKind::Inside,
        );
        if !released {
            ui.ctx().request_repaint();
            return;
        }
        let mut chosen = Vec::new();
        for (at, m) in &self.key_editor.seen {
            if rect.contains(*at) && !chosen.contains(m) {
                chosen.push(*m);
            }
        }
        self.key_editor.selected = chosen;
        self.key_editor.boxing = None;
    }

    pub(super) fn delete_editor_keys(&mut self) -> bool {
        if !self.keys_in_view() || self.key_editor.selected.is_empty() {
            return false;
        }
        let Some(id) = self.key_editor.clip.clone() else {
            return false;
        };
        let marks = std::mem::take(&mut self.key_editor.selected);
        self.remove_marks(&id, &marks);
        true
    }

    fn remove_marks(&self, id: &str, marks: &[Mark]) {
        let parts = unique_params(marks)
            .into_iter()
            .map(|param| {
                let frames = frames_of(marks, param);
                json!({ "clips": [id], "param": param, "frames": frames })
            })
            .collect();
        self.send_each("keyframes.remove", parts);
    }

    fn key_target(&self) -> Result<Target, &'static str> {
        let p = self.project_shared().ok_or("Open a project first")?;
        let clip = self
            .first_clip(&p)
            .ok_or("Select a clip or a subtitle first")?;
        let e = &self.key_editor;
        let mine = e.clip.as_deref() == Some(clip.id.as_str());
        let mut shown = Vec::new();
        if self.key_tray {
            shown.extend(lanes_of(&clip, true).into_iter().map(|l| l.param));
        }
        if self.left == LeftTab::Keyframes {
            shown.extend(lanes_of(&clip, e.keyed_only).into_iter().map(|l| l.param));
        }
        let rows = e
            .rows
            .iter()
            .copied()
            .filter(|r| mine && shown.contains(r))
            .collect();
        let marks = if mine { e.selected.clone() } else { Vec::new() };
        Ok(Target {
            lanes: lanes_of(&clip, false),
            clip,
            marks,
            rows,
            play: self.snap.timeline.position,
        })
    }

    fn speed_clips(&self) -> Result<Vec<Clip>, &'static str> {
        let p = self.project_shared().ok_or("Open a project first")?;
        let picked: Vec<Clip> = self
            .snap
            .selection
            .clips
            .iter()
            .filter_map(|id| p.clip(id))
            .cloned()
            .collect();
        if picked.is_empty() {
            return Err("Select a clip first");
        }
        let clips: Vec<Clip> = picked.into_iter().filter(takes_speed).collect();
        if clips.is_empty() {
            return Err("Titles and compound clips have no speed keyframes");
        }
        Ok(clips)
    }

    pub(super) fn key_menu_why(&self) -> Option<&'static str> {
        if self.keys_in_view() {
            self.key_target().err()
        } else {
            self.speed_clips().err()
        }
    }

    fn speed_cmd_why(&self, cmd: KeyCmd) -> Option<&'static str> {
        let clips = match self.speed_clips() {
            Ok(c) => c,
            Err(why) => return Some(why),
        };
        let play = self.snap.timeline.position;
        let under = |c: &&Clip| (c.start..c.end()).contains(&play);
        match cmd {
            KeyCmd::Add => add_speed_key_why(&clips.iter().collect::<Vec<_>>(), play),
            KeyCmd::Delete if !clips.iter().any(|c| under(&c)) => {
                Some("Move the playhead over the clip")
            }
            KeyCmd::Delete
                if !clips
                    .iter()
                    .any(|c| under(&c) && speed_key_upto(c, play).is_some()) =>
            {
                Some("No speed keyframe at or before the playhead")
            }
            KeyCmd::DeleteAll if clips.iter().all(|c| speed_key_count(c) == 0) => {
                Some("The clip has no speed keyframes")
            }
            _ => None,
        }
    }

    fn speed_cmd(&mut self, cmd: KeyCmd) {
        let Ok(clips) = self.speed_clips() else {
            return;
        };
        let play = self.snap.timeline.position;
        let speed = KeyParam::Speed;
        match cmd {
            KeyCmd::Add => {
                let ids: Vec<&str> = clips
                    .iter()
                    .filter(|c| adds_speed_key(c, play))
                    .map(|c| c.id.as_str())
                    .collect();
                self.send(
                    "keyframes.add",
                    json!({ "clips": ids, "param": speed, "at": play }),
                );
            }
            KeyCmd::Delete => {
                let parts = clips
                    .iter()
                    .filter(|c| (c.start..c.end()).contains(&play))
                    .filter_map(|c| {
                        let frame = speed_key_upto(c, play)?;
                        Some(json!({ "clips": [c.id], "param": speed, "frames": [frame] }))
                    })
                    .collect();
                self.send_each("keyframes.remove", parts);
            }
            KeyCmd::DeleteAll => {
                let held: Vec<&Clip> = clips.iter().filter(|c| speed_key_count(c) > 0).collect();
                let count: usize = held.iter().map(|c| speed_key_count(c)).sum();
                let whose = match held.as_slice() {
                    [one] => self
                        .project_shared()
                        .map(|p| clip_name(&p, one))
                        .unwrap_or_default(),
                    many => format!("{} clips", many.len()),
                };
                let keyframes = if count == 1 { "keyframe" } else { "keyframes" };
                self.key_editor.deleting = Some(Deleting {
                    clips: held.iter().map(|c| c.id.clone()).collect(),
                    params: vec![speed],
                    question: format!(
                        "Delete {count} {} {keyframes} of {whose}?",
                        lane_name(speed)
                    ),
                    fresh: true,
                });
            }
        }
    }

    pub(super) fn key_cmd_why(&self, cmd: KeyCmd) -> Option<&'static str> {
        if !self.keys_in_view() {
            return self.speed_cmd_why(cmd);
        }
        let t = match self.key_target() {
            Ok(t) => t,
            Err(why) => return Some(why),
        };
        if cmd != KeyCmd::Add && !t.marks.is_empty() {
            return None;
        }
        if t.rows.is_empty() {
            return Some(match cmd {
                KeyCmd::Add if self.left == LeftTab::Keyframes => {
                    "Click a parameter name, such as Opacity"
                }
                KeyCmd::Add if lanes_of(&t.clip, true).is_empty() => {
                    "Click a parameter name in Timeline > Keyframes"
                }
                KeyCmd::Add => "Click a parameter name in the keyframe tray",
                KeyCmd::Delete => "Click a keyframe first",
                KeyCmd::DeleteAll => "Click a keyframe or a parameter name first",
            });
        }
        match cmd {
            KeyCmd::Add if !t.rows.iter().any(|&r| t.takes(r)) => {
                Some("Move the playhead over the clip")
            }
            KeyCmd::Add if !t.rows.iter().any(|&r| t.takes(r) && !t.keyed_here(r)) => {
                Some("Already keyed here. Move the playhead to add one.")
            }
            KeyCmd::Delete if !t.rows.iter().any(|&r| t.keyed_here(r)) => {
                Some("Click a keyframe, or move the playhead onto one")
            }
            KeyCmd::DeleteAll if t.rows.iter().all(|&r| t.count(r) == 0) => {
                Some("Click a parameter name that has keyframes")
            }
            _ => None,
        }
    }

    pub(super) fn key_cmd(&mut self, cmd: KeyCmd) {
        if self.key_cmd_why(cmd).is_some() {
            return;
        }
        if !self.keys_in_view() {
            self.speed_cmd(cmd);
            return;
        }
        let Ok(t) = self.key_target() else {
            return;
        };
        let id = t.clip.id.clone();
        if cmd != KeyCmd::Add && !t.marks.is_empty() {
            self.key_editor.selected.clear();
            self.remove_marks(&id, &t.marks);
            return;
        }
        match cmd {
            KeyCmd::Add => {
                let parts = t
                    .rows
                    .iter()
                    .filter(|&&r| t.takes(r) && !t.keyed_here(r))
                    .map(|r| json!({ "clips": [id], "param": r, "at": t.play }))
                    .collect();
                self.send_each("keyframes.add", parts);
            }
            KeyCmd::Delete => {
                let parts = t
                    .rows
                    .iter()
                    .filter(|&&r| t.keyed_here(r))
                    .map(|r| json!({ "clips": [id], "param": r, "at": [t.play] }))
                    .collect();
                self.send_each("keyframes.remove", parts);
            }
            KeyCmd::DeleteAll => {
                let rows: Vec<KeyParam> =
                    t.rows.iter().copied().filter(|&r| t.count(r) > 0).collect();
                let count: usize = rows.iter().map(|&r| t.count(r)).sum();
                let names: Vec<String> = rows.iter().map(|&r| lane_name(r)).collect();
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                let name = self
                    .project_shared()
                    .map(|p| clip_name(&p, &t.clip))
                    .unwrap_or_default();
                let keyframes = if count == 1 { "keyframe" } else { "keyframes" };
                self.key_editor.deleting = Some(Deleting {
                    clips: vec![id],
                    params: rows,
                    question: format!("Delete {count} {} {keyframes} of {name}?", listed(&names)),
                    fresh: true,
                });
            }
        }
    }

    pub(super) fn delete_all_window(&mut self, ctx: &egui::Context) {
        let Some(ask) = self.key_editor.deleting.as_mut() else {
            return;
        };
        let fresh = std::mem::take(&mut ask.fresh);
        let question = ask.question.clone();
        let mut done = None;
        let modal = egui::Modal::new(egui::Id::unique("delete-all-keyframes")).show(ctx, |ui| {
            ui.set_max_width(super::dialog::WIDTH);
            ui.label(RichText::new(question).strong());
            super::dialog::footer(ui, |ui| {
                if super::dialog::main_button(ui, "Delete", true) {
                    done = Some(true);
                }
                let cancel = ui.button("Cancel");
                if fresh {
                    cancel.request_focus();
                }
                if cancel.clicked() {
                    done = Some(false);
                }
            });
        });
        if done.is_none() && super::workspace::escape_cancels(ctx, &modal) {
            done = Some(false);
        }
        if done.is_none() {
            return;
        }
        let Some(ask) = self.key_editor.deleting.take() else {
            return;
        };
        if done == Some(true) {
            let parts = ask
                .params
                .iter()
                .map(|r| json!({ "clips": ask.clips, "param": r }))
                .collect();
            self.send_each("keyframes.remove", parts);
        }
    }

    fn key_header(
        &mut self,
        ui: &mut egui::Ui,
        p: &Project,
        clip: &Clip,
        view: (f64, f64),
        play: i64,
    ) {
        let name = clip_name(p, clip);
        let (whole_lo, whole_hi) = clip_span(clip);
        let length = whole_hi - whole_lo;
        let most = (length / LEAST_FRAMES).max(1.0);
        let mut level = (length / (view.1 - view.0)).clamp(1.0, most);
        let mut zoom_to = None;
        let mut fit = false;
        let mut detail = false;
        let mut open = None;
        let editor = &mut self.key_editor;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button(DOTS_THREE, |ui| {
                    ui.radio_value(&mut editor.keyed_only, false, "Display All Parameters");
                    ui.radio_value(
                        &mut editor.keyed_only,
                        true,
                        "Display Parameters with Keyframes",
                    );
                    ui.separator();
                    if ui.button("Expand All Parameters").clicked() {
                        open = Some(true);
                        ui.close();
                    }
                    if ui.button("Collapse All Parameters").clicked() {
                        open = Some(false);
                        ui.close();
                    }
                    ui.separator();
                    ui.checkbox(&mut editor.snapping, "Enable Snapping");
                });
                if most > 1.0 {
                    ui.spacing_mut().slider_width = ZOOM_W;
                    let r = ui
                        .add(
                            egui::Slider::new(&mut level, 1.0..=most)
                                .logarithmic(true)
                                .show_value(false),
                        )
                        .on_hover_text("Zoom");
                    if r.changed() {
                        zoom_to = Some(level);
                    }
                }
                if ui
                    .button(ARROWS_OUT_LINE_HORIZONTAL)
                    .on_hover_text("Fit Clip")
                    .clicked()
                {
                    fit = true;
                }
                if ui
                    .button(MAGNIFYING_GLASS_PLUS)
                    .on_hover_text("Detail Zoom")
                    .clicked()
                {
                    detail = true;
                }
                if ui
                    .add(egui::Button::selectable(editor.graph, CHART_LINE))
                    .on_hover_text("Graph")
                    .clicked()
                {
                    editor.graph = !editor.graph;
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                });
            });
        });
        if fit {
            editor.view = None;
        }
        if detail {
            let half = ((view.1 - view.0) / DETAIL).max(LEAST_FRAMES) / 2.0;
            let at = play as f64;
            editor.view = Some((at - half, at + half));
        }
        if let Some(level) = zoom_to {
            let mid = (view.0 + view.1) / 2.0;
            let half = length / level / 2.0;
            editor.view = Some((mid - half, mid + half));
        }
        match open {
            Some(true) => editor.closed.clear(),
            Some(false) => editor.closed = groups_of(clip),
            None => {}
        }
    }

    fn ruler(&mut self, ui: &mut egui::Ui, p: &Project, clip: &Clip, plot: Plot) {
        let now = p.settings.timecode(plot.play);
        let (rect, resp) = row(ui, RULER_H, plot, |ui| {
            ui.label(RichText::new(now).small().color(theme::colors().text_dim));
        });
        let map = TimeMap::of(plot.lo, plot.hi, rect);
        let painter = ui.painter_at(rect);
        let colors = theme::colors();
        painter.rect_filled(rect, 2.0, colors.panel_dark);
        painter.rect_filled(clip_band(map, clip, rect), 0.0, colors.track);
        let step = tick_step(map.ppf(), p.settings.fps.as_f64()).max(1);
        let font = TextStyle::Small.resolve(ui.style());
        let mut t = (map.lo / step as f64).ceil() as i64 * step;
        while (t as f64) <= map.hi {
            let x = map.x(t as f64);
            painter.line_segment(
                [pos2(x, rect.bottom() - 5.0), pos2(x, rect.bottom())],
                Stroke::new(1.0, colors.text_dim),
            );
            painter.text(
                pos2(x + 2.0, rect.top() + 1.0),
                Align2::LEFT_TOP,
                p.settings.timecode(t),
                font.clone(),
                colors.text_dim,
            );
            t += step;
        }
        playhead(&painter, map, rect, plot);
        if let Some(pos) = resp.interact_pointer_pos() {
            let frame = map.time(pos.x).round() as i64;
            let done = resp.clicked() || resp.drag_stopped();
            if done || frame != plot.play {
                self.seek(PlayerId::Timeline, frame, !done);
            }
        }
    }

    fn key_scroll(&mut self, ui: &mut egui::Ui, clip: &Clip, lanes: &[Lane], plot: Plot) {
        let (rect, resp) = row(ui, SCROLL_H, plot, |_| {});
        let (mut lo, mut hi) = clip_span(clip);
        for k in lanes.iter().flat_map(|l| &l.keys) {
            let t = clip.key_time(k.frame);
            lo = lo.min(t);
            hi = hi.max(t);
        }
        let span = plot.hi - plot.lo;
        let total = self
            .key_editor
            .grab
            .map_or((lo.min(plot.lo), hi.max(plot.hi)), |(_, t)| t);
        let map = TimeMap::of(total.0, total.1, rect);
        let painter = ui.painter_at(rect);
        let colors = theme::colors();
        let round = rect.height() / 2.0;
        painter.rect_filled(rect, round, colors.panel_dark);
        let a = map.x(plot.lo);
        let b = map.x(plot.hi).max(a + SCROLL_H);
        let thumb = Rect::from_x_y_ranges(a..=b, rect.y_range()).shrink(1.0);
        painter.rect_filled(thumb, round, colors.text_dim);
        playhead(&painter, map, rect, plot);
        if !resp.is_pointer_button_down_on() {
            self.key_editor.grab = None;
            return;
        }
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let (grab, total) = *self.key_editor.grab.get_or_insert_with(|| {
            let from = ui
                .input(|i| i.pointer.press_origin())
                .map_or(pos.x, |o| o.x);
            let t = map.time(from);
            let grab = if (plot.lo..=plot.hi).contains(&t) {
                t - plot.lo
            } else {
                span / 2.0
            };
            (grab, total)
        });
        let start = (map.time(pos.x) - grab).clamp(total.0, (total.1 - span).max(total.0));
        self.key_editor.view = Some((start, start + span));
    }

    fn zoom_view(&mut self, ui: &egui::Ui, area: Rect, clip: &Clip, plot: Plot) {
        let left = area.left() + LABEL + ui.spacing().item_spacing.x;
        let (hover, zoom, pan) = ui.input(|i| {
            (
                i.pointer.hover_pos(),
                i.zoom_delta(),
                i.smooth_scroll_delta.x,
            )
        });
        let Some(pos) = hover.filter(|h| area.contains(*h) && h.x >= left) else {
            return;
        };
        if zoom == 1.0 && pan == 0.0 {
            return;
        }
        let span = plot.hi - plot.lo;
        let at = plot.lo + f64::from((pos.x - left) / plot.width) * span;
        let longest = ((clip.end() - clip.start) as f64 * 20.0).max(100.0);
        let next = (span / f64::from(zoom)).clamp(LEAST_FRAMES, longest);
        let k = next / span;
        let by = -f64::from(pan) / f64::from(plot.width) * next;
        self.key_editor.view = Some((at - (at - plot.lo) * k + by, at + (plot.hi - at) * k + by));
    }

    fn finish_drag(&mut self, ui: &egui::Ui, clip: &Clip, lanes: &[Lane], rev: u64) {
        let (released, pos) = ui.input(|i| (i.pointer.any_released(), i.pointer.latest_pos()));
        if released
            && let Some(drag) = self.key_editor.drag.clone()
            && drag.clip == clip.id
            && let Some(pos) = pos
        {
            let shift = shift_of(clip, lanes, &drag, pos);
            self.commit(clip, rev, &shift);
            self.key_editor.drag = None;
        } else if self.key_editor.drag.is_some() {
            ui.ctx().request_repaint();
        }
    }

    fn expire_pending(&mut self, clip: &Clip, rev: u64) {
        let Some(pending) = self.key_editor.pending.clone() else {
            return;
        };
        if pending.clip != clip.id {
            self.key_editor.pending = None;
            return;
        }
        if rev != pending.rev {
            retarget(
                &mut self.key_editor.selected,
                &pending.shift.marks,
                pending.shift.dt,
            );
            if let Some(menu) = self.key_editor.menu.as_mut() {
                let mut one = [*menu];
                retarget(&mut one, &pending.shift.marks, pending.shift.dt);
                *menu = one[0];
            }
            self.key_editor.pending = None;
        } else if pending.since.elapsed() >= Duration::from_secs(1) {
            self.key_editor.pending = None;
        }
    }

    fn shown_shift(&mut self, ui: &egui::Ui, clip: &Clip, lanes: &[Lane]) -> Option<Shift> {
        if let Some(drag) = self.key_editor.drag.as_mut()
            && drag.clip == clip.id
            && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
        {
            let (shift, alt, press) =
                ui.input(|i| (i.modifiers.shift, i.modifiers.alt, i.pointer.press_origin()));
            drag.lock = locked(drag.lock, shift, pos - press.unwrap_or(drag.origin));
            drag.apart = alt;
            return Some(shift_of(clip, lanes, drag, pos));
        }
        self.key_editor
            .pending
            .as_ref()
            .filter(|p| p.clip == clip.id)
            .map(|p| p.shift.clone())
    }

    fn commit(&mut self, clip: &Clip, rev: u64, shift: &Shift) {
        let mut parts = Vec::new();
        if shift.dv != 0.0 && shift.axis != Axis::None {
            for param in unique_params(&shift.marks) {
                let frames = frames_of(&shift.marks, param);
                parts.push((
                    "keyframes.set",
                    by_body(
                        &clip.id,
                        param,
                        &frames,
                        by_value(clip, param, shift.axis, shift.dv),
                    ),
                ));
            }
        }
        for edit in &shift.handles {
            parts.push((
                "keyframes.set",
                handle_body(&clip.id, edit.mark, edit.out, edit.handle),
            ));
        }
        if shift.dt != 0 {
            for param in unique_params(&shift.marks) {
                let frames = frames_of(&shift.marks, param);
                parts.push((
                    "keyframes.move",
                    move_body(&clip.id, param, &frames, shift.dt),
                ));
            }
        }
        if parts.is_empty() {
            return;
        }
        send_parts(self, parts);
        self.key_editor.pending = Some(Pending {
            clip: clip.id.clone(),
            rev,
            since: Instant::now(),
            shift: shift.clone(),
        });
    }

    fn group_row(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        members: &[&Lane],
        plot: Plot,
        shift: Option<&Shift>,
    ) {
        let group = group_of(members[0].param);
        let open = !self.key_editor.closed.contains(&group);
        let mut flip = false;
        let (rect, resp) = row(ui, GROUP_H, plot, |ui| {
            let icon = if open { CARET_DOWN } else { CARET_RIGHT };
            let text = RichText::new(format!("{icon} {}", group_name(group, clip))).small();
            if ui.add(egui::Button::new(text).frame(false)).clicked() {
                flip = true;
            }
        });
        if flip {
            if open {
                self.key_editor.closed.push(group);
            } else {
                self.key_editor.closed.retain(|g| *g != group);
            }
        }
        let items = lane_items(clip, members);
        let mut times: Vec<i64> = items.iter().map(|(_, _, t)| *t).collect();
        times.sort_unstable();
        times.dedup();
        let map = TimeMap::of(plot.lo, plot.hi, rect);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, theme::colors().panel_dark);
        playhead(&painter, map, rect, plot);
        let mut spots = Vec::new();
        let mut shown = Vec::new();
        for t in times {
            let marks = group_at(&items, t);
            let at_time = shown_group_time(clip, &marks, shift);
            let at = pos2(map.x(at_time), rect.center().y);
            shown.push(at_time);
            spots.push((at, marks));
            diamond(&painter, at, 5.0, theme::colors().text);
        }
        edge_arrows(&painter, rect, &shown, map);
        for (at, marks) in &spots {
            self.key_editor.seen.extend(marks.iter().map(|m| (*at, *m)));
        }
        self.poke_group(ui, &resp, clip, map, &spots);
    }

    fn drag_snap(&self) -> Option<i64> {
        self.key_editor
            .snapping
            .then_some(self.snap.timeline.position)
    }

    fn poke_group(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        clip: &Clip,
        map: TimeMap,
        spots: &[(Pos2, Vec<Mark>)],
    ) {
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let from = pressed_at(ui, resp, pos);
        let hit = closest(spots, from, |(at, _)| *at).map(|(_, marks)| marks.clone());
        if !resp.drag_started() {
            return;
        }
        let Some(marks) = hit else {
            self.start_box(ui, clip, pos);
            return;
        };
        if marks.iter().any(|m| fixed(m.param)) {
            return;
        }
        self.key_editor.drag = Some(Drag {
            snap: self.drag_snap(),
            ..Drag::new(clip, Kind::Group, pos, map, marks)
        });
    }

    fn track_row(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        lane: &Lane,
        rows: &[Shown],
        plot: Plot,
    ) {
        let play = plot.play;
        let speed = lane.param == KeyParam::Speed;
        let reached = |t: i64| !speed || (clip.start <= t && t < clip.end());
        let times: Vec<i64> = lane
            .keys
            .iter()
            .map(|k| clip.key_time(k.frame).round() as i64)
            .collect();
        let on_key = times.contains(&play);
        let keyable = !speed || (clip.start < play && play < clip.end());
        let prev = times.iter().any(|&t| reached(t) && t < play);
        let next = times.iter().any(|&t| reached(t) && t > play);
        let graph = self.key_editor.graph;
        let curve = if graph {
            !self.key_editor.graph_off.contains(&lane.param)
        } else {
            self.key_editor.curves.contains(&lane.param)
        };
        let keyed = !lane.keys.is_empty();
        let picked = self.key_editor.rows.contains(&lane.param);
        let mut act = None;
        let mut named = false;
        let (rect, resp) = row(ui, TRACK_H, plot, |ui| {
            let head = ui.max_rect();
            named = ui
                .interact(
                    head,
                    ui.scope_id().with(lane_name(lane.param)),
                    Sense::click(),
                )
                .clicked();
            if picked {
                ui.painter().rect_filled(head, 2.0, theme::colors().line);
            }
            if !plot.tray {
                let c = ui
                    .add_enabled(
                        has_curve(clip, lane),
                        egui::Button::selectable(curve && keyed, WAVE_SINE).frame(false),
                    )
                    .on_hover_text("Curve");
                if c.clicked() {
                    act = Some(RowAct::Curve);
                }
            }
            ui.add(
                egui::Label::new(RichText::new(lane_name(lane.param)).small()).selectable(false),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        keyed,
                        egui::Button::new(ARROW_COUNTER_CLOCKWISE).frame(false),
                    )
                    .on_hover_text("Reset Keyframes")
                    .clicked()
                {
                    act = Some(RowAct::Reset);
                }
                if ui
                    .add_enabled(next, egui::Button::new(CARET_RIGHT).frame(false))
                    .clicked()
                {
                    act = Some(RowAct::Step(1));
                }
                if ui
                    .add_enabled(keyable || on_key, key_diamond(on_key))
                    .clicked()
                {
                    act = Some(RowAct::Toggle);
                }
                if ui
                    .add_enabled(prev, egui::Button::new(CARET_LEFT).frame(false))
                    .clicked()
                {
                    act = Some(RowAct::Step(-1));
                }
            });
        });
        match act {
            Some(RowAct::Curve) => {
                let list = if graph {
                    &mut self.key_editor.graph_off
                } else {
                    &mut self.key_editor.curves
                };
                if list.contains(&lane.param) {
                    list.retain(|c| *c != lane.param);
                } else {
                    list.push(lane.param);
                }
            }
            Some(RowAct::Step(dir)) => {
                let to = if dir < 0 {
                    times
                        .iter()
                        .copied()
                        .filter(|&t| reached(t) && t < play)
                        .max()
                } else {
                    times
                        .iter()
                        .copied()
                        .filter(|&t| reached(t) && t > play)
                        .min()
                };
                if let Some(t) = to {
                    self.seek(PlayerId::Timeline, t, false);
                }
            }
            Some(RowAct::Toggle) => {
                let method = if on_key {
                    "keyframes.remove"
                } else {
                    "keyframes.add"
                };
                self.send(
                    method,
                    json!({ "clips": [clip.id], "param": lane.param, "at": play }),
                );
                if !on_key {
                    self.key_editor.fresh = Some((lane.param, play, Instant::now()));
                }
            }
            Some(RowAct::Reset) => {
                self.send(
                    "keyframes.remove",
                    json!({ "clips": [clip.id], "param": lane.param }),
                );
            }
            None => {}
        }
        if named {
            self.activate(lane.param);
        }
        let map = TimeMap::of(plot.lo, plot.hi, rect);
        let painter = ui.painter_at(rect);
        let colors = theme::colors();
        let (back, band) = if picked {
            (colors.line, colors.faint)
        } else {
            (colors.panel_dark, colors.track)
        };
        painter.rect_filled(rect, 2.0, back);
        painter.rect_filled(clip_band(map, clip, rect), 0.0, band);
        playhead(&painter, map, rect, plot);
        let mut spots = Vec::new();
        let mut shown = Vec::new();
        for row in rows {
            let t = clip.key_time(row.key.frame);
            let at = pos2(map.x(t), rect.center().y);
            let on = self
                .key_editor
                .selected
                .iter()
                .any(|m| m.param == lane.param && m.frame == row.from);
            let color = if on {
                theme::colors().accent
            } else {
                theme::colors().text
            };
            diamond(&painter, at, if on { 6.0 } else { 4.5 }, color);
            spots.push((at, row.from));
            shown.push(t);
        }
        edge_arrows(&painter, rect, &shown, map);
        self.key_editor.seen.extend(spots.iter().map(|(at, frame)| {
            (
                *at,
                Mark {
                    param: lane.param,
                    frame: *frame,
                },
            )
        }));
        self.open_menu(&resp, lane.param, &spots, false);
        self.poke_keys(
            Press {
                ui,
                resp: &resp,
                clip,
                lane,
                map,
            },
            &spots,
            None,
            Axis::None,
        );
        self.empty_press(ui, &resp, clip, lane.param, map, &spots);
        self.show_menu(&resp, clip, lane, false);
    }

    fn empty_press(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        clip: &Clip,
        param: KeyParam,
        map: TimeMap,
        spots: &[(Pos2, i64)],
    ) {
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        if closest(spots, pressed_at(ui, resp, pos), |(at, _)| *at).is_some() {
            return;
        }
        if resp.drag_started() {
            if self.key_editor.drag.is_none() {
                self.start_box(ui, clip, pos);
            }
        } else if resp.double_clicked() {
            let t = map.time(pos.x).round() as i64;
            if clip.start <= t && t < clip.end() {
                self.send(
                    "keyframes.add",
                    json!({ "clips": [clip.id], "param": param, "at": t }),
                );
                self.key_editor.fresh = Some((param, t, Instant::now()));
                self.seek(PlayerId::Timeline, t, false);
            }
        } else if resp.clicked()
            && !ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl)
        {
            self.key_editor.selected.clear();
        }
    }

    fn activate(&mut self, param: KeyParam) {
        self.key_editor.graph_active = Some(param);
        self.key_editor.rows = vec![param];
    }

    fn curve_row(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        lane: &Lane,
        rows: &[Shown],
        plot: Plot,
        frame: &Frame,
    ) {
        let (rect, resp) = row(ui, CURVE_H, plot, |_| {});
        let colors = theme::colors();
        ui.painter_at(rect)
            .rect_filled(rect, 2.0, colors.panel_dark);
        let area = CurveArea {
            rect,
            resp: &resp,
            clip,
            plot,
            frame,
        };
        let spots = self.paint_curve(ui, &area, lane, rows, [colors.text, colors.text_dim]);
        self.use_curve(ui, &area, lane, &spots);
    }

    fn curve_graph(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        lanes: &[Lane],
        plot: Plot,
        frame: &Frame,
        shift: Option<&Shift>,
    ) {
        let shown = graphed(clip, lanes, &self.key_editor.graph_off);
        let mut active = shown
            .iter()
            .find(|(lane, _)| Some(lane.param) == self.key_editor.graph_active)
            .or(shown.first())
            .map(|(lane, _)| lane.param);
        let gap = ui.spacing().item_spacing.y;
        let most = (ui.available_height()
            - BAR_H
            - SCROLL_H
            - GRIP_H
            - 3.0 * gap
            - TRACKS_KEPT * (TRACK_H + gap))
            .max(CURVE_H);
        let h = self
            .key_editor
            .graph_h
            .unwrap_or(GRAPH_H)
            .clamp(CURVE_H, most);
        let mut named = None;
        let (rect, resp) = row(ui, h, plot, |ui| {
            named = legend(ui, h, &shown, active);
        });
        if let Some(param) = named {
            self.activate(param);
            active = Some(param);
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, theme::colors().panel_dark);
        let area = CurveArea {
            rect,
            resp: &resp,
            clip,
            plot,
            frame,
        };
        let mut dims = Vec::new();
        for (lane, color) in shown.iter().filter(|(l, _)| Some(l.param) != active) {
            let rows = shown_lane(lane, shift);
            let lines = dim_curve(&area, &painter, lane, &rows, color.gamma_multiply(DIMMED));
            dims.extend(lines.into_iter().map(|line| (lane.param, line)));
        }
        if let Some((lane, color)) = shown.iter().find(|(l, _)| Some(l.param) == active) {
            let rows = shown_lane(lane, shift);
            let lines = [*color, color.gamma_multiply(DIMMED)];
            let spots = self.paint_curve(ui, &area, lane, &rows, lines);
            if resp
                .hover_pos()
                .filter(|at| !spots.hit(*at))
                .and_then(|at| dim_at(&dims, at))
                .is_some()
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let picked = resp
                .interact_pointer_pos()
                .filter(|at| resp.clicked() && !spots.hit(*at))
                .and_then(|at| dim_at(&dims, at));
            match picked {
                Some(param) => self.activate(param),
                None => self.use_curve(ui, &area, lane, &spots),
            }
        } else {
            playhead(&painter, TimeMap::of(plot.lo, plot.hi, rect), rect, plot);
        }
        self.graph_grip(ui, plot, h, most);
    }

    fn graph_grip(&mut self, ui: &mut egui::Ui, plot: Plot, h: f32, most: f32) {
        let width = plot.label + ui.spacing().item_spacing.x + plot.width;
        let (rect, resp) = ui.allocate_exact_size(vec2(width, GRIP_H), Sense::drag());
        let colors = theme::colors();
        let busy = resp.hovered() || resp.dragged();
        if busy {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            Stroke::new(1.0, if busy { colors.text_dim } else { colors.line }),
        );
        if resp.dragged() {
            self.key_editor.graph_h = Some((h + resp.drag_delta().y).clamp(CURVE_H, most));
        }
    }

    fn paint_curve(
        &self,
        ui: &egui::Ui,
        area: &CurveArea<'_>,
        lane: &Lane,
        rows: &[Shown],
        lines: [Color32; 2],
    ) -> CurveSpots {
        let CurveArea {
            rect,
            clip,
            plot,
            frame,
            ..
        } = *area;
        let map = TimeMap::of(plot.lo, plot.hi, rect);
        let values = self.curve_values(lane, rows, rect);
        let painter = ui.painter_at(rect);
        let colors = theme::colors();
        if !matches!(
            lane.param,
            KeyParam::Position
                | KeyParam::UpperPosition
                | KeyParam::LowerPosition
                | KeyParam::Anchor
        ) {
            let font = TextStyle::Small.resolve(ui.style());
            for (v, y, align) in [
                (values.hi, rect.top() + 1.0, Align2::LEFT_TOP),
                (values.lo, rect.bottom() - 1.0, Align2::LEFT_BOTTOM),
            ] {
                painter.text(
                    pos2(rect.left() + 3.0, y),
                    align,
                    value_text(lane.param, v, frame),
                    font.clone(),
                    colors.text_dim,
                );
            }
        }
        let keys: Vec<Keyframe> = rows.iter().map(|r| r.key).collect();
        let paint = CurvePaint {
            painter: &painter,
            clip,
            map,
            values,
        };
        let line = sample_curve(&paint, 1.5, lines[0], |source| {
            value_mapped(&keys, source, |i| keys[i].value)
        });
        if rows.first().is_some_and(|r| r.paired) {
            let ys: Vec<f64> = rows.iter().map(|r| r.y).collect();
            sample_curve(&paint, 1.0, lines[1], |source| {
                value_paired(&keys, source, |i| ys[i])
            });
        }
        playhead(&painter, map, rect, plot);
        let mut handles = Vec::new();
        let mut spots = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let x = map.x(clip.key_time(row.key.frame));
            let at = pos2(x, values.y(row.key.value));
            let on = self
                .key_editor
                .selected
                .iter()
                .any(|m| m.param == lane.param && m.frame == row.from);
            for (out, handle) in [(false, row.key.in_handle), (true, row.key.out_handle)] {
                let Some(handle) = handle else { continue };
                let Some(other) = (if out {
                    keys.get(i + 1)
                } else {
                    i.checked_sub(1).and_then(|j| keys.get(j))
                }) else {
                    continue;
                };
                let Some((source, curve)) = handle_point(
                    if out { &row.key } else { other },
                    if out { other } else { &row.key },
                    out,
                ) else {
                    continue;
                };
                let hat = pos2(map.x(clip.time_of(source)), values.y(curve));
                painter.line_segment([at, hat], Stroke::new(1.0, colors.text_dim));
                painter.rect_filled(
                    Rect::from_center_size(hat, vec2(7.0, 7.0)),
                    1.0,
                    colors.accent,
                );
                handles.push((hat, row.from, out, handle, other.frame));
            }
            dot(
                &painter,
                at,
                if on { 5.0 } else { 3.5 },
                if on { colors.accent } else { colors.text },
            );
            spots.push((at, row.from, Axis::X));
            if row.paired {
                let yat = pos2(x, values.y(row.y));
                dot(
                    &painter,
                    yat,
                    3.0,
                    if on { colors.accent } else { colors.text_dim },
                );
                spots.push((yat, row.from, Axis::Y));
            }
        }
        CurveSpots {
            map,
            values,
            keys: spots,
            handles,
            line,
        }
    }

    fn curve_values(&self, lane: &Lane, rows: &[Shown], rect: Rect) -> ValueMap {
        let frozen = self
            .key_editor
            .drag
            .as_ref()
            .and_then(|d| d.frozen)
            .filter(|(param, ..)| *param == lane.param);
        if let Some((_, lo, hi)) = frozen {
            return ValueMap {
                lo,
                hi,
                top: rect.top(),
                height: rect.height().max(1.0),
            };
        }
        let (vlo, vhi) = value_ends(rows);
        ValueMap::of(vlo, vhi, least_span(lane.param), rect)
    }

    fn use_curve(&mut self, ui: &egui::Ui, area: &CurveArea<'_>, lane: &Lane, at: &CurveSpots) {
        let CurveArea { resp, clip, .. } = *area;
        if resp
            .hover_pos()
            .is_some_and(|p| at.bend_at(clip, lane, p).is_some())
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        self.poke_curve(
            Press {
                ui,
                resp,
                clip,
                lane,
                map: at.map,
            },
            at,
        );
        let flat: Vec<(Pos2, i64)> = at.keys.iter().map(|(at, frame, _)| (*at, *frame)).collect();
        self.key_editor.seen.extend(flat.iter().map(|(at, frame)| {
            (
                *at,
                Mark {
                    param: lane.param,
                    frame: *frame,
                },
            )
        }));
        let all: Vec<(Pos2, i64)> = flat
            .iter()
            .copied()
            .chain(at.handles.iter().map(|(at, frame, ..)| (*at, *frame)))
            .collect();
        self.empty_press(ui, resp, clip, lane.param, at.map, &all);
        self.open_menu(resp, lane.param, &flat, true);
        self.show_menu(resp, clip, lane, true);
    }

    fn key_bar(&mut self, ui: &mut egui::Ui, p: &Project, clip: &Clip, lanes: &[Lane], f: &Frame) {
        ui.separator();
        let marks = self.key_editor.selected.clone();
        let mut gone = false;
        match marks.as_slice() {
            [] => {}
            [one] => gone = self.one_key(ui, p, clip, lanes, *one, f),
            many => {
                let (now, options) = marks_eases(clip, lanes, many);
                let mut ease = None;
                ui.horizontal(|ui| {
                    ui.label(format!("{} Keyframes", many.len()));
                    if !options.is_empty() {
                        ease = ease_combo(ui, "key-ease-many", now, "Mixed", options);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        gone = ui.button("Delete").clicked();
                    });
                });
                if let Some(e) = ease {
                    send_parts(self, ease_parts(clip, lanes, many, e));
                }
            }
        }
        if gone {
            self.delete_editor_keys();
        }
    }

    fn one_key(
        &mut self,
        ui: &mut egui::Ui,
        p: &Project,
        clip: &Clip,
        lanes: &[Lane],
        mark: Mark,
        f: &Frame,
    ) -> bool {
        let Some(lane) = lanes.iter().find(|l| l.param == mark.param) else {
            return false;
        };
        let Some(i) = lane.keys.iter().position(|k| k.frame == mark.frame) else {
            return false;
        };
        let key = lane.keys[i];
        let y = (lane.extra.len() == lane.keys.len()).then(|| lane.extra[i]);
        let paired = paired(mark.param) && y.is_some();
        let at = p.settings.timecode(clip.key_time(key.frame).round() as i64);
        let mut gone = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(lane_name(mark.param)).strong());
            ui.label(dim(at));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                gone = ui.button("Delete").clicked();
            });
        });
        let mut value = None;
        let mut ease = None;
        ui.horizontal(|ui| {
            let axes: &[bool] = if paired { &[false, true] } else { &[false] };
            for &on_y in axes {
                let raw = if on_y { y.unwrap_or(0.0) } else { key.value };
                let prefix = match (paired, on_y) {
                    (false, _) => "",
                    (true, false) => "X ",
                    (true, true) => "Y ",
                };
                if let Some(v) = self.value_field(ui, mark, on_y, raw, prefix, f) {
                    value = Some(if paired {
                        let (vx, vy) = if on_y {
                            (key.value, v)
                        } else {
                            (v, y.unwrap_or(0.0))
                        };
                        json!([vx, vy])
                    } else {
                        json!(v)
                    });
                }
            }
            if linear(clip, mark.param) {
                return;
            }
            let (current, options) = key_eases(clip, lane, i);
            let salt = ("key-ease", lane_name(mark.param), mark.frame);
            ease = ease_combo(ui, salt, current, "Custom", options);
        });
        if let Some(value) = value {
            if fixed(mark.param) {
                let at = clip.key_time(key.frame).round() as i64;
                self.send_asking(
                    "speed.set",
                    json!({ "clips": [clip.id], "at": at, "speed": value }),
                );
            } else {
                self.send(
                    "keyframes.set",
                    json!({ "clips": [clip.id], "param": mark.param, "frames": [mark.frame], "value": value }),
                );
            }
        }
        if let Some(e) = ease {
            self.send(
                "keyframes.set",
                ease_body(&clip.id, mark.param, &[mark.frame], e),
            );
        }
        gone
    }

    fn value_field(
        &mut self,
        ui: &mut egui::Ui,
        mark: Mark,
        on_y: bool,
        raw: f64,
        prefix: &str,
        f: &Frame,
    ) -> Option<f64> {
        let param = mark.param;
        let shown = shown_value(param, raw, on_y, f);
        let held = self
            .key_editor
            .held
            .filter(|h| h.mark == mark && h.y == on_y);
        let mut v = held.map_or(shown, |h| h.value);
        let speed = match param {
            KeyParam::Volume => 0.1,
            KeyParam::Zoom
            | KeyParam::UpperZoom
            | KeyParam::LowerZoom
            | KeyParam::Rotation
            | KeyParam::UpperRotation
            | KeyParam::LowerRotation => 0.5,
            KeyParam::Pitch | KeyParam::Yaw | KeyParam::CenterX | KeyParam::CenterY => 0.01,
            _ => 1.0,
        };
        let forward = !fixed(param) || raw > 0.0;
        let r = ui
            .push_id(("key-value", lane_name(param), on_y), |ui| {
                let mut drag = egui::DragValue::new(&mut v)
                    .update_while_editing(false)
                    .speed(speed)
                    .max_decimals(places(param).max(1))
                    .prefix(prefix)
                    .suffix(unit(param))
                    .min_size(vec2(FIELD_W, ui.spacing().interact_size.y));
                if fixed(param) && forward {
                    let (lo, hi) = limits(param);
                    drag = drag
                        .range(shown_value(param, lo, on_y, f)..=shown_value(param, hi, on_y, f));
                }
                ui.add_enabled(forward, drag)
                    .on_disabled_hover_text("Only a forward section takes a speed")
            })
            .inner;
        if r.drag_started() {
            self.key_editor.held = Some(Held {
                mark,
                y: on_y,
                value: shown,
                from: shown,
            });
        }
        if r.dragged()
            && r.changed()
            && let Some(h) = self.key_editor.held.as_mut()
        {
            h.value = v;
        }
        let out = if r.drag_stopped() {
            self.key_editor
                .held
                .take()
                .filter(|h| h.value != h.from)
                .map(|h| h.value)
        } else if r.changed() && !r.dragged() {
            self.key_editor.held = None;
            Some(v)
        } else {
            None
        };
        out.map(|v| stored_value(param, v, on_y, f))
    }

    fn poke_keys(
        &mut self,
        press: Press<'_>,
        spots: &[(Pos2, i64)],
        values: Option<ValueMap>,
        axis: Axis,
    ) {
        let Press {
            ui,
            resp,
            clip,
            lane,
            map,
        } = press;
        let toggle = ui.input(|i| i.modifiers.command);
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let hit = closest(spots, pressed_at(ui, resp, pos), |(at, _)| *at).map(|(_, frame)| Mark {
            param: lane.param,
            frame: *frame,
        });
        if (resp.drag_started() || resp.clicked()) && hit.is_some() {
            self.key_editor.graph_active = Some(lane.param);
            if !self.key_editor.rows.contains(&lane.param) {
                self.key_editor.rows.push(lane.param);
            }
        }
        if resp.drag_started()
            && !fixed(lane.param)
            && let Some(hit) = hit
        {
            self.key_editor.selected = if self.key_editor.selected.contains(&hit) {
                self.key_editor.selected.clone()
            } else {
                choose(&self.key_editor.selected, hit, toggle)
            };
            let marks = dragged(&self.key_editor.selected, hit);
            let (v_span, v_height) = values.map_or((0.0, 1.0), |v| (v.span(), v.height));
            self.key_editor.drag = Some(Drag {
                v_span,
                v_height,
                axis,
                snap: self.drag_snap(),
                ..Drag::new(clip, Kind::Keys, pos, map, marks)
            });
        } else if resp.clicked()
            && let Some(hit) = hit
        {
            self.key_editor.selected = choose(&self.key_editor.selected, hit, toggle);
        }
    }

    fn poke_curve(&mut self, press: Press<'_>, at: &CurveSpots) {
        let CurveSpots {
            values,
            keys: spots,
            handles,
            ..
        } = at;
        let values = *values;
        let Some(pos) = press.resp.interact_pointer_pos() else {
            return;
        };
        let Press {
            ui,
            resp,
            clip,
            lane,
            map,
        } = press;
        let from = pressed_at(ui, resp, pos);
        let frozen = Some((lane.param, values.lo, values.hi));
        let handle_hit = closest(handles, from, |(at, ..)| *at);
        let key_hit = closest(spots, from, |(at, ..)| *at);
        let use_handle = match (handle_hit, key_hit) {
            (Some(handle), Some(key)) => handle.0.distance(from) <= key.0.distance(from),
            (Some(_), None) => true,
            _ => false,
        };
        let toggle = ui.input(|i| i.modifiers.command);
        if use_handle && let Some((_, frame, out, handle, neighbor)) = handle_hit {
            let mark = Mark {
                param: lane.param,
                frame: *frame,
            };
            if resp.clicked() {
                self.key_editor.selected = choose(&self.key_editor.selected, mark, toggle);
            }
            if !resp.drag_started() {
                return;
            }
            let row_value = lane
                .keys
                .iter()
                .find(|k| k.frame == *frame)
                .map(|k| k.value)
                .unwrap_or(0.0);
            self.key_editor.drag = Some(Drag {
                v_span: values.span(),
                v_height: values.height,
                fix: Some(HandleFix {
                    mark,
                    out: *out,
                    frames: handle.frames,
                    value: handle.value,
                    key_value: row_value,
                    neighbor: *neighbor,
                    across: across_of(&lane.keys, *frame, *out),
                }),
                frozen,
                ..Drag::new(clip, Kind::Handle, pos, map, vec![mark])
            });
            return;
        }
        if resp.drag_started()
            && let Some((i, frame)) = at.bend_at(clip, lane, from)
        {
            self.key_editor.drag = Some(Drag {
                v_span: values.span(),
                v_height: values.height,
                bend: Some(bend_of(lane, i, frame)),
                frozen,
                ..Drag::new(clip, Kind::Bend, pos, map, Vec::new())
            });
            return;
        }
        let key_spots: Vec<(Pos2, i64)> =
            spots.iter().map(|(at, frame, _)| (*at, *frame)).collect();
        let axis = key_hit.map(|(_, _, axis)| *axis).unwrap_or(Axis::X);
        self.poke_keys(
            Press {
                ui,
                resp,
                clip,
                lane,
                map,
            },
            &key_spots,
            Some(values),
            axis,
        );
    }

    fn open_menu(
        &mut self,
        resp: &egui::Response,
        param: KeyParam,
        spots: &[(Pos2, i64)],
        curve: bool,
    ) {
        if !resp.secondary_clicked() {
            return;
        }
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        if let Some((_, frame)) = closest(spots, pos, |(at, _)| *at) {
            let mark = Mark {
                param,
                frame: *frame,
            };
            if !self.key_editor.selected.contains(&mark) {
                self.key_editor.selected = vec![mark];
            }
            self.key_editor.menu = Some(mark);
            self.key_editor.menu_curve = curve;
            self.key_editor.menu_tray = self.key_editor.tray;
        }
    }

    fn show_menu(&mut self, resp: &egui::Response, clip: &Clip, lane: &Lane, curve: bool) {
        let e = &self.key_editor;
        let open = e.menu.is_some_and(|m| {
            m.param == lane.param && e.menu_curve == curve && e.menu_tray == e.tray
        });
        if !open {
            return;
        }
        if linear(clip, lane.param) {
            self.key_editor.menu = None;
            return;
        }
        let lanes = lanes_of(clip, false);
        let marks: Vec<Mark> = match self.key_editor.menu {
            Some(m) if !self.key_editor.selected.contains(&m) => vec![m],
            _ => self.key_editor.selected.clone(),
        };
        let (current, options) = marks_eases(clip, &lanes, &marks);
        let shown = resp.context_menu(|ui| {
            for ease in options {
                if ui
                    .selectable_label(current == Some(ease), ease_label(ease))
                    .clicked()
                {
                    send_parts(self, ease_parts(clip, &lanes, &marks, ease));
                    self.key_editor.menu = None;
                    ui.close();
                }
            }
        });
        if shown.is_none() {
            self.key_editor.menu = None;
        }
    }
}

fn ease_combo(
    ui: &mut egui::Ui,
    salt: impl egui::AsIdSalt,
    now: Option<Ease>,
    other: &'static str,
    options: Vec<Ease>,
) -> Option<Ease> {
    let mut chosen = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(now.map_or(other, ease_label))
        .show_ui(ui, |ui| {
            for e in options {
                if ui.selectable_label(now == Some(e), ease_label(e)).clicked() {
                    chosen = Some(e);
                }
            }
        });
    chosen
}

fn row(
    ui: &mut egui::Ui,
    h: f32,
    plot: Plot,
    label: impl FnOnce(&mut egui::Ui),
) -> (Rect, egui::Response) {
    ui.horizontal(|ui| {
        let size = vec2(plot.label, h);
        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_size(size);
            label(ui);
        });
        ui.allocate_exact_size(vec2(plot.width, h), Sense::click_and_drag())
    })
    .inner
}

fn live_marks(lanes: &[Lane], shift: Option<&Shift>) -> Vec<Mark> {
    lanes
        .iter()
        .flat_map(|lane| {
            shown_lane(lane, shift).into_iter().map(|row| Mark {
                param: lane.param,
                frame: row.from,
            })
        })
        .collect()
}

fn shown_group_time(clip: &Clip, marks: &[Mark], shift: Option<&Shift>) -> f64 {
    let Some(mark) = marks.first() else {
        return clip.start as f64;
    };
    let frame = match shift {
        Some(shift) if shift.dt != 0 && shift.marks.contains(mark) => mark.frame + shift.dt,
        _ => mark.frame,
    };
    clip.key_time(frame)
}

#[derive(Clone, Copy)]
struct CurveArea<'a> {
    rect: Rect,
    resp: &'a egui::Response,
    clip: &'a Clip,
    plot: Plot,
    frame: &'a Frame,
}

struct CurveSpots {
    map: TimeMap,
    values: ValueMap,
    keys: Vec<(Pos2, i64, Axis)>,
    handles: Vec<(Pos2, i64, bool, Handle, i64)>,
    line: Vec<Pos2>,
}

impl CurveSpots {
    fn bend_at(&self, clip: &Clip, lane: &Lane, pos: Pos2) -> Option<(usize, f64)> {
        if self.hit(pos) || line_distance(&self.line, pos) > LINE_HIT {
            return None;
        }
        let frame = clip.key_at(self.map.time(pos.x));
        segment_at(&lane.keys, frame).map(|i| (i, frame))
    }

    fn hit(&self, pos: Pos2) -> bool {
        self.keys.iter().any(|(at, ..)| near(*at, pos))
            || self.handles.iter().any(|(at, ..)| near(*at, pos))
    }
}

struct Press<'a> {
    ui: &'a egui::Ui,
    resp: &'a egui::Response,
    clip: &'a Clip,
    lane: &'a Lane,
    map: TimeMap,
}

#[derive(Clone, Copy)]
struct CurvePaint<'a> {
    painter: &'a egui::Painter,
    clip: &'a Clip,
    map: TimeMap,
    values: ValueMap,
}

fn sample_curve(
    paint: &CurvePaint<'_>,
    width: f32,
    color: Color32,
    at: impl Fn(f64) -> Option<f64>,
) -> Vec<Pos2> {
    let CurvePaint {
        painter,
        clip,
        map,
        values,
    } = *paint;
    let n = ((map.width / 3.0) as i32).clamp(2, 400);
    let mut pts = Vec::with_capacity(n as usize + 1);
    for i in 0..=n {
        let x = map.left + map.width * i as f32 / n as f32;
        let t = map.lo + f64::from((x - map.left) / map.width) * (map.hi - map.lo);
        let source = clip.key_at(t);
        let Some(v) = at(source) else {
            continue;
        };
        pts.push(pos2(x, values.y(v)));
    }
    if pts.len() >= 2 {
        painter.add(Shape::line(pts.clone(), Stroke::new(width, color)));
    }
    pts
}

fn has_curve(clip: &Clip, lane: &Lane) -> bool {
    !lane.keys.is_empty() && !linear(clip, lane.param) && !fixed(lane.param)
}

fn graphed<'a>(clip: &Clip, lanes: &'a [Lane], off: &[KeyParam]) -> Vec<(&'a Lane, Color32)> {
    let all = lanes_of(clip, false);
    let hue = |param: KeyParam| {
        let i = all.iter().position(|l| l.param == param).unwrap_or(0);
        HUES[i % HUES.len()]
    };
    lanes
        .iter()
        .filter(|l| has_curve(clip, l) && !off.contains(&l.param))
        .map(|l| (l, hue(l.param)))
        .collect()
}

fn legend(
    ui: &mut egui::Ui,
    h: f32,
    shown: &[(&Lane, Color32)],
    active: Option<KeyParam>,
) -> Option<KeyParam> {
    if shown.is_empty() {
        ui.label(dim("No Curves"));
        return None;
    }
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("keyframe-legend")
        .auto_shrink([false, false])
        .max_height(h)
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                for (lane, color) in shown {
                    ui.horizontal(|ui| {
                        let (swatch, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().rect_filled(swatch, 2.0, *color);
                        let name = RichText::new(lane_name(lane.param)).small();
                        if ui
                            .selectable_label(active == Some(lane.param), name)
                            .clicked()
                        {
                            picked = Some(lane.param);
                        }
                    });
                }
            });
        });
    picked
}

fn dim_curve(
    area: &CurveArea<'_>,
    painter: &egui::Painter,
    lane: &Lane,
    rows: &[Shown],
    color: Color32,
) -> Vec<Vec<Pos2>> {
    let (vlo, vhi) = value_ends(rows);
    let paint = CurvePaint {
        painter,
        clip: area.clip,
        map: TimeMap::of(area.plot.lo, area.plot.hi, area.rect),
        values: ValueMap::of(vlo, vhi, least_span(lane.param), area.rect),
    };
    let keys: Vec<Keyframe> = rows.iter().map(|r| r.key).collect();
    let mut lines = vec![sample_curve(&paint, 1.0, color, |source| {
        value_mapped(&keys, source, |i| keys[i].value)
    })];
    if rows.first().is_some_and(|r| r.paired) {
        let ys: Vec<f64> = rows.iter().map(|r| r.y).collect();
        lines.push(sample_curve(&paint, 1.0, color, |source| {
            value_paired(&keys, source, |i| ys[i])
        }));
    }
    lines
}

fn lined_up(moved: Handle, other: Handle, gap: f64, px: [f64; 2]) -> Handle {
    let length = |h: Handle| (h.frames * px[0]).hypot(h.value * px[1]);
    let moved_len = length(moved);
    if moved_len == 0.0 {
        return other;
    }
    let k = (length(other) / moved_len).min(gap / moved.frames.abs());
    Handle {
        frames: (-moved.frames * k * 100.0).round() / 100.0,
        value: (-moved.value * k * 1e6).round() / 1e6,
    }
}

fn edits_with_pair(
    mark: Mark,
    out: bool,
    handle: Handle,
    across: Option<(Handle, f64)>,
    px: [f64; 2],
) -> Vec<HandleEdit> {
    let mut edits = vec![HandleEdit { mark, out, handle }];
    if let Some((other, gap)) = across {
        edits.push(HandleEdit {
            mark,
            out: !out,
            handle: lined_up(handle, other, gap, px),
        });
    }
    edits
}

fn across_of(keys: &[Keyframe], frame: i64, out: bool) -> Option<(Handle, f64)> {
    let i = keys.iter().position(|k| k.frame == frame)?;
    let key = &keys[i];
    let (other, neighbor) = if out {
        (key.in_handle, keys.get(i.checked_sub(1)?))
    } else {
        (key.out_handle, keys.get(i + 1))
    };
    Some((other?, (neighbor?.frame - frame).abs() as f64))
}

fn segment_at(keys: &[Keyframe], frame: f64) -> Option<usize> {
    keys.windows(2)
        .position(|w| (w[0].frame as f64) < frame && frame < w[1].frame as f64)
}

fn bend_of(lane: &Lane, i: usize, frame: f64) -> BendFix {
    let (a, b) = (lane.keys[i], lane.keys[i + 1]);
    BendFix {
        param: lane.param,
        a,
        b,
        frame,
        before: across_of(&lane.keys, a.frame, true),
        after: across_of(&lane.keys, b.frame, false),
    }
}

fn bend_shift(fix: BendFix, by: f64, apart: bool, px: [f64; 2]) -> Shift {
    let mut handles = Vec::new();
    if by != 0.0 {
        let (out, into) = bent(&fix.a, &fix.b, fix.frame, by);
        let mark = |frame: i64| Mark {
            param: fix.param,
            frame,
        };
        let pair = |across: Option<(Handle, f64)>| across.filter(|_| !apart);
        if let Some(h) = out {
            handles.extend(edits_with_pair(
                mark(fix.a.frame),
                true,
                h,
                pair(fix.before),
                px,
            ));
        }
        if let Some(h) = into {
            handles.extend(edits_with_pair(
                mark(fix.b.frame),
                false,
                h,
                pair(fix.after),
                px,
            ));
        }
    }
    Shift {
        marks: Vec::new(),
        dt: 0,
        dv: 0.0,
        axis: Axis::None,
        handles,
        snap: None,
    }
}

fn line_distance(line: &[Pos2], pos: Pos2) -> f32 {
    line.windows(2)
        .map(|w| {
            let along = w[1] - w[0];
            let t = (pos - w[0]).dot(along) / along.length_sq().max(f32::EPSILON);
            (w[0] + along * t.clamp(0.0, 1.0)).distance(pos)
        })
        .fold(f32::MAX, f32::min)
}

fn dim_at(dims: &[(KeyParam, Vec<Pos2>)], pos: Pos2) -> Option<KeyParam> {
    dims.iter()
        .map(|(param, line)| (*param, line_distance(line, pos)))
        .filter(|(_, d)| *d <= HIT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(param, _)| param)
}

fn shift_of(clip: &Clip, lanes: &[Lane], drag: &Drag, pos: Pos2) -> Shift {
    let dx = pos.x - drag.origin.x;
    let dy = pos.y - drag.origin.y;
    let px = [
        f64::from(drag.ppf),
        f64::from(drag.v_height) / drag.v_span.max(f64::MIN_POSITIVE),
    ];
    if let Some(fix) = drag.bend {
        let by = d_value(drag.v_span, drag.v_height, dy);
        return bend_shift(fix, by, drag.apart, px);
    }
    if drag.kind == Kind::Handle
        && let Some(fix) = drag.fix
    {
        let handle = moved_handle(clip, fix, dx, dy, drag.ppf, drag.v_span, drag.v_height);
        let same =
            (handle.frames - fix.frames).abs() < 1e-4 && (handle.value - fix.value).abs() < 1e-6;
        let mut handles = Vec::new();
        if !same {
            let across = fix.across.filter(|_| !drag.apart);
            handles = edits_with_pair(fix.mark, fix.out, handle, across, px);
        }
        return Shift {
            marks: vec![fix.mark],
            dt: 0,
            dv: 0.0,
            axis: Axis::None,
            handles,
            snap: None,
        };
    }
    let (dx, dy) = match drag.lock {
        Some(Lock::Time) => (dx, 0.0),
        Some(Lock::Value) => (0.0, dy),
        None => (dx, dy),
    };
    let moved = frames_moved(dx, drag.ppf);
    let near = drag
        .snap
        .filter(|_| drag.lock != Some(Lock::Value))
        .and_then(|play| snapped(clip, lanes, &drag.marks, moved, play, drag.ppf));
    let by = near.map_or(moved, |(by, _)| by);
    let dt = fitted(lanes, &drag.marks, by);
    let dv = if drag.axis == Axis::None {
        0.0
    } else if let Some(lane) = lanes
        .iter()
        .find(|l| drag.marks.iter().any(|m| m.param == l.param))
    {
        clamped_delta(
            clip,
            lane,
            &drag.marks,
            drag.axis,
            d_value(drag.v_span, drag.v_height, dy),
        )
    } else {
        0.0
    };
    Shift {
        marks: drag.marks.clone(),
        dt,
        dv,
        axis: drag.axis,
        handles: Vec::new(),
        snap: near.filter(|_| dt == by).map(|(_, at)| at),
    }
}

fn clip_name(p: &Project, clip: &Clip) -> String {
    clip.name
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| p.media(&clip.media).map(|m| m.name.clone()))
        .unwrap_or_default()
}

fn listed(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn send_parts(ws: &Workspace, parts: Vec<(&'static str, Value)>) {
    match parts.as_slice() {
        [] => {}
        [(method, params)] => ws.send_drag(method, params.clone()),
        _ => {
            let commands: Vec<Value> = parts
                .into_iter()
                .map(|(method, params)| json!({ "method": method, "params": params }))
                .collect();
            ws.send_drag("apply", json!({ "commands": commands }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(param: KeyParam, frame: i64) -> Mark {
        Mark { param, frame }
    }

    fn key(frame: i64) -> Keyframe {
        Keyframe {
            frame,
            value: 0.0,
            in_handle: None,
            out_handle: None,
        }
    }

    fn lane(param: KeyParam, frames: &[i64]) -> Lane {
        Lane {
            param,
            keys: frames.iter().copied().map(key).collect(),
            extra: Vec::new(),
        }
    }

    fn clip() -> Clip {
        serde_json::from_value(json!({
            "id": "c1", "media": "m1", "start": 0, "duration": 100,
            "source_in": 0, "video": true, "enabled": true
        }))
        .unwrap()
    }

    #[test]
    fn a_lined_up_handle_keeps_its_length_on_the_screen() {
        let moved = Handle {
            frames: 30.0,
            value: 0.4,
        };
        let other = Handle {
            frames: -5.0,
            value: 0.0,
        };
        let done = lined_up(moved, other, 300.0, [2.0, 200.0]);
        assert_eq!((done.frames, done.value), (-3.0, -0.04));
    }

    #[test]
    fn a_lined_up_handle_stops_at_its_neighbor() {
        let moved = Handle {
            frames: 10.0,
            value: 1.0,
        };
        let other = Handle {
            frames: -40.0,
            value: 0.0,
        };
        let done = lined_up(moved, other, 20.0, [1.0, 1.0]);
        assert_eq!((done.frames, done.value), (-20.0, -2.0));
    }

    #[test]
    fn a_handle_at_its_key_leaves_the_other_one() {
        let zero = Handle {
            frames: 0.0,
            value: 0.0,
        };
        let other = Handle {
            frames: -7.0,
            value: 2.0,
        };
        let done = lined_up(zero, other, 20.0, [1.0, 1.0]);
        assert_eq!((done.frames, done.value), (-7.0, 2.0));
    }

    #[test]
    fn a_bend_realigns_the_handle_across_the_key() {
        let mut lane = lane(KeyParam::Rotation, &[0, 30, 60]);
        lane.keys[1].in_handle = Some(Handle {
            frames: -10.0,
            value: 0.0,
        });
        lane.keys[1].out_handle = Some(Handle {
            frames: 10.0,
            value: 0.0,
        });
        lane.keys[2].in_handle = Some(Handle {
            frames: -10.0,
            value: 0.0,
        });
        let shift = bend_shift(bend_of(&lane, 1, 40.0), 6.0, false, [1.0, 1.0]);
        let across = shift
            .handles
            .iter()
            .find(|e| e.mark.frame == 30 && !e.out)
            .expect("the In handle moves too");
        assert!(across.handle.value < 0.0);
    }

    #[test]
    fn a_point_beside_a_steep_line_is_near_it() {
        let line = [pos2(0.0, 0.0), pos2(3.0, 90.0)];
        assert!(line_distance(&line, pos2(1.5, 45.0)) < 0.01);
    }

    #[test]
    fn a_click_on_two_dimmed_curves_takes_the_nearer() {
        let dims = [
            (KeyParam::Opacity, vec![pos2(0.0, 10.0), pos2(90.0, 10.0)]),
            (KeyParam::Rotation, vec![pos2(0.0, 14.0), pos2(90.0, 14.0)]),
        ];
        assert_eq!(dim_at(&dims, pos2(40.0, 13.0)), Some(KeyParam::Rotation));
    }

    #[test]
    fn a_shift_drag_keeps_the_direction_it_started_in() {
        let first = locked(None, true, vec2(1.0, 8.0));
        assert_eq!(locked(first, true, vec2(40.0, 2.0)), Some(Lock::Value));
    }

    #[test]
    fn a_drag_snaps_to_the_nearest_keyframe_of_another_value() {
        let lanes = [
            lane(KeyParam::Volume, &[10]),
            lane(KeyParam::Opacity, &[23, 40]),
        ];
        let marks = [mark(KeyParam::Volume, 10)];
        assert_eq!(
            snapped(&clip(), &lanes, &marks, 12, 80, 2.0),
            Some((13, 23))
        );
    }

    #[test]
    fn a_drag_does_not_snap_to_a_keyframe_of_its_own_value() {
        let lanes = [lane(KeyParam::Volume, &[10, 23])];
        let marks = [mark(KeyParam::Volume, 10)];
        assert_eq!(snapped(&clip(), &lanes, &marks, 12, 80, 2.0), None);
    }

    #[test]
    fn an_ease_in_leaves_a_first_keyframe_alone() {
        let lanes = [lane(KeyParam::Volume, &[0, 10, 20])];
        let marks = [mark(KeyParam::Volume, 0), mark(KeyParam::Volume, 10)];
        assert_eq!(
            ease_parts(&clip(), &lanes, &marks, Ease::EaseIn),
            vec![(
                "keyframes.set",
                json!({"clips":["c1"],"param":"volume","frames":[10],"ease":"ease_in"})
            )]
        );
    }

    #[test]
    fn a_click_selects_that_keyframe() {
        let a = mark(KeyParam::Volume, 1);
        let b = mark(KeyParam::Volume, 4);
        assert_eq!(choose(&[a], b, false), vec![b]);
    }

    #[test]
    fn a_drag_moves_the_selected_keyframes_of_that_value() {
        let a = mark(KeyParam::Zoom, 1);
        let b = mark(KeyParam::Zoom, 8);
        let c = mark(KeyParam::Volume, 1);
        assert_eq!(dragged(&[a, b, c], a), vec![a, b]);
    }

    #[test]
    fn twenty_pixels_at_ten_per_frame_moves_two_frames() {
        assert_eq!(frames_moved(20.0, 10.0), 2);
    }

    #[test]
    fn a_move_stops_one_frame_before_the_next_keyframe() {
        let lanes = [lane(KeyParam::Volume, &[0, 10])];
        assert_eq!(fitted(&lanes, &[mark(KeyParam::Volume, 0)], 10), 9);
    }

    #[test]
    fn a_group_stops_at_the_first_collision() {
        let lanes = [
            lane(KeyParam::Position, &[0, 10]),
            lane(KeyParam::Zoom, &[0, 4]),
        ];
        let marks = [mark(KeyParam::Position, 0), mark(KeyParam::Zoom, 0)];
        assert_eq!(fitted(&lanes, &marks, 10), 3);
    }

    #[test]
    fn a_group_drag_takes_the_keyframes_at_that_time() {
        let items = [
            (KeyParam::Position, 5, 10),
            (KeyParam::Zoom, 7, 10),
            (KeyParam::Rotation, 9, 12),
        ];
        assert_eq!(
            group_at(&items, 10),
            vec![mark(KeyParam::Position, 5), mark(KeyParam::Zoom, 7)]
        );
    }

    #[test]
    fn a_paired_y_ignores_the_handle_value() {
        let row = Shown {
            from: 0,
            key: Keyframe {
                frame: 0,
                value: 1.0,
                in_handle: None,
                out_handle: Some(Handle {
                    frames: 10.0,
                    value: 4.0,
                }),
            },
            y: 2.0,
            paired: true,
        };
        assert_eq!(value_ends(&[row]), (1.0, 5.0));
    }

    #[test]
    fn an_out_handle_drag_stores_the_offset() {
        assert_eq!(
            handle_of(0, 1.0, 30, true, 10.0, 1.5),
            Handle {
                frames: 10.0,
                value: 0.5
            }
        );
    }

    #[test]
    fn an_out_handle_stops_at_the_next_keyframe() {
        assert_eq!(handle_of(0, 1.0, 30, true, 40.0, 1.0).frames, 30.0);
    }

    #[test]
    fn a_position_drag_sets_x_and_keeps_y() {
        assert_eq!(
            by_body("c1", KeyParam::Position, &[4], json!([0.25, 0.0])),
            json!({"clips":["c1"],"param":"position","frames":[4],"by":[0.25,0.0]})
        );
    }

    #[test]
    fn an_ease_sets_that_keyframe() {
        assert_eq!(
            ease_body("c1", KeyParam::Volume, &[3], Ease::EaseIn),
            json!({"clips":["c1"],"param":"volume","frames":[3],"ease":"ease_in"})
        );
    }

    #[test]
    fn a_handle_request_names_the_out_handle() {
        assert_eq!(
            handle_body(
                "c1",
                mark(KeyParam::Rotation, 2),
                true,
                Handle {
                    frames: 4.0,
                    value: -1.0
                }
            ),
            json!({"clips":["c1"],"param":"rotation","frames":[2],"out":{"frames":4.0,"value":-1.0}})
        );
    }

    #[test]
    fn a_move_request_names_the_source_frames() {
        assert_eq!(
            move_body("c1", KeyParam::Volume, &[2, 5], 3),
            json!({"clips":["c1"],"param":"volume","frames":[2,5],"by":3})
        );
    }
}
