use serde_json::{Value, json};

use super::{EDITOR, schema};
use crate::model::{ClipAudio, Effects, Kept};

const VOLUME: &str = "Fairlight Clip Volume and Fades";

const FADES: [(&str, bool); 4] = [
    ("videoFaderIn", true),
    ("videoFaderOut", false),
    ("faderIn", true),
    ("faderOut", false),
];

pub(super) fn rounded(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

pub(super) fn plain(id: &str, value: Value, usual: Value, variant: &str) -> Value {
    json!({
        "Default Parameter Value": usual,
        "Parameter ID": id,
        "Parameter Value": value,
        "Variant Type": variant,
    })
}

pub(super) fn keyed(id: &str, value: Value, usual: Value, variant: &str) -> Value {
    let mut p = plain(id, value, usual, variant);
    p["Key Frames"] = json!({});
    p
}

pub(super) fn ranged(mut p: Value, low: f64, high: f64) -> Value {
    p["minValue"] = json!(low);
    p["maxValue"] = json!(high);
    p
}

pub(super) fn group(name: &str, kind: u32, shown: u32, parameters: Vec<Value>) -> Value {
    json!({
        "Display Type": shown,
        "Effect Name": name,
        "Enabled": true,
        "Name": name,
        "Parameters": parameters,
        "Type": kind,
    })
}

pub(super) fn effect(group: Value) -> Value {
    json!({
        "OTIO_SCHEMA": "Effect.1",
        "metadata": { EDITOR: group },
        "name": "",
        "effect_name": "Resolve Effect",
    })
}

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

pub(super) fn gain(item: &Value) -> Option<f64> {
    let e = item["effects"]
        .as_array()?
        .iter()
        .find(|e| e["metadata"][EDITOR]["Effect Name"] == VOLUME)
        .filter(|e| e["metadata"][EDITOR]["Enabled"] != false)?;
    let level = params(e).find(|p| p["Parameter ID"] == "volume")?;
    level["Parameter Value"]
        .as_f64()
        .map(|g| (g * 100.0).round() / 100.0)
}

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

pub fn name(e: &Value) -> String {
    let r = &e["metadata"][EDITOR];
    [&r["Name"], &r["Effect Name"], &e["effect_name"], &e["name"]]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|n| !n.is_empty())
        .unwrap_or("")
        .to_string()
}

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

fn params(e: &Value) -> impl Iterator<Item = &Value> {
    e["metadata"][EDITOR]["Parameters"]
        .as_array()
        .into_iter()
        .flatten()
}

fn params_of(e: &mut Value) -> Option<&mut Vec<Value>> {
    e.pointer_mut(&format!("/metadata/{EDITOR}/Parameters"))
        .and_then(Value::as_array_mut)
}

fn params_mut(list: &mut [Kept]) -> impl Iterator<Item = &mut Value> {
    list.iter_mut()
        .flat_map(|e| params_of(e.make_mut()).into_iter().flatten())
}

fn fade(p: &Value) -> Option<bool> {
    FADES
        .iter()
        .find(|(id, _)| p["Parameter ID"] == *id)
        .map(|&(_, head)| head)
}

fn is_fade(p: &Value) -> bool {
    fade(p).is_some()
}

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

fn frame_key(at: f64) -> String {
    if at.fract() == 0.0 {
        format!("{}", at as i64)
    } else {
        at.to_string()
    }
}

pub fn cut(effects: &mut Option<Effects>, head: bool, tail: bool) {
    if let Some(e) = effects {
        e.cut(head, tail);
        if e.list.is_empty() {
            *effects = None;
        }
    }
}

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
