use serde_json::{Map, Value, json};

use super::EDITOR;
use super::effects::{effect, group, handle_point, handle_value, keyed, plain, ranged, rounded};
use crate::model::{
    CompositeMode, Crop, DynamicZoom, Ease, Handle, Keyframe, Picture, VideoInfo, ZoomBox,
};

const TRANSFORM: &str = "Transform";
const CROPPING: &str = "Cropping";
const SCALING: &str = "Retime and Scaling";
const ZOOMING: &str = "Dynamic Zoom";
const COMPOSITE: &str = "Composite";
const NAMES: [&str; 5] = [TRANSFORM, CROPPING, SCALING, ZOOMING, COMPOSITE];

const ZOOM_X: &str = "transformationZoomX";
const ZOOM_Y: &str = "transformationZoomY";
const ZOOM_LINK: &str = "transformationZoomLink";
const PAN: &str = "transformationPan";
const TILT: &str = "transformationTilt";
const ROTATION: &str = "transformationRotationAngle";
const PITCH: &str = "transformationPitch";
const YAW: &str = "transformationYaw";
const ANCHOR: &str = "transformationAnchorPoint";
const FLIP_X: &str = "transformationFlipX";
const FLIP_Y: &str = "transformationFlipY";
const CROPS: [&str; 4] = ["cropLeft", "cropRight", "cropTop", "cropBottom"];
const SOFTNESS: &str = "cropSoftness";
const CROPPED: [&str; 5] = [CROPS[0], CROPS[1], CROPS[2], CROPS[3], SOFTNESS];
const OPACITY: &str = "opacity";
const MODE: &str = "composite mode";
const SCALE: &str = "scale";
const EASE: &str = "dynamicZoomEase";
const CENTER: &str = "dynamicZoomCenter";
const SIZE: &str = "dynamicZoomScale";

const FIT: u8 = 2;
const REACH: f64 = 4.0;
const TURNS: f64 = 3600.0;
const LEAN: f64 = 1.5;
const LATER: i64 = 1000;
const SIZES: (f64, f64) = (0.01, 100.0);
const SOFT: f64 = 100.0;
const PERCENT: f64 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    frame: [f64; 2],
    fitted: [f64; 2],
    source: [f64; 2],
}

impl Shape {
    pub fn of(v: &VideoInfo, (w, h): (u32, u32)) -> Self {
        let sar = (v.sar[0], v.sar[1]);
        let (fw, fh) = crate::place::upright_fit((v.width, v.height), sar, v.rotation, (w, h));
        let (dw, dh) = v.display_size();
        Self {
            frame: [w.into(), h.into()],
            fitted: [fw.into(), fh.into()],
            source: [dw.into(), dh.into()],
        }
    }

    pub fn whole((w, h): (u32, u32)) -> Self {
        let frame = [w.into(), h.into()];
        Self {
            frame,
            fitted: frame,
            source: frame,
        }
    }

    fn picture(&self, scale: u8) -> [f64; 2] {
        let ([w, h], [dw, dh]) = (self.frame, self.source);
        match scale {
            1 => self.source,
            3 => {
                let s = (w / dw).max(h / dh);
                [dw * s, dh * s]
            }
            4 => self.frame,
            _ => self.fitted,
        }
    }
}

fn kind(e: &Value) -> Option<&'static str> {
    let name = e["metadata"][EDITOR]["Effect Name"].as_str()?;
    let found = NAMES.into_iter().find(|n| *n == name)?;
    (found != COMPOSITE || mixable(e)).then_some(found)
}

fn mode_of(v: &Value) -> Option<CompositeMode> {
    let code = v.as_f64().filter(|c| c.fract() == 0.0 && *c >= 0.0)?;
    CompositeMode::of_code(code as u32)
}

fn mixable(e: &Value) -> bool {
    e["metadata"][EDITOR]["Parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["Parameter ID"] == MODE)
        .all(|p| mode_of(&p["Parameter Value"]).is_some() && !keyframed(p))
}

pub fn is_placing(e: &Value) -> bool {
    kind(e).is_some()
}

fn known(name: &str) -> &'static [&'static str] {
    match name {
        TRANSFORM => &[
            ZOOM_X, ZOOM_Y, ZOOM_LINK, PAN, TILT, ROTATION, PITCH, YAW, ANCHOR, FLIP_X, FLIP_Y,
        ],
        CROPPING => &CROPPED,
        ZOOMING => &[EASE, CENTER, SIZE],
        COMPOSITE => &[OPACITY, MODE],
        _ => &[SCALE],
    }
}

