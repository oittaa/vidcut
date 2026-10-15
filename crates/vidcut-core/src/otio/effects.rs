//! Clip effects other editors write (deviation X9): which an import keeps,
//! what they are called, and how they are cut and written back.
//!
//! - The editor writes its whole stack of clip effects on every clip
//!   (Transform, Cropping, Pan, …), untouched ones included. An import
//!   keeps the ones in use as [`Effects`] on the clip's picture or sound,
//!   as they came, and a speed change apart.
//! - Keyframe times count from the item's first source frame, so an item
//!   written from another first frame (a trimmed head, a split, a slip)
//!   has its keyframes moved by the difference: they stay on their frames.
//! - A fade belongs to its clip's edge: a split leaves the fade-in on the
//!   first part and the fade-out on the last.
//! - The clip volume is the clip gain (`ClipAudio::gain_db`), written into
//!   a kept clip volume effect or as one of its own.

use serde_json::{Value, json};

use super::{EDITOR, schema};
use crate::model::{ClipAudio, Effects, Kept};

/// The editor's clip volume (and audio fades), whose level vidcut reads as
/// the clip gain.
const VOLUME: &str = "Fairlight Clip Volume and Fades";

/// The editor's fade lengths, in frames: `(parameter, at the clip's head)`.
const FADES: [(&str, bool); 4] = [
    ("videoFaderIn", true),
    ("videoFaderOut", false),
    ("faderIn", true),
    ("faderOut", false),
];

/// A clip volume effect at `db`, in the editor metadata.
pub(super) fn volume(db: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "Effect.1",
        "name": "",
        "effect_name": "Resolve Effect",
        "metadata": { EDITOR: {
            "Effect Name": VOLUME,
            "Enabled": true,
            "Name": "Volume",
            "Type": 62,
            "Parameters": [
                { "Parameter ID": "volume", "Parameter Value": db, "Variant Type": "Double" }
            ],
        } },
    })
}

/// The clip volume of `item` in dB.
pub(super) fn gain(item: &Value) -> Option<f64> {
    let e = item["effects"]
        .as_array()?
        .iter()
        .find(|e| e["metadata"][EDITOR]["Effect Name"] == VOLUME)
        .filter(|e| e["metadata"][EDITOR]["Enabled"] != false)?;
    let level = params(e).find(|p| p["Parameter ID"] == "volume")?;
    // Editors write -16.999998 for -17.
    level["Parameter Value"]
        .as_f64()
        .map(|g| (g * 100.0).round() / 100.0)
}

/// The effects of `item` to keep: those in use; and its speed change, a
/// time effect, if it has one.
pub(super) fn read(item: &Value) -> (Vec<Value>, Option<Value>) {
    let (mut kept, mut speed) = (Vec::new(), None);
    for e in item["effects"].as_array().into_iter().flatten() {
        let s = schema(e);
        if s.starts_with("LinearTimeWarp")
            || s.starts_with("FreezeFrame")
            || s.starts_with("TimeEffect")
        {
            speed.get_or_insert_with(|| e.clone());
        } else if in_use(e) {
            kept.push(e.clone());
        }
    }
    (kept, speed)
}

/// The name effect `e` goes by: `Transform`, `Pan`, `Gaussian Blur` (empty
/// for an unnamed one of another program's).
pub fn name(e: &Value) -> String {
    let r = &e["metadata"][EDITOR];
    [&r["Name"], &r["Effect Name"], &e["effect_name"], &e["name"]]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|n| !n.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Whether effect `e` changes the clip:
/// - one of the editor's, switched on with a setting off its default
///   (Dynamic Zoom moves the picture whenever it is on; the clip volume's
///   level is the clip gain, so only its fades and keyframes count);
/// - or another program's named one, which says nothing of its defaults.
fn in_use(e: &Value) -> bool {
    let r = &e["metadata"][EDITOR];
    let Some(effect) = r["Effect Name"].as_str() else {
        return !name(e).is_empty();
    };
    if r["Enabled"] == false {
        return false;
    }
    let off = |v: &Value, default: &Value| !same_value(v, default);
    effect == "Dynamic Zoom"
        || params(e).any(|p| {
            let default = &p["Default Parameter Value"];
            let value = &p["Parameter Value"];
            let set = if is_fade(p) {
                value.as_f64().is_some_and(|v| v > 0.0)
            } else {
                !(effect == VOLUME && p["Parameter ID"] == "volume") && off(value, default)
            };
            set || p["Key Frames"]
                .as_object()
                .into_iter()
                .flatten()
                .any(|(_, k)| off(&k["Value"], default))
        })
}

/// Whether two parameter values are the same, numbers to a millionth
/// (editors write -16.999998 for -17).
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_value(x, y))
        }
        _ => a == b,
    }
}

/// The editor's parameters of effect `e`.
fn params(e: &Value) -> impl Iterator<Item = &Value> {
    e["metadata"][EDITOR]["Parameters"]
        .as_array()
        .into_iter()
        .flatten()
}

/// The editor's parameter list of effect `e`, to change.
fn params_of(e: &mut Value) -> Option<&mut Vec<Value>> {
    e.pointer_mut(&format!("/metadata/{EDITOR}/Parameters"))
        .and_then(Value::as_array_mut)
}

/// The editor's parameters of all of `list`, to change.
fn params_mut(list: &mut [Kept]) -> impl Iterator<Item = &mut Value> {
    list.iter_mut()
        .flat_map(|e| params_of(e.make_mut()).into_iter().flatten())
}

