use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, Layout, Pos2, Rect, RichText, Sense, Shape, Stroke, TextStyle,
    pos2, vec2,
};
use egui_phosphor::regular::{
    ARROW_COUNTER_CLOCKWISE, ARROWS_OUT_LINE_HORIZONTAL, CARET_DOWN, CARET_LEFT, CARET_RIGHT,
    DIAMOND, DOTS_THREE, MAGNIFYING_GLASS_PLUS, WAVE_SINE,
};
use serde_json::{Value, json};
use vidcut_core::keyframes::{
    PictureValue, ease_of, eases, handle_point, value_mapped, value_paired,
};
use vidcut_core::model::{Clip, Ease, Handle, Keyframe, Picture, Project};
use vidcut_core::ops::picture::has_picture;
use vidcut_core::request::KeyParam;
use vidcut_core::validate::GAIN;

use super::inspector::{Shape as Frame, shape};
use super::placing::ease_label;
use super::theme;
use super::title_browser::LeftTab;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

const LABEL: f32 = 172.0;
const RULER_H: f32 = 20.0;
const GROUP_H: f32 = 22.0;
const TRACK_H: f32 = 26.0;
const CURVE_H: f32 = 110.0;
const BAR_H: f32 = 68.0;
const SCROLL_H: f32 = 12.0;
const ZOOM_W: f32 = 72.0;
const LEAST_FRAMES: f64 = 4.0;
const DETAIL: f64 = 4.0;
const HIT: f32 = 8.0;
const TICK_GAP: f32 = 96.0;
const FIELD_W: f32 = 72.0;
const GROUPS: [Group; 4] = [
    Group::Transform,
    Group::Crop,
    Group::Composite,
    Group::Audio,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Transform,
    Crop,
    Composite,
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
enum Kind {
    Keys,
    Group,
    Handle,
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
    handle: Option<HandleEdit>,
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
}

#[derive(Clone, Copy, Debug)]
struct HandleFix {
    mark: Mark,
    out: bool,
    frames: f64,
    value: f64,
    key_value: f64,
    neighbor: i64,
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
    width: f32,
    play: i64,
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

#[derive(Default)]
pub struct Editor {
    clip: Option<String>,
    selected: Vec<Mark>,
    drag: Option<Drag>,
    pending: Option<Pending>,
    menu: Option<Mark>,
    menu_curve: bool,
    keyed_only: bool,
    closed: Vec<Group>,
    curves: Vec<KeyParam>,
    view: Option<(f64, f64)>,
    held: Option<Held>,
    fresh: Option<(KeyParam, i64, Instant)>,
    grab: Option<(f64, (f64, f64))>,
}

impl Editor {
    pub(super) fn leave(&mut self) {
        self.drag = None;
    }
}

fn group_of(param: KeyParam) -> Group {
    match param {
        KeyParam::Position
        | KeyParam::Zoom
        | KeyParam::Rotation
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
    }
}

fn group_name(group: Group) -> &'static str {
    match group {
        Group::Transform => "Transform",
        Group::Crop => "Crop",
        Group::Composite => "Composite",
        Group::Audio => "Audio",
    }
}

fn lane_name(param: KeyParam) -> &'static str {
    match param {
        KeyParam::Volume => "Volume",
        KeyParam::Position => "Position",
        KeyParam::Zoom => "Zoom",
        KeyParam::Rotation => "Rotation",
        KeyParam::Anchor => "Anchor Point",
        KeyParam::Pitch => "Pitch",
        KeyParam::Yaw => "Yaw",
        KeyParam::CropLeft => "Crop Left",
        KeyParam::CropRight => "Crop Right",
        KeyParam::CropTop => "Crop Top",
        KeyParam::CropBottom => "Crop Bottom",
        KeyParam::CropSoftness => "Crop Softness",
        KeyParam::Opacity => "Opacity",
    }
}