fn animated(name: &str, id: &str) -> bool {
    name == ZOOMING && (id == CENTER || id == SIZE)
}

fn playable(name: &str, id: &str) -> bool {
    match name {
        TRANSFORM => matches!(id, ZOOM_X | ZOOM_Y | PAN | TILT | ROTATION),
        CROPPING => CROPPED.contains(&id),
        COMPOSITE => id == OPACITY,
        _ => false,
    }
}

fn plain_keys(p: &Value) -> bool {
    p["Key Frames"].as_object().is_some_and(|keys| {
        keys.values().all(|k| {
            k.as_object().is_some_and(|o| {
                o.keys()
                    .all(|n| matches!(n.as_str(), "Value" | "Variant Type" | "InBez" | "OutBez"))
            })
        })
    })
}

fn keyframed(p: &Value) -> bool {
    match &p["Key Frames"] {
        Value::Null => false,
        Value::Object(keys) => !keys.is_empty(),
        Value::Array(keys) => !keys.is_empty(),
        _ => true,
    }
}

type Parameters<'a> = (Vec<&'static str>, Vec<(&'a str, &'a Value)>);

fn parameters(effects: &[Value]) -> Option<Parameters<'_>> {
    let (mut found, mut values) = (Vec::new(), Vec::new());
    for name in NAMES {
        let mut of = effects
            .iter()
            .filter(|e| kind(e) == Some(name) && e["metadata"][EDITOR]["Enabled"] != false);
        let first = of.next();
        if of.next().is_some() {
            return None;
        }
        let Some(e) = first else {
            continue;
        };
        found.push(name);
        for p in e["metadata"][EDITOR]["Parameters"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let id = p["Parameter ID"].as_str()?;
            if !known(name).contains(&id) {
                return None;
            }
            let keyed = keyframed(p);
            if keyed && animated(name, id) {
                values.push((id, p));
                continue;
            }
            if keyed && !(playable(name, id) && plain_keys(p)) {
                return None;
            }
            values.push((id, p));
        }
    }
    (!found.is_empty()).then_some((found, values))
}

fn ease_code(e: Ease) -> u8 {
    match e {
        Ease::Linear => 0,
        Ease::EaseIn => 1,
        Ease::EaseOut => 2,
        Ease::EaseInOut => 3,
    }
}

fn ease_of(v: &Value) -> Option<Ease> {
    let code = v.as_f64().filter(|c| c.fract() == 0.0)?;
    Ease::ALL
        .into_iter()
        .find(|e| f64::from(ease_code(*e)) == code)
}

fn ends(p: &Value) -> Option<[&Value; 2]> {
    let keys = p["Key Frames"].as_object().filter(|k| k.len() == 2)?;
    let mut at: Vec<(f64, &Value)> = keys
        .iter()
        .map(|(t, k)| {
            Some((
                t.parse::<f64>().ok().filter(|t| t.is_finite())?,
                &k["Value"],
            ))
        })
        .collect::<Option<_>>()?;
    at.sort_by(|a, b| a.0.total_cmp(&b.0));
    (at[0].0 < at[1].0).then(|| [at[0].1, at[1].1])
}