/// Whether parameter `p` is a fade length, and whether at the head.
fn fade(p: &Value) -> Option<bool> {
    FADES
        .iter()
        .find(|(id, _)| p["Parameter ID"] == *id)
        .map(|&(_, head)| head)
}

fn is_fade(p: &Value) -> bool {
    fade(p).is_some()
}

/// Moves the keyframe times of `list` through `f`.
fn move_keys(list: &mut [Kept], f: &dyn Fn(f64) -> f64) {
    for p in params_mut(list) {
        if let Some(Value::Object(keys)) = p.get_mut("Key Frames") {
            *keys = std::mem::take(keys)
                .into_iter()
                .map(|(t, k)| match t.parse::<f64>() {
                    Ok(at) => (frame_key(f(at)), k),
                    Err(_) => (t, k),
                })
                .collect();
        }
    }
}

/// A keyframe time as the editor writes one: `12`, `-50`.
fn frame_key(at: f64) -> String {
    if at.fract() == 0.0 {
        format!("{}", at as i64)
    } else {
        at.to_string()
    }
}

/// Leaves out the fade at the head and/or the tail of `effects`, and what
/// then changes nothing: what a split or a cut leaves of them.
pub fn cut(effects: &mut Option<Effects>, head: bool, tail: bool) {
    if let Some(e) = effects {
        e.cut(head, tail);
        if e.list.is_empty() {
            *effects = None;
        }
    }
}

/// The effects of two parts of a clip joined into one: the first's fade-in
/// and the second's fade-out. None when that would change what plays: they
/// differ in more than those fades, or a fade sits where they meet.
pub fn join(first: &Option<Effects>, second: &Option<Effects>) -> Option<Option<Effects>> {
    let fades = |e: &Option<Effects>, head: bool| {
        e.iter()
            .flat_map(|e| &e.list)
            .flat_map(|k| params(k))
            .any(|p| {
                fade(p) == Some(head) && p["Parameter Value"].as_f64().is_some_and(|v| v > 0.0)
            })
    };
    let without_fades = |e: &Option<Effects>| {
        let mut e = e.clone();
        cut(&mut e, true, true);
        e
    };
    if fades(first, false) || fades(second, true) || without_fades(first) != without_fades(second) {
        return None;
    }
    let Some(second) = second else {
        return Some(first.clone());
    };
    let mut joined = first.clone().unwrap_or_else(|| Effects {
        from: second.from,
        list: Vec::new(),
    });
    for e in &second.list {
        let mut missing = false;
        for p in params(e).filter(|p| fade(p) == Some(false)) {
            match params_mut(&mut joined.list).find(|q| q["Parameter ID"] == p["Parameter ID"]) {
                Some(q) => q["Parameter Value"] = p["Parameter Value"].clone(),
                None => missing = true,
            }
        }
        if missing {
            joined.list.push(e.clone());
        }
    }
    Some(Some(joined))
}

impl Effects {
    /// Their names, once each: `Transform`, `Cropping`.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for n in self.list.iter().map(|k| name(k)) {
            if !n.is_empty() && !names.contains(&n) {
                names.push(n);
            }
        }
        names
    }

    fn cut(&mut self, head: bool, tail: bool) {
        for p in params_mut(&mut self.list) {
            if fade(p).is_some_and(|at_head| if at_head { head } else { tail }) {
                p["Parameter Value"] = json!(0.0);
            }
        }
        self.list.retain(|k| in_use(k));
    }

    /// Keyframe times and fade lengths through `f`: from another frame
    /// rate to the project's.
    pub fn retime(&mut self, f: impl Fn(f64) -> f64) {
        move_keys(&mut self.list, &f);
        for p in params_mut(&mut self.list) {
            if is_fade(p)
                && let Some(v) = p["Parameter Value"].as_f64()
            {
                p["Parameter Value"] = json!(f(v));
            }
        }
    }

    /// As written on an item whose first source frame is `source`, cut at
    /// its head and/or tail (by a split, or the edges of an export).
    pub fn written(&self, source: i64, head: bool, tail: bool) -> Vec<Value> {
        let mut e = self.clone();
        let by = (self.from - source) as f64;
        if by != 0.0 {
            move_keys(&mut e.list, &|at| at + by);
        }
        if head || tail {
            e.cut(head, tail);
        }
        e.list.into_iter().map(Kept::into_value).collect()
    }
}

/// The effects of a clip's sound as written: those it carries with the
/// clip volume at its gain, or a clip volume of its own when the gain is
/// set.
pub(super) fn sound(au: &ClipAudio, source: i64, head: bool, tail: bool) -> Vec<Value> {
    let mut list = au
        .effects
        .as_ref()
        .map_or_else(Vec::new, |e| e.written(source, head, tail));
    let carried = list
        .iter_mut()
        .find(|e| e["metadata"][EDITOR]["Effect Name"] == VOLUME)
        .and_then(params_of);
    match carried {
        Some(ps) => match ps.iter_mut().find(|p| p["Parameter ID"] == "volume") {
            Some(p) => p["Parameter Value"] = json!(au.gain_db),
            None => ps.push(volume(au.gain_db)["metadata"][EDITOR]["Parameters"][0].clone()),
        },
        None if au.gain_db != 0.0 => list.insert(0, volume(au.gain_db)),
        None => {}
    }
    list
}