fn limits(param: KeyParam) -> (f64, f64) {
    match param {
        KeyParam::Volume => (*GAIN.start(), *GAIN.end()),
        KeyParam::Position => (-100.0, 100.0),
        KeyParam::Zoom => (0.01, 100.0),
        KeyParam::Rotation => (-3600.0, 3600.0),
        KeyParam::Anchor => (-100.0, 100.0),
        KeyParam::Pitch | KeyParam::Yaw => (-1.5, 1.5),
        KeyParam::CropLeft | KeyParam::CropRight | KeyParam::CropTop | KeyParam::CropBottom => {
            (0.0, 0.9999)
        }
        KeyParam::CropSoftness => (-100.0, 100.0),
        KeyParam::Opacity => (0.0, 1.0),
    }
}

fn quantize(param: KeyParam, d: f64) -> f64 {
    let places = match param {
        KeyParam::Volume => 1,
        KeyParam::Rotation | KeyParam::CropSoftness => 2,
        _ => 6,
    };
    let k = 10f64.powi(places);
    (d * k).round() / k
}

fn least_span(param: KeyParam) -> f64 {
    match param {
        KeyParam::Volume => 6.0,
        KeyParam::Zoom => 0.1,
        KeyParam::Rotation | KeyParam::CropSoftness => 10.0,
        KeyParam::Pitch | KeyParam::Yaw => 0.1,
        _ => 0.05,
    }
}

fn shown_value(param: KeyParam, v: f64, y: bool, f: &Frame) -> f64 {
    let ([w, h], [fw, fh]) = (f.frame, f.fitted);
    match param {
        KeyParam::Position if y => (0.5 - v) * h,
        KeyParam::Position => (v - 0.5) * w,
        KeyParam::Anchor if y => (0.5 - v) * fh,
        KeyParam::Anchor => (v - 0.5) * fw,
        KeyParam::Zoom | KeyParam::Opacity => v * 100.0,
        KeyParam::CropLeft | KeyParam::CropRight => v * fw,
        KeyParam::CropTop | KeyParam::CropBottom => v * fh,
        KeyParam::Volume
        | KeyParam::Rotation
        | KeyParam::Pitch
        | KeyParam::Yaw
        | KeyParam::CropSoftness => v,
    }
}

fn stored_value(param: KeyParam, v: f64, y: bool, f: &Frame) -> f64 {
    let ([w, h], [fw, fh]) = (f.frame, f.fitted);
    match param {
        KeyParam::Position if y => 0.5 - v / h,
        KeyParam::Position => v / w + 0.5,
        KeyParam::Anchor if y => 0.5 - v / fh,
        KeyParam::Anchor => v / fw + 0.5,
        KeyParam::Zoom | KeyParam::Opacity => v / 100.0,
        KeyParam::CropLeft | KeyParam::CropRight => v / fw,
        KeyParam::CropTop | KeyParam::CropBottom => v / fh,
        KeyParam::Volume
        | KeyParam::Rotation
        | KeyParam::Pitch
        | KeyParam::Yaw
        | KeyParam::CropSoftness => v,
    }
}

fn unit(param: KeyParam) -> &'static str {
    match param {
        KeyParam::Volume => " dB",
        KeyParam::Zoom | KeyParam::Opacity => "%",
        KeyParam::Rotation => "°",
        _ => "",
    }
}

fn places(param: KeyParam) -> usize {
    match param {
        KeyParam::Volume | KeyParam::Zoom | KeyParam::Opacity => 1,
        KeyParam::Rotation | KeyParam::CropSoftness | KeyParam::Pitch | KeyParam::Yaw => 2,
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
        KeyParam::Position | KeyParam::Zoom | KeyParam::Anchor
    )
}

fn place_value(param: KeyParam, v: f64) -> f64 {
    let (lo, hi) = limits(param);
    v.clamp(lo, hi)
}