fn zoom_of(ease: Option<&Value>, center: &Value, size: &Value) -> Option<DynamicZoom> {
    let ease = match ease {
        None => Ease::Linear,
        Some(p) => ease_of(&p["Parameter Value"])?,
    };
    let [c0, c1] = ends(center)?;
    let [s0, s1] = ends(size)?;
    let at = |c: &Value, s: &Value| -> Option<ZoomBox> {
        let a = c.as_array().filter(|a| a.len() == 2)?;
        let x = |i: usize| a[i].as_f64().filter(|v| v.is_finite());
        Some(ZoomBox {
            center: [rounded(0.5 + x(0)?), rounded(0.5 - x(1)?)],
            size: rounded(s.as_f64().filter(|v| v.is_finite())?),
        })
    };
    Some(DynamicZoom {
        start: at(c0, s0)?,
        end: at(c1, s1)?,
        ease,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    zoom: [f64; 2],
    moved: [f64; 2],
    turn: f64,
    pitch: f64,
    yaw: f64,
    anchor: [f64; 2],
    flips: [bool; 2],
    crop: [f64; 4],
    softness: f64,
    scale: u8,
    dynamic: Option<DynamicZoom>,
    mode: CompositeMode,
    opacity: f64,
}

fn placement(effects: &[Value]) -> Option<Placement> {
    let (found, values) = parameters(effects)?;
    let param = |id: &str| values.iter().find(|(k, _)| *k == id).map(|(_, p)| *p);
    let given = |id: &str| param(id).map(|p| &p["Parameter Value"]);
    let number = |id: &str, usual: f64| match given(id) {
        None => Some(usual),
        Some(v) => v.as_f64().filter(|v| v.is_finite()),
    };
    let switch = |id: &str| match given(id) {
        None => Some(false),
        Some(v) => v.as_bool().or_else(|| v.as_f64().map(|x| x != 0.0)),
    };
    let anchor = match given(ANCHOR) {
        None => [0.0, 0.0],
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() == 2)?;
            let x = |i: usize| a[i].as_f64().filter(|v| v.is_finite());
            [x(0)?, x(1)?]
        }
    };
    let scale = match given(SCALE) {
        None => 0,
        Some(v) => {
            let s = v
                .as_f64()
                .filter(|s| s.fract() == 0.0 && (0.0..=4.0).contains(s))?;
            s as u8
        }
    };
    let crop = [
        number(CROPS[0], 0.0)?,
        number(CROPS[1], 0.0)?,
        number(CROPS[2], 0.0)?,
        number(CROPS[3], 0.0)?,
    ];
    let dynamic = match found.contains(&ZOOMING) {
        true => Some(zoom_of(param(EASE), param(CENTER)?, param(SIZE)?)?),
        false => None,
    };
    Some(Placement {
        zoom: [number(ZOOM_X, 1.0)?, number(ZOOM_Y, 1.0)?],
        moved: [number(PAN, 0.0)?, number(TILT, 0.0)?],
        turn: number(ROTATION, 0.0)?,
        pitch: number(PITCH, 0.0)?,
        yaw: number(YAW, 0.0)?,
        anchor,
        flips: [switch(FLIP_X)?, switch(FLIP_Y)?],
        crop,
        softness: number(SOFTNESS, 0.0)?,
        scale,
        dynamic,
        mode: match given(MODE) {
            None => CompositeMode::Normal,
            Some(v) => mode_of(v)?,
        },
        opacity: number(OPACITY, PERCENT)? / PERCENT,
    })
}

impl Placement {
    fn picture(&self, shape: &Shape) -> Picture {
        let [pw, ph] = shape.picture(self.scale);
        let ([w, h], [fw, fh]) = (shape.frame, shape.fitted);
        let ([zx, zy], [ax, ay]) = (self.zoom, self.anchor);
        let d = [
            ax * (pw - fw) + self.moved[0] * pw,
            -ay * (ph - fh) - self.moved[1] * ph,
        ];
        let turn = if self.turn.abs() > TURNS {
            self.turn % 360.0
        } else {
            self.turn
        };
        let [left, right, top, bottom] = self.crop.map(rounded);
        let pitch = rounded(self.pitch);
        let yaw = rounded(self.yaw);
        let softness = rounded(self.softness);
        let opacity = rounded(self.opacity);
        Picture {
            position: Some([rounded(0.5 + d[0] / w), rounded(0.5 + d[1] / h)]),
            zoom: Some([rounded(pw / fw * zx), rounded(ph / fh * zy)]),
            rotation: Some(rounded(turn)),
            anchor: Some([rounded(0.5 + ax), rounded(0.5 - ay)]),
            pitch: (pitch != 0.0).then_some(pitch),
            yaw: (yaw != 0.0).then_some(yaw),
            flip_horizontal: self.flips[0],
            flip_vertical: self.flips[1],
            crop: Crop {
                left,
                right,
                top,
                bottom,
            },
            crop_softness: (softness != 0.0).then_some(softness),
            dynamic_zoom: self.dynamic,
            composite_mode: (self.mode != CompositeMode::Normal).then_some(self.mode),
            opacity: (opacity != 1.0).then_some(opacity),
            ..Picture::default()
        }
    }

    fn with_keys(
        self,
        effects: &[Value],
        shape: &Shape,
        timing: &super::effects::Timing,
        origin: i64,
    ) -> Option<Picture> {
        let (_, values) = parameters(effects)?;
        let keys = |id: &str| match values.iter().find(|(k, _)| *k == id) {
            None => Some(Vec::new()),
            Some((_, p)) => read_keys(p, timing, origin),
        };
        let mut picture = self.picture(shape);
        let zoom = merge(keys(ZOOM_X)?, keys(ZOOM_Y)?, self.zoom[0], self.zoom[1])?;
        let [pw, ph] = shape.picture(self.scale);
        let [fw, fh] = shape.fitted;
        (picture.zoom_keys, picture.zoom_y) = scaled_pair(zoom, pw / fw, ph / fh);
        let moved = merge(keys(PAN)?, keys(TILT)?, self.moved[0], self.moved[1])?;
        (picture.position_keys, picture.position_y) =
            placed_pair(moved, self.anchor, self.scale, shape);
        picture.rotation_keys = keys(ROTATION)?
            .into_iter()
            .map(|k| Keyframe {
                value: if k.value.abs() > TURNS {
                    rounded(k.value % 360.0)
                } else {
                    rounded(k.value)
                },
                ..k
            })
            .collect();
        let crop = [
            keys(CROPS[0])?,
            keys(CROPS[1])?,
            keys(CROPS[2])?,
            keys(CROPS[3])?,
        ];
        picture.crop_left_keys = crop[0]
            .iter()
            .map(|k| Keyframe {
                value: rounded(k.value),
                ..*k
            })
            .collect();
        picture.crop_right_keys = crop[1]
            .iter()
            .map(|k| Keyframe {
                value: rounded(k.value),
                ..*k
            })
            .collect();
        picture.crop_top_keys = crop[2]
            .iter()
            .map(|k| Keyframe {
                value: rounded(k.value),
                ..*k
            })
            .collect();
        picture.crop_bottom_keys = crop[3]
            .iter()
            .map(|k| Keyframe {
                value: rounded(k.value),
                ..*k
            })
            .collect();
        picture.crop_softness_keys = keys(SOFTNESS)?
            .into_iter()
            .map(|k| Keyframe {
                value: rounded(k.value),
                ..k
            })
            .collect();
        picture.opacity_keys = keys(OPACITY)?
            .into_iter()
            .map(|k| Keyframe {
                value: rounded(k.value / PERCENT),
                in_handle: scale_handle(k.in_handle, 1.0 / PERCENT),
                out_handle: scale_handle(k.out_handle, 1.0 / PERCENT),
                ..k
            })
            .collect();
        Some(picture)
    }
}

pub fn taken(effects: &[Value], ours: bool) -> bool {
    ours || placement(effects).is_some()
}