fn choose(selected: &[Mark], hit: Mark, add: bool) -> Vec<Mark> {
    if add {
        let mut out = selected.to_vec();
        if !out.contains(&hit) {
            out.push(hit);
        }
        out
    } else {
        vec![hit]
    }
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

fn fits(clip: &Clip, keys: &[Keyframe], moving: &[i64], dt: i64) -> bool {
    if clip.speed() == 0.0 {
        return dt == 0;
    }
    let shifted = |f: i64| clip.sound_at(clip.key_time(f) + dt as f64).round() as i64;
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

fn marks_fit(clip: &Clip, lanes: &[Lane], marks: &[Mark], dt: i64) -> bool {
    unique_params(marks).into_iter().all(|param| {
        let Some(lane) = lanes.iter().find(|l| l.param == param) else {
            return false;
        };
        let moving: Vec<i64> = marks
            .iter()
            .filter(|m| m.param == param)
            .map(|m| m.frame)
            .collect();
        fits(clip, &lane.keys, &moving, dt)
    })
}

fn fitted(clip: &Clip, lanes: &[Lane], marks: &[Mark], by: i64) -> i64 {
    if by == 0 || marks_fit(clip, lanes, marks, by) {
        return by;
    }
    let sign = by.signum();
    let (mut lo, mut hi) = (0, by.abs());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if marks_fit(clip, lanes, marks, mid * sign) {
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

fn by_value(param: KeyParam, axis: Axis, d: f64) -> Value {
    match param {
        KeyParam::Position | KeyParam::Zoom | KeyParam::Anchor => match axis {
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

fn ease_body(id: &str, mark: Mark, ease: Ease) -> Value {
    json!({ "clips": [id], "param": mark.param, "frames": [mark.frame], "ease": ease })
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

fn side_hi(clip: &Clip, param: KeyParam, frame: i64) -> f64 {
    let (_, hi) = limits(param);
    let Some(picture) = clip.picture.as_ref() else {
        return hi;
    };
    let crop = picture.crop_at(frame as f64);
    let other = match param {
        KeyParam::CropLeft => crop.right,
        KeyParam::CropRight => crop.left,
        KeyParam::CropTop => crop.bottom,
        KeyParam::CropBottom => crop.top,
        _ => return hi,
    };
    (1.0 - other - 1e-4).clamp(0.0, hi)
}

fn clamped_delta(clip: &Clip, lane: &Lane, marks: &[Mark], axis: Axis, raw: f64) -> f64 {
    if axis == Axis::None {
        return 0.0;
    }
    let (lo, _) = limits(lane.param);
    let mut d = raw;
    for m in marks.iter().filter(|m| m.param == lane.param) {
        let Some(i) = lane.keys.iter().position(|k| k.frame == m.frame) else {
            continue;
        };
        let start = match axis {
            Axis::Y => lane.extra.get(i).copied().unwrap_or(0.0),
            _ => lane.keys[i].value,
        };
        let hi = side_hi(clip, lane.param, m.frame);
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
        let hi = side_hi(clip, lane.param, m.frame);
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
    let source = clip.sound_at(new_t);
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

fn retarget(marks: &mut [Mark], clip: &Clip, moved: &[Mark], dt: i64) {
    if dt == 0 {
        return;
    }
    for m in marks {
        if moved.contains(m) {
            m.frame = clip.sound_at(clip.key_time(m.frame) + dt as f64).round() as i64;
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
    if has_picture(clip) {
        let picture = clip.picture.clone().unwrap_or_default();
        for v in PictureValue::ALL {
            if !keyed_only || picture.has_keys(v) {
                out.push(lane_from(&picture, v));
            }
        }
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

fn shown_lane(clip: &Clip, lane: &Lane, shift: Option<&Shift>) -> Vec<Shown> {
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
    if let Some(edit) = &shift.handle
        && edit.mark.param == lane.param
        && let Some(row) = rows.iter_mut().find(|r| r.from == edit.mark.frame)
    {
        if edit.out {
            row.key.out_handle = Some(edit.handle);
        } else {
            row.key.in_handle = Some(edit.handle);
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
            Axis::X => row.key.value = place_value(lane.param, row.key.value + shift.dv),
            Axis::Y => row.y = place_value(lane.param, row.y + shift.dv),
            Axis::None => {}
        }
        if shift.dt != 0 {
            let f = row.key.frame;
            row.key.frame = clip.sound_at(clip.key_time(f) + shift.dt as f64).round() as i64;
        }
    }
    rows.sort_by_key(|r| r.key.frame);
    rows
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

fn playhead(painter: &egui::Painter, map: TimeMap, rect: Rect, at: i64) {
    let t = at as f64;
    if (map.lo..=map.hi).contains(&t) {
        let x = map.x(t);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, theme::colors().playhead),
        );
    }
}

impl Workspace {
    pub(super) fn keyframe_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project_shared() else {
            ui.label(dim("Select a clip."));
            return;
        };
        let Some(id) = self
            .snap
            .selection
            .clips
            .iter()
            .find(|id| p.clip(id).is_some())
            .cloned()
        else {
            ui.label(dim("Select a clip."));
            return;
        };
        let Some(clip) = p.clip(&id).cloned() else {
            ui.label(dim("Select a clip."));
            return;
        };
        if self.key_editor.clip.as_deref() != Some(clip.id.as_str()) {
            self.key_editor.clip = Some(clip.id.clone());
            self.key_editor.selected.clear();
            self.key_editor.drag = None;
            self.key_editor.pending = None;
            self.key_editor.menu = None;
            self.key_editor.menu_curve = false;
            self.key_editor.view = None;
            self.key_editor.held = None;
            self.key_editor.fresh = None;
            self.key_editor.grab = None;
        }
        let lanes = lanes_of(&clip, self.key_editor.keyed_only);
        self.select_fresh(&clip, &lanes);
        let rev = p.rev;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.key_editor.drag = None;
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
            width,
            play,
        };
        self.ruler(ui, &p, &clip, plot);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(
                (ui.available_height() - BAR_H - SCROLL_H - ui.spacing().item_spacing.y)
                    .max(TRACK_H),
            )
            .show(ui, |ui| {
                for group in GROUPS {
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
                        let rows = shown_lane(&clip, lane, shift.as_ref());
                        self.track_row(ui, &clip, lane, &rows, plot);
                        if !lane.keys.is_empty() && self.key_editor.curves.contains(&lane.param) {
                            self.curve_row(ui, &clip, lane, &rows, plot, &frame);
                        }
                    }
                }
            });
        let live = live_marks(&clip, &lanes, shift.as_ref());
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

    pub(super) fn delete_editor_keys(&mut self) -> bool {
        if self.left != LeftTab::Keyframes || self.key_editor.selected.is_empty() {
            return false;
        }
        let Some(id) = self.key_editor.clip.clone() else {
            return false;
        };
        let marks = std::mem::take(&mut self.key_editor.selected);
        let parts = unique_params(&marks)
            .into_iter()
            .map(|param| {
                let frames = frames_of(&marks, param);
                json!({ "clips": [id], "param": param, "frames": frames })
            })
            .collect::<Vec<_>>();
        match parts.as_slice() {
            [] => return false,
            [one] => self.send("keyframes.remove", one.clone()),
            _ => {
                let commands: Vec<Value> = parts
                    .into_iter()
                    .map(|params| json!({ "method": "keyframes.remove", "params": params }))
                    .collect();
                self.send("apply", json!({ "commands": commands }));
            }
        }
        true
    }

    fn key_header(
        &mut self,
        ui: &mut egui::Ui,
        p: &Project,
        clip: &Clip,
        view: (f64, f64),
        play: i64,
    ) {
        let name = clip
            .name
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| p.media(&clip.media).map(|m| m.name.clone()))
            .unwrap_or_default();
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
            Some(false) => editor.closed = GROUPS.to_vec(),
            None => {}
        }
    }

    fn ruler(&mut self, ui: &mut egui::Ui, p: &Project, clip: &Clip, plot: Plot) {
        let now = p.settings.timecode(plot.play);
        let (rect, resp) = row(ui, RULER_H, plot.width, |ui| {
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
        playhead(&painter, map, rect, plot.play);
        if let Some(pos) = resp.interact_pointer_pos() {
            let frame = map.time(pos.x).round() as i64;
            let done = resp.clicked() || resp.drag_stopped();
            if done || frame != plot.play {
                self.seek(PlayerId::Timeline, frame, !done);
            }
        }
    }

    fn key_scroll(&mut self, ui: &mut egui::Ui, clip: &Clip, lanes: &[Lane], plot: Plot) {
        let (rect, resp) = row(ui, SCROLL_H, plot.width, |_| {});
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
        playhead(&painter, map, rect, plot.play);
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
                clip,
                &pending.shift.marks,
                pending.shift.dt,
            );
            if let Some(menu) = self.key_editor.menu.as_mut() {
                let mut one = [*menu];
                retarget(&mut one, clip, &pending.shift.marks, pending.shift.dt);
                *menu = one[0];
            }
            self.key_editor.pending = None;
        } else if pending.since.elapsed() >= Duration::from_secs(1) {
            self.key_editor.pending = None;
        }
    }

    fn shown_shift(&self, ui: &egui::Ui, clip: &Clip, lanes: &[Lane]) -> Option<Shift> {
        if let Some(drag) = &self.key_editor.drag
            && drag.clip == clip.id
            && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
        {
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
                        by_value(param, shift.axis, shift.dv),
                    ),
                ));
            }
        }
        if let Some(edit) = &shift.handle {
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
        let (rect, resp) = row(ui, GROUP_H, plot.width, |ui| {
            let icon = if open { CARET_DOWN } else { CARET_RIGHT };
            let text = RichText::new(format!("{icon} {}", group_name(group))).small();
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
        playhead(&painter, map, rect, plot.play);
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
        self.poke_group(&resp, clip, map, &spots);
    }

    fn poke_group(
        &mut self,
        resp: &egui::Response,
        clip: &Clip,
        map: TimeMap,
        spots: &[(Pos2, Vec<Mark>)],
    ) {
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let hit = closest(spots, pos, |(at, _)| *at).map(|(_, marks)| marks.clone());
        if resp.drag_started()
            && let Some(marks) = hit
        {
            self.key_editor.drag = Some(Drag {
                clip: clip.id.clone(),
                kind: Kind::Group,
                origin: pos,
                ppf: map.ppf(),
                v_span: 0.0,
                v_height: 1.0,
                marks,
                axis: Axis::None,
                fix: None,
            });
        }
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
        let inside = |t: i64| clip.start <= t && t < clip.end();
        let times: Vec<i64> = lane
            .keys
            .iter()
            .map(|k| clip.key_time(k.frame).round() as i64)
            .collect();
        let on_key = times.contains(&play);
        let prev = times.iter().any(|&t| inside(t) && t < play);
        let next = times.iter().any(|&t| inside(t) && t > play);
        let curve = self.key_editor.curves.contains(&lane.param);
        let keyed = !lane.keys.is_empty();
        let mut act = None;
        let (rect, resp) = row(ui, TRACK_H, plot.width, |ui| {
            let c = ui
                .add_enabled(
                    keyed,
                    egui::Button::selectable(curve && keyed, WAVE_SINE).frame(false),
                )
                .on_hover_text("Curve");
            if c.clicked() {
                act = Some(RowAct::Curve);
            }
            ui.label(RichText::new(lane_name(lane.param)).small());
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
                    .add_enabled(
                        inside(play),
                        egui::Button::selectable(on_key, DIAMOND).frame(false),
                    )
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
                if curve {
                    self.key_editor.curves.retain(|c| *c != lane.param);
                } else {
                    self.key_editor.curves.push(lane.param);
                }
            }
            Some(RowAct::Step(dir)) => {
                let to = if dir < 0 {
                    times
                        .iter()
                        .copied()
                        .filter(|&t| inside(t) && t < play)
                        .max()
                } else {
                    times
                        .iter()
                        .copied()
                        .filter(|&t| inside(t) && t > play)
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
        let map = TimeMap::of(plot.lo, plot.hi, rect);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, theme::colors().panel_dark);
        painter.rect_filled(clip_band(map, clip, rect), 0.0, theme::colors().track);
        playhead(&painter, map, rect, plot.play);
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
        self.show_menu(&resp, &clip.id, lane, false);
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
        if closest(spots, pos, |(at, _)| *at).is_some() {
            return;
        }
        if resp.double_clicked() {
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

    fn curve_row(
        &mut self,
        ui: &mut egui::Ui,
        clip: &Clip,
        lane: &Lane,
        rows: &[Shown],
        plot: Plot,
        frame: &Frame,
    ) {
        let (rect, resp) = row(ui, CURVE_H, plot.width, |_| {});
        let map = TimeMap::of(plot.lo, plot.hi, rect);
        let (vlo, vhi) = value_ends(rows);
        let values = ValueMap::of(vlo, vhi, least_span(lane.param), rect);
        let painter = ui.painter_at(rect);
        let colors = theme::colors();
        painter.rect_filled(rect, 2.0, colors.panel_dark);
        if !matches!(lane.param, KeyParam::Position | KeyParam::Anchor) {
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
        sample_curve(&paint, 1.5, colors.text, |source| {
            value_mapped(&keys, source, |i| keys[i].value)
        });
        if rows.first().is_some_and(|r| r.paired) {
            let ys: Vec<f64> = rows.iter().map(|r| r.y).collect();
            sample_curve(&paint, 1.0, colors.text_dim, |source| {
                value_paired(&keys, source, |i| ys[i])
            });
        }
        playhead(&painter, map, rect, plot.play);
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
        self.poke_curve(
            Press {
                ui,
                resp: &resp,
                clip,
                lane,
                map,
            },
            values,
            &spots,
            &handles,
        );
        let flat: Vec<(Pos2, i64)> = spots.iter().map(|(at, frame, _)| (*at, *frame)).collect();
        let all: Vec<(Pos2, i64)> = flat
            .iter()
            .copied()
            .chain(handles.iter().map(|(at, frame, ..)| (*at, *frame)))
            .collect();
        self.empty_press(ui, &resp, clip, lane.param, map, &all);
        self.open_menu(&resp, lane.param, &flat, true);
        self.show_menu(&resp, &clip.id, lane, true);
    }

    fn key_bar(&mut self, ui: &mut egui::Ui, p: &Project, clip: &Clip, lanes: &[Lane], f: &Frame) {
        ui.separator();
        let marks = self.key_editor.selected.clone();
        let mut gone = false;
        match marks.as_slice() {
            [] => {}
            [one] => gone = self.one_key(ui, p, clip, lanes, *one, f),
            many => {
                ui.horizontal(|ui| {
                    ui.label(format!("{} Keyframes", many.len()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        gone = ui.button("Delete").clicked();
                    });
                });
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
            let current = ease_of(&lane.keys, i);
            egui::ComboBox::from_id_salt(("key-ease", lane_name(mark.param), mark.frame))
                .selected_text(current.map_or("Custom", ease_label))
                .show_ui(ui, |ui| {
                    for e in eases(&lane.keys, i) {
                        if ui
                            .selectable_label(current == Some(e), ease_label(e))
                            .clicked()
                        {
                            ease = Some(e);
                        }
                    }
                });
        });
        if let Some(value) = value {
            self.send(
                "keyframes.set",
                json!({ "clips": [clip.id], "param": mark.param, "frames": [mark.frame], "value": value }),
            );
        }
        if let Some(e) = ease {
            self.send("keyframes.set", ease_body(&clip.id, mark, e));
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
            KeyParam::Zoom | KeyParam::Rotation => 0.5,
            KeyParam::Pitch | KeyParam::Yaw => 0.01,
            _ => 1.0,
        };
        let r = ui
            .push_id(("key-value", lane_name(param), on_y), |ui| {
                ui.add(
                    egui::DragValue::new(&mut v)
                        .update_while_editing(false)
                        .speed(speed)
                        .max_decimals(places(param).max(1))
                        .prefix(prefix)
                        .suffix(unit(param))
                        .min_size(vec2(FIELD_W, ui.spacing().interact_size.y)),
                )
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
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
        let Some(pos) = resp.interact_pointer_pos() else {
            return;
        };
        let hit = closest(spots, pos, |(at, _)| *at).map(|(_, frame)| Mark {
            param: lane.param,
            frame: *frame,
        });
        if resp.drag_started()
            && let Some(hit) = hit
        {
            self.key_editor.selected = if add {
                choose(&self.key_editor.selected, hit, true)
            } else if self.key_editor.selected.contains(&hit) {
                self.key_editor.selected.clone()
            } else {
                vec![hit]
            };
            let marks = dragged(&self.key_editor.selected, hit);
            let (v_span, v_height) = values.map_or((0.0, 1.0), |v| (v.span(), v.height));
            self.key_editor.drag = Some(Drag {
                clip: clip.id.clone(),
                kind: Kind::Keys,
                origin: pos,
                ppf: map.ppf(),
                v_span,
                v_height,
                marks,
                axis,
                fix: None,
            });
        } else if resp.clicked()
            && let Some(hit) = hit
        {
            self.key_editor.selected = choose(&self.key_editor.selected, hit, add);
        }
    }

    fn poke_curve(
        &mut self,
        press: Press<'_>,
        values: ValueMap,
        spots: &[(Pos2, i64, Axis)],
        handles: &[(Pos2, i64, bool, Handle, i64)],
    ) {
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
        let handle_hit = closest(handles, pos, |(at, ..)| *at);
        let key_hit = closest(spots, pos, |(at, ..)| *at);
        let use_handle = match (handle_hit, key_hit) {
            (Some(handle), Some(key)) => handle.0.distance(pos) <= key.0.distance(pos),
            (Some(_), None) => true,
            _ => false,
        };
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
        if use_handle && let Some((_, frame, out, handle, neighbor)) = handle_hit {
            let mark = Mark {
                param: lane.param,
                frame: *frame,
            };
            if resp.clicked() {
                self.key_editor.selected = choose(&self.key_editor.selected, mark, add);
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
                clip: clip.id.clone(),
                kind: Kind::Handle,
                origin: pos,
                ppf: map.ppf(),
                v_span: values.span(),
                v_height: values.height,
                marks: vec![mark],
                axis: Axis::None,
                fix: Some(HandleFix {
                    mark,
                    out: *out,
                    frames: handle.frames,
                    value: handle.value,
                    key_value: row_value,
                    neighbor: *neighbor,
                }),
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
            self.key_editor.selected = vec![mark];
            self.key_editor.menu = Some(mark);
            self.key_editor.menu_curve = curve;
        }
    }

    fn show_menu(&mut self, resp: &egui::Response, clip: &str, lane: &Lane, curve: bool) {
        let open = self
            .key_editor
            .menu
            .is_some_and(|m| m.param == lane.param && self.key_editor.menu_curve == curve);
        if !open {
            return;
        }
        let mark = self.key_editor.menu;
        let shown = resp.context_menu(|ui| {
            let Some(mark) = mark else { return };
            let Some(i) = lane.keys.iter().position(|k| k.frame == mark.frame) else {
                return;
            };
            let current = ease_of(&lane.keys, i);
            for ease in eases(&lane.keys, i) {
                if ui
                    .selectable_label(current == Some(ease), ease_label(ease))
                    .clicked()
                {
                    self.send("keyframes.set", ease_body(clip, mark, ease));
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

fn row(
    ui: &mut egui::Ui,
    h: f32,
    plot_w: f32,
    label: impl FnOnce(&mut egui::Ui),
) -> (Rect, egui::Response) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(LABEL, h), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_size(vec2(LABEL, h));
            label(ui);
        });
        ui.allocate_exact_size(vec2(plot_w, h), Sense::click_and_drag())
    })
    .inner
}

fn live_marks(clip: &Clip, lanes: &[Lane], shift: Option<&Shift>) -> Vec<Mark> {
    lanes
        .iter()
        .flat_map(|lane| {
            shown_lane(clip, lane, shift).into_iter().map(|row| Mark {
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
        Some(shift) if shift.dt != 0 && shift.marks.contains(mark) => clip
            .sound_at(clip.key_time(mark.frame) + shift.dt as f64)
            .round() as i64,
        _ => mark.frame,
    };
    clip.key_time(frame)
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
) {
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
        let source = clip.sound_at(t);
        let Some(v) = at(source) else {
            continue;
        };
        pts.push(pos2(x, values.y(v)));
    }
    if pts.len() >= 2 {
        painter.add(Shape::line(pts, Stroke::new(width, color)));
    }
}

fn shift_of(clip: &Clip, lanes: &[Lane], drag: &Drag, pos: Pos2) -> Shift {
    let dx = pos.x - drag.origin.x;
    let dy = pos.y - drag.origin.y;
    if drag.kind == Kind::Handle
        && let Some(fix) = drag.fix
    {
        let handle = moved_handle(clip, fix, dx, dy, drag.ppf, drag.v_span, drag.v_height);
        let same =
            (handle.frames - fix.frames).abs() < 1e-4 && (handle.value - fix.value).abs() < 1e-6;
        return Shift {
            marks: vec![fix.mark],
            dt: 0,
            dv: 0.0,
            axis: Axis::None,
            handle: (!same).then_some(HandleEdit {
                mark: fix.mark,
                out: fix.out,
                handle,
            }),
        };
    }
    let by = frames_moved(dx, drag.ppf);
    let dt = fitted(clip, lanes, &drag.marks, by);
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
        handle: None,
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

    fn clip() -> Clip {
        Clip {
            id: "c1".into(),
            media: "m1".into(),
            start: 0,
            duration: 100,
            source_in: 0,
            video: true,
            audio: None,
            enabled: true,
            name: None,
            effects: None,
            generator: None,
            title: None,
            picture: None,
            compound: None,
            speed: None,
            retime: vidcut_core::model::ClipRetime::Project,
            color: None,
        }
    }

    fn lane(param: KeyParam, frames: &[i64]) -> Lane {
        Lane {
            param,
            keys: frames.iter().copied().map(key).collect(),
            extra: Vec::new(),
        }
    }

    #[test]
    fn a_click_selects_that_keyframe() {
        let a = mark(KeyParam::Volume, 1);
        let b = mark(KeyParam::Volume, 4);
        assert_eq!(choose(&[a], b, false), vec![b]);
    }

    #[test]
    fn shift_adds_a_keyframe() {
        let a = mark(KeyParam::Zoom, 2);
        let b = mark(KeyParam::Zoom, 6);
        assert_eq!(choose(&[a], b, true), vec![a, b]);
    }

    #[test]
    fn shift_does_not_add_the_same_keyframe_twice() {
        let a = mark(KeyParam::Zoom, 2);
        assert_eq!(choose(&[a], a, true), vec![a]);
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
        assert_eq!(fitted(&clip(), &lanes, &[mark(KeyParam::Volume, 0)], 10), 9);
    }

    #[test]
    fn a_group_stops_at_the_first_collision() {
        let lanes = [
            lane(KeyParam::Position, &[0, 10]),
            lane(KeyParam::Zoom, &[0, 4]),
        ];
        let marks = [mark(KeyParam::Position, 0), mark(KeyParam::Zoom, 0)];
        assert_eq!(fitted(&clip(), &lanes, &marks, 10), 3);
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
            ease_body("c1", mark(KeyParam::Volume, 3), Ease::EaseIn),
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