pub fn take(
    effects: &[Value],
    ours: Option<&Picture>,
    shape: &Shape,
    origin: i64,
    speed: f64,
) -> Result<(Vec<Value>, Option<Picture>), Vec<String>> {
    let rest = || -> Vec<Value> { effects.iter().filter(|e| !is_placing(e)).cloned().collect() };
    if let Some(p) = ours {
        return Ok((rest(), Some(p.clone())));
    }
    let Some(read) = placement(effects) else {
        return Ok((effects.to_vec(), None));
    };
    let timing = super::effects::Timing { speed };
    let Some(picture) = read.with_keys(effects, shape, &timing, origin) else {
        return Ok((effects.to_vec(), None));
    };
    match crate::ops::picture::checked(&picture) {
        Ok(picture) => Ok((rest(), picture)),
        Err(_) => {
            let mut names: Vec<String> = Vec::new();
            for n in effects
                .iter()
                .filter(|e| is_placing(e))
                .map(super::effects::name)
            {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            Err(names)
        }
    }
}

fn moved(p: &Picture, shape: &Shape) -> [f64; 2] {
    let [x, y] = p.place();
    let ([w, h], [fw, fh]) = (shape.frame, shape.fitted);
    [rounded((x - 0.5) * w / fw), rounded(-(y - 0.5) * h / fh)]
}

pub fn reaches(p: &Picture, shape: &Shape) -> bool {
    moved(p, shape).iter().all(|v| v.abs() <= REACH)
}

fn moved_at([x, y]: [f64; 2], shape: &Shape) -> [f64; 2] {
    let ([w, h], [fw, fh]) = (shape.frame, shape.fitted);
    [rounded((x - 0.5) * w / fw), rounded(-(y - 0.5) * h / fh)]
}

fn read_keys(p: &Value, timing: &super::effects::Timing, origin: i64) -> Option<Vec<Keyframe>> {
    let Some(keys) = p.get("Key Frames").and_then(Value::as_object) else {
        return Some(Vec::new());
    };
    if keys.is_empty() {
        return Some(Vec::new());
    }
    let [before, after] = timing.handle_names();
    let mut out = Vec::new();
    for (t, k) in keys {
        let at = t.parse::<f64>().ok()?;
        let frame = (timing.source(at) + origin as f64).round() as i64;
        let value = k["Value"].as_f64()?;
        let handle = |name: &str, before: bool| {
            let b = handle_point(k, name)?;
            let h = Handle {
                frames: b.first()?.as_f64()? * timing.scale(),
                value: b.get(1)?.as_f64()?,
            };
            crate::keyframes::checked_handle(h, before).ok()
        };
        out.push(Keyframe {
            frame,
            value,
            in_handle: handle(before, true),
            out_handle: handle(after, false),
        });
    }
    out.sort_by_key(|k| k.frame);
    out.dedup_by_key(|k| k.frame);
    Some(out)
}

fn merge(
    x: Vec<Keyframe>,
    y: Vec<Keyframe>,
    x_plain: f64,
    y_plain: f64,
) -> Option<(Vec<Keyframe>, Vec<f64>)> {
    if x.is_empty() && y.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    if x.is_empty() {
        let ys: Vec<f64> = y.iter().map(|k| k.value).collect();
        let keys = y
            .into_iter()
            .map(|k| Keyframe {
                value: x_plain,
                ..k
            })
            .collect();
        return Some((keys, ys));
    }
    if y.is_empty() {
        return Some((x.clone(), vec![y_plain; x.len()]));
    }
    if x.len() != y.len() || x.iter().zip(&y).any(|(a, b)| a.frame != b.frame) {
        return None;
    }
    Some((x, y.into_iter().map(|k| k.value).collect()))
}

fn scale_handle(h: Option<Handle>, factor: f64) -> Option<Handle> {
    h.map(|h| Handle {
        frames: h.frames,
        value: h.value * factor,
    })
}

fn scaled_pair((x, y): (Vec<Keyframe>, Vec<f64>), fx: f64, fy: f64) -> (Vec<Keyframe>, Vec<f64>) {
    if x.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let keys = x
        .into_iter()
        .map(|k| Keyframe {
            value: rounded(k.value * fx),
            in_handle: scale_handle(k.in_handle, fx),
            out_handle: scale_handle(k.out_handle, fx),
            ..k
        })
        .collect();
    (keys, y.into_iter().map(|v| rounded(v * fy)).collect())
}

fn placed_pair(
    (x, y): (Vec<Keyframe>, Vec<f64>),
    anchor: [f64; 2],
    scale: u8,
    shape: &Shape,
) -> (Vec<Keyframe>, Vec<f64>) {
    if x.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let [pw, _] = shape.picture(scale);
    let ([w, _], _) = (shape.frame, shape.fitted);
    let fx = pw / w;
    let mut keys = Vec::new();
    let mut ys = Vec::new();
    for (k, tilt) in x.into_iter().zip(y) {
        let at = position_of(k.value, tilt, anchor, scale, shape);
        keys.push(Keyframe {
            value: at[0],
            in_handle: scale_handle(k.in_handle, fx),
            out_handle: scale_handle(k.out_handle, fx),
            frame: k.frame,
        });
        ys.push(at[1]);
    }
    (keys, ys)
}

fn position_of(pan: f64, tilt: f64, anchor: [f64; 2], scale: u8, shape: &Shape) -> [f64; 2] {
    let [pw, ph] = shape.picture(scale);
    let ([w, h], [fw, fh]) = (shape.frame, shape.fitted);
    let [ax, ay] = anchor;
    let d = [ax * (pw - fw) + pan * pw, -ay * (ph - fh) - tilt * ph];
    [rounded(0.5 + d[0] / w), rounded(0.5 + d[1] / h)]
}

fn file_curve(
    keys: &[Keyframe],
    y: Option<&[f64]>,
    source: i64,
    timing: &super::effects::Timing,
    map: impl Fn(f64) -> f64,
    slope: f64,
) -> Option<Value> {
    if keys.is_empty() {
        return None;
    }
    let mut out = serde_json::Map::new();
    for (i, k) in keys.iter().enumerate() {
        let at = timing.file((k.frame - source) as f64).round();
        let raw = y.and_then(|ys| ys.get(i).copied()).unwrap_or(k.value);
        let mut v = json!({ "Value": map(raw), "Variant Type": "Double" });
        let bez = |name: &str, h: Handle| {
            let value = match y {
                Some(_) => 0.0,
                None => h.value * slope,
            };
            handle_value(name, h.frames / timing.scale(), value)
        };
        let [before, after] = timing.handle_names();
        if let Some(h) = k.in_handle {
            v[before] = bez(before, h);
        }
        if let Some(h) = k.out_handle {
            v[after] = bez(after, h);
        }
        let name = if at.fract() == 0.0 {
            format!("{}", at as i64)
        } else {
            at.to_string()
        };
        out.insert(name, v);
    }
    Some(Value::Object(out))
}

fn double(id: &str, value: f64, usual: f64, (low, high): (f64, f64)) -> Option<Value> {
    (value != usual).then(|| ranged(keyed(id, json!(value), json!(usual), "Double"), low, high))
}

fn double_frames(
    id: &str,
    value: f64,
    usual: f64,
    range: (f64, f64),
    frames: Option<Value>,
) -> Option<Value> {
    match frames {
        None => double(id, value, usual, range),
        Some(frames) => {
            let mut p = ranged(
                keyed(id, json!(value), json!(usual), "Double"),
                range.0,
                range.1,
            );
            p["Key Frames"] = frames;
            Some(p)
        }
    }
}

fn boolean(id: &str, value: bool, usual: bool) -> Option<Value> {
    (value != usual).then(|| keyed(id, json!(value), json!(usual), "Bool"))
}

fn point(id: &str, value: [f64; 2], usual: [f64; 2]) -> Option<Value> {
    (value != usual).then(|| keyed(id, json!(value), json!(usual), "POINTF"))
}

fn zooming(z: &DynamicZoom, g: i64) -> Vec<Value> {
    let keys = |a: Value, b: Value, variant: &str| {
        let mut m = Map::new();
        for (at, v) in [(-g, a), (LATER - g, b)] {
            m.insert(
                at.to_string(),
                json!({ "Value": v, "Variant Type": variant }),
            );
        }
        Value::Object(m)
    };
    let middle = |b: &ZoomBox| json!([rounded(b.center[0] - 0.5), rounded(0.5 - b.center[1])]);
    let mut center = plain(CENTER, json!([0.0, 0.0]), json!([0.0, 0.0]), "POINTF");
    center["Key Frames"] = keys(middle(&z.start), middle(&z.end), "POINTF");
    let mut size = ranged(
        plain(SIZE, json!(1.0), json!(1.0), "Double"),
        SIZES.0,
        SIZES.1,
    );
    size["Key Frames"] = keys(json!(z.start.size), json!(z.end.size), "Double");
    let ease = keyed(EASE, json!(ease_code(z.ease)), json!(0), "UInt");
    vec![ease, center, size]
}

pub(crate) fn effects(
    p: &Picture,
    shape: &Shape,
    g: i64,
    source: i64,
    timing: &super::effects::Timing,
) -> Vec<Value> {
    let [zx, zy] = p.scale();
    let [ax, ay] = p.pivot();
    let [pan, tilt] = moved(p, shape).map(|v| v.clamp(-REACH, REACH));
    let (zoom, reach) = ((0.0, 100.0), (-REACH, REACH));
    let (sx, sy) = (
        shape.frame[0] / shape.fitted[0],
        shape.frame[1] / shape.fitted[1],
    );
    let zoom_x = file_curve(&p.zoom_keys, None, source, timing, |v| v, 1.0);
    let zoom_y = file_curve(&p.zoom_keys, Some(&p.zoom_y), source, timing, |v| v, 1.0);
    let pan_keys = file_curve(
        &p.position_keys,
        None,
        source,
        timing,
        |v| moved_at([v, 0.5], shape)[0],
        sx,
    );
    let tilt_keys = file_curve(
        &p.position_keys,
        Some(&p.position_y),
        source,
        timing,
        |v| moved_at([0.5, v], shape)[1],
        -sy,
    );
    let turn_keys = file_curve(&p.rotation_keys, None, source, timing, |v| v, 1.0);
    let zx = p.zoom_keys.first().map_or(zx, |k| k.value);
    let zy = p.zoom_y.first().copied().unwrap_or(zy);
    let pan = p
        .position_keys
        .first()
        .map_or(pan, |k| moved_at([k.value, 0.5], shape)[0]);
    let tilt = p
        .position_y
        .first()
        .copied()
        .map_or(tilt, |y| moved_at([0.5, y], shape)[1]);
    let turn = p.rotation_keys.first().map_or(p.turn(), |k| k.value);
    let linked = match p.zoom_keys.is_empty() {
        true => zx == zy,
        false => p
            .zoom_keys
            .iter()
            .zip(&p.zoom_y)
            .all(|(k, y)| k.value == *y),
    };
    let transform: Vec<Value> = [
        double_frames(ZOOM_X, zx, 1.0, zoom, zoom_x),
        double_frames(ZOOM_Y, zy, 1.0, zoom, zoom_y),
        boolean(ZOOM_LINK, linked, true),
        double_frames(PAN, pan, 0.0, reach, pan_keys),
        double_frames(TILT, tilt, 0.0, reach, tilt_keys),
        double_frames(ROTATION, turn, 0.0, (-100000.0, 100000.0), turn_keys),
        double(PITCH, p.pitch(), 0.0, (-LEAN, LEAN)),
        double(YAW, p.yaw(), 0.0, (-LEAN, LEAN)),
        point(ANCHOR, [rounded(ax - 0.5), rounded(0.5 - ay)], [0.0, 0.0]),
        boolean(FLIP_X, p.flip_horizontal, false),
        boolean(FLIP_Y, p.flip_vertical, false),
    ]
    .into_iter()
    .flatten()
    .collect();
    let crop_keys = [
        &p.crop_left_keys,
        &p.crop_right_keys,
        &p.crop_top_keys,
        &p.crop_bottom_keys,
    ];
    let mut cropping: Vec<Value> = CROPS
        .into_iter()
        .zip(p.crop.sides())
        .zip(crop_keys)
        .filter_map(|((id, v), keys)| {
            let frames = file_curve(keys, None, source, timing, |n| n, 1.0);
            let value = keys.first().map_or(v, |k| k.value);
            double_frames(id, value, 0.0, (0.0, 1.0), frames)
        })
        .collect();
    let soft_keys = file_curve(&p.crop_softness_keys, None, source, timing, |n| n, 1.0);
    let softness = p
        .crop_softness_keys
        .first()
        .map_or(p.softness(), |k| k.value);
    cropping.extend(double_frames(
        SOFTNESS,
        softness,
        0.0,
        (-SOFT, SOFT),
        soft_keys,
    ));
    let scaling = vec![keyed(SCALE, json!(FIT), json!(0), "UInt")];
    let dynamic = p.dynamic_zoom.map_or_else(Vec::new, |z| zooming(&z, g));
    let opacity_keys = file_curve(
        &p.opacity_keys,
        None,
        source,
        timing,
        |n| rounded(n * PERCENT),
        PERCENT,
    );
    let opacity = p.opacity_keys.first().map_or(p.opaque(), |k| k.value);
    let mode = (p.mode() != CompositeMode::Normal)
        .then(|| keyed(MODE, json!(p.mode().code()), json!(0), "UInt"));
    let composite: Vec<Value> = mode
        .into_iter()
        .chain(double_frames(
            OPACITY,
            rounded(opacity * PERCENT),
            PERCENT,
            (0.0, PERCENT),
            opacity_keys,
        ))
        .collect();
    [
        (TRANSFORM, 2, transform),
        (CROPPING, 3, cropping),
        (SCALING, 22, scaling),
        (ZOOMING, 59, dynamic),
        (COMPOSITE, 1, composite),
    ]
    .into_iter()
    .filter(|(_, _, list)| !list.is_empty())
    .map(|(name, kind, list)| effect(group(name, kind, 1, list)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::Placing;
    use crate::render::Placed;
    use crate::test_support::still_info;

    fn given(transform: &[(&str, Value)], scale: u8) -> Vec<Value> {
        let param = |(id, v): &(&str, Value)| json!({ "Parameter ID": id, "Parameter Value": v });
        let made = |name: &str, kind: u32, params: Vec<Value>| {
            json!({
                "OTIO_SCHEMA": "Effect.1",
                "metadata": { EDITOR: {
                    "Effect Name": name, "Enabled": true, "Name": name,
                    "Parameters": params, "Type": kind,
                } },
            })
        };
        vec![
            made(TRANSFORM, 2, transform.iter().map(param).collect()),
            made(SCALING, 22, vec![param(&(SCALE, json!(scale)))]),
        ]
    }

    fn filled() -> Shape {
        let v = still_info(1440, 1080).video.expect("video");
        Shape::of(&v, (1920, 1080))
    }

    fn filled_turned() -> Vec<Value> {
        let transform = [
            (ZOOM_X, json!(1.5)),
            (ZOOM_Y, json!(1.5)),
            (ROTATION, json!(90.0)),
            (ANCHOR, json!([0.25, 0.1])),
        ];
        given(&transform, 3)
    }

    fn steady() -> crate::otio::effects::Timing {
        crate::otio::effects::Timing { speed: 1.0 }
    }

    fn read(effects: &[Value], shape: &Shape) -> Option<Picture> {
        take(effects, None, shape, 0, 1.0).expect("read").1
    }

    #[test]
    fn a_fitted_pictures_anchor_leaves_its_position_to_the_pan() {
        let transform = [
            (ZOOM_X, json!(2.0)),
            (ZOOM_Y, json!(2.0)),
            (ROTATION, json!(30.0)),
            (PAN, json!(0.1)),
            (ANCHOR, json!([0.25, 0.1])),
        ];
        let got = read(&given(&transform, FIT), &Shape::whole((1920, 1080)));
        let want = Picture {
            position: Some([0.6, 0.5]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(30.0),
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        assert_eq!(got, Some(want));
    }

    #[test]
    fn a_filled_pictures_scaling_move_goes_into_its_position() {
        let want = Picture {
            position: Some([0.5625, 0.466667]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(90.0),
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        assert_eq!(read(&filled_turned(), &filled()), Some(want));
    }

    #[test]
    fn a_filled_pictures_middle_lands_where_the_file_puts_it() {
        let picture = read(&filled_turned(), &filled()).expect("a picture");
        let placed = Placed {
            clip: "c1".into(),
            picture,
            run: None,
            source: 0,
            pace: crate::render::Pace::default(),
            start: 0,
            ready: true,
        };
        let fitted = [240.0, 0.0, 1440.0, 1080.0];
        let level = Placing::new(&[placed], (1920.0, 1080.0), fitted).levels[0];
        let middle = level
            .forward([0.5, 0.5])
            .map(|v| (v * 100.0).round() / 100.0);
        assert_eq!(middle, [1656.0, 1116.0]);
    }

    #[test]
    fn a_transform_without_an_anchor_reads_without_one() {
        let transform = [(ZOOM_X, json!(2.0)), (ZOOM_Y, json!(2.0))];
        let got = read(&given(&transform, FIT), &Shape::whole((1920, 1080)));
        let want = Picture {
            zoom: Some([2.0, 2.0]),
            ..Picture::default()
        };
        assert_eq!(got, Some(want));
    }

    #[test]
    fn the_anchor_is_written_from_the_middle_with_y_up() {
        let p = Picture {
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        let written = effects(&p, &Shape::whole((1920, 1080)), 0, 0, &steady());
        let anchor = written[0]["metadata"][EDITOR]["Parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|q| q["Parameter ID"] == ANCHOR)
            .map(|q| q["Parameter Value"].clone());
        assert_eq!(anchor, Some(json!([0.25, 0.1])));
    }

    #[test]
    fn a_written_anchor_reads_back_the_same() {
        let p = Picture {
            position: Some([0.6, 0.5]),
            zoom: Some([2.0, 2.0]),
            rotation: Some(30.0),
            anchor: Some([0.75, 0.4]),
            ..Picture::default()
        };
        let shape = Shape::whole((1920, 1080));
        assert_eq!(read(&effects(&p, &shape, 0, 0, &steady()), &shape), Some(p));
    }

    #[test]
    fn a_written_pitch_and_yaw_read_back_the_same() {
        let p = Picture {
            pitch: Some(0.25),
            yaw: Some(-0.5),
            ..Picture::default()
        };
        let shape = Shape::whole((1920, 1080));
        assert_eq!(read(&effects(&p, &shape, 0, 0, &steady()), &shape), Some(p));
    }

    #[test]
    fn an_out_handle_offset_is_scaled_on_x_and_zero_on_y() {
        let handle = Handle {
            frames: 12.0,
            value: 4.0,
        };
        let at = |value| Keyframe {
            frame: 0,
            value,
            in_handle: None,
            out_handle: Some(handle),
        };
        let p = Picture {
            position_keys: vec![at(0.5)],
            position_y: vec![0.5],
            zoom_keys: vec![at(1.0)],
            zoom_y: vec![1.0],
            ..Picture::default()
        };
        let v = still_info(960, 1080).video.expect("video");
        let written = effects(&p, &Shape::of(&v, (1920, 1080)), 0, 0, &steady());
        let out = |id: &str| {
            written[0]["metadata"][EDITOR]["Parameters"]
                .as_array()
                .and_then(|ps| ps.iter().find(|q| q["Parameter ID"] == id))
                .and_then(|q| q["Key Frames"]["0"]["OutBez"]["OutBez"].get(1))
                .and_then(Value::as_f64)
                .expect("out handle")
        };
        assert_eq!(out(PAN), 8.0);
        assert_eq!(out(TILT), 0.0);
        assert_eq!((out(ZOOM_X), out(ZOOM_Y)), (4.0, 0.0));
    }
}
