use serde_json::{Value, json};

use super::{EDITOR, schema};
use crate::model::{ClipAudio, Effects, EqKind, Equalizer, Handle, Kept, Keyframe};
use crate::validate::GAIN;

const VOLUME: &str = "Fairlight Clip Volume and Fades";
const PAN: &str = "Fairlight Clip Pan";
const EQUALIZER: &str = "Fairlight Clip Equaliser";
const EQ_BAND: &str = "Fairlight Equaliser Band";

const FADES: [(&str, bool); 4] = [
    ("videoFaderIn", true),
    ("videoFaderOut", false),
    ("faderIn", true),
    ("faderOut", false),
];

pub(crate) fn rounded(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

pub(super) fn handle_value(name: &str, frames: f64, value: f64) -> Value {
    let mut v = json!({ "Variant Type": "POINTF" });
    v[name] = json!([rounded(frames), rounded(value)]);
    v
}

pub(super) fn handle_point<'a>(key: &'a Value, name: &str) -> Option<&'a Vec<Value>> {
    key.get(name)?.get(name)?.as_array()
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

fn pan_effect(au: &ClipAudio, origin: i64) -> Value {
    let value = au.pan_keys.first().map_or(au.pan, |k| k.value);
    let mut p = ranged(
        keyed("pan", json!(value), json!(0.0), "Double"),
        *crate::validate::PAN.start(),
        *crate::validate::PAN.end(),
    );
    if au.is_pan_keyed() {
        p["Key Frames"] = key_frames(&au.pan_keys, origin);
    }
    let mut g = group(PAN, 72, 1, vec![p]);
    g["Name"] = json!("Pan");
    g["Enabled"] = json!(au.pan_on);
    effect(g)
}

fn is_pan(e: &Value) -> bool {
    e["metadata"][EDITOR]["Effect Name"] == PAN
}

fn pan_param(e: &Value) -> Option<&Value> {
    params(e).find(|p| p["Parameter ID"] == "pan")
}

fn pan_node(item: &Value) -> Option<&Value> {
    item["effects"].as_array()?.iter().find(|e| is_pan(e))
}

pub(super) fn pan_on(item: &Value) -> Option<bool> {
    Some(pan_node(item)?["metadata"][EDITOR]["Enabled"] != false)
}

pub(super) fn pan(item: &Value) -> Option<f64> {
    pan_param(pan_node(item)?)?["Parameter Value"].as_f64()
}

pub(super) fn pan_keys(item: &Value) -> Vec<Keyframe> {
    read_keys(pan_node(item).and_then(pan_param), crate::validate::PAN)
}

pub(super) fn drop_pan(list: &mut Vec<Value>) {
    list.retain(|e| !is_pan(e));
}

fn eq_kind_code(kind: EqKind) -> i64 {
    match kind {
        EqKind::LoShelf => 1,
        EqKind::Bell => 2,
        EqKind::HiShelf => 3,
        EqKind::Notch => 4,
        EqKind::LoPass => 5,
        EqKind::HiPass => 6,
    }
}

fn eq_kind_of(code: i64) -> Option<EqKind> {
    match code {
        1 => Some(EqKind::LoShelf),
        2 => Some(EqKind::Bell),
        3 => Some(EqKind::HiShelf),
        4 => Some(EqKind::Notch),
        5 => Some(EqKind::LoPass),
        6 => Some(EqKind::HiPass),
        _ => None,
    }
}

fn is_equalizer(e: &Value) -> bool {
    e["metadata"][EDITOR]["Effect Name"] == EQUALIZER
}

fn is_eq_band(e: &Value) -> bool {
    e["metadata"][EDITOR]["Effect Name"] == EQ_BAND
}

fn has_keys(e: &Value) -> bool {
    params(e).any(|p| {
        p["Key Frames"]
            .as_object()
            .is_some_and(|keys| !keys.is_empty())
    })
}

fn int_param(id: &str, value: i64, default: i64, low: f64, high: f64) -> Value {
    ranged(keyed(id, json!(value), json!(default), "Int"), low, high)
}

fn equalizer_effects(eq: &Equalizer) -> Vec<Value> {
    let mut out = vec![effect(json!({
        "Display Type": 3,
        "Effect Name": EQUALIZER,
        "Enabled": eq.on,
        "Name": "Equalizer",
        "Parameters": [],
        "Type": 64,
    }))];
    let defaults = crate::equalizer::first();
    let default_q = (crate::equalizer::FIRST_Q * 1000.0).round() as i64;
    for (i, (band, usual)) in eq.bands.iter().zip(&defaults.bands).enumerate() {
        let mut ps = Vec::new();
        if band.kind != usual.kind {
            ps.push(int_param("eq type", eq_kind_code(band.kind), 0, 0.0, 0.0));
        }
        let freq = band.freq.round() as i64;
        if freq != usual.freq.round() as i64 {
            ps.push(int_param("eq frequency", freq, 0, 0.0, 19000.0));
        }
        let gain = (band.gain * 10.0).round() as i64;
        if gain != 0 {
            ps.push(int_param("eq dB gain", gain, 0, -700.0, 240.0));
        }
        let q = (band.q * 1000.0).round() as i64;
        if q != default_q {
            ps.push(int_param("eq qFactor", q, 0, 0.0, 100000.0));
        }
        ps.push(int_param("eq band index", i as i64, i as i64, -1.0, 5.0));
        out.push(effect(json!({
            "Display Type": 3,
            "Effect Name": EQ_BAND,
            "Enabled": band.on,
            "Name": "Equalizer Band",
            "Parameters": ps,
            "Type": 63,
        })));
    }
    out
}

pub(super) fn equalizer(item: &Value) -> Option<Equalizer> {
    let list = item["effects"].as_array()?;
    let main = list.iter().find(|e| is_equalizer(e))?;
    let bands: Vec<&Value> = list.iter().filter(|e| is_eq_band(e)).collect();
    if has_keys(main) || bands.iter().any(|e| has_keys(e)) {
        return None;
    }
    let mut eq = Equalizer {
        on: main["metadata"][EDITOR]["Enabled"] != false,
        ..crate::equalizer::first()
    };
    let mut seen = [false; crate::equalizer::BANDS];
    for e in bands {
        let idx = params(e)
            .find(|p| p["Parameter ID"] == "eq band index")
            .and_then(|p| p["Parameter Value"].as_i64())
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i < crate::equalizer::BANDS)?;
        if std::mem::replace(&mut seen[idx], true) {
            return None;
        }
        let band = &mut eq.bands[idx];
        band.on = e["metadata"][EDITOR]["Enabled"] != false;
        for p in params(e) {
            let v = &p["Parameter Value"];
            match p["Parameter ID"].as_str()? {
                "eq band index" => {}
                "eq type" => band.kind = eq_kind_of(v.as_i64()?)?,
                "eq frequency" => band.freq = v.as_f64()?,
                "eq dB gain" => band.gain = v.as_f64()? / 10.0,
                "eq qFactor" => band.q = v.as_f64()? / 1000.0,
                _ => return None,
            }
        }
    }
    crate::equalizer::problem(&eq).is_none().then_some(eq)
}

pub(super) fn drop_equalizer(list: &mut Vec<Value>, taken: bool) {
    if taken {
        list.retain(|e| !is_equalizer(e) && !is_eq_band(e));
    }
}

pub(super) fn gain(item: &Value) -> Option<f64> {
    let level = volume_param(item)?;
    level["Parameter Value"]
        .as_f64()
        .map(|g| (g * 100.0).round() / 100.0)
}

fn volume_param(item: &Value) -> Option<&Value> {
    let e = item["effects"]
        .as_array()?
        .iter()
        .find(|e| is_volume(e))
        .filter(|e| e["metadata"][EDITOR]["Enabled"] != false)?;
    params(e).find(|p| p["Parameter ID"] == "volume")
}

fn is_volume(e: &Value) -> bool {
    e["metadata"][EDITOR]["Effect Name"] == VOLUME
}

fn read_keys(param: Option<&Value>, range: std::ops::RangeInclusive<f64>) -> Vec<Keyframe> {
    let Some(keys) = param.and_then(|p| p["Key Frames"].as_object()) else {
        return Vec::new();
    };
    let handle = |k: &Value, name: &str, before: bool| {
        let b = handle_point(k, name)?;
        let h = Handle {
            frames: b.first()?.as_f64()?,
            value: b.get(1)?.as_f64()?,
        };
        crate::keyframes::checked_handle(h, before).ok()
    };
    let [before, after] = ["InBez", "OutBez"];
    let mut out: Vec<Keyframe> = keys
        .iter()
        .filter_map(|(t, k)| {
            Some(Keyframe {
                frame: t.parse::<f64>().ok()?.round() as i64,
                value: k["Value"].as_f64()?.clamp(*range.start(), *range.end()),
                in_handle: handle(k, before, true),
                out_handle: handle(k, after, false),
            })
        })
        .collect();
    out.sort_by_key(|k| k.frame);
    out.dedup_by_key(|k| k.frame);
    out
}

pub(super) fn gain_keys(item: &Value) -> Vec<Keyframe> {
    read_keys(volume_param(item), GAIN)
}

pub(super) fn drop_gain_keys(list: &mut Vec<Value>) {
    for e in list.iter_mut().filter(|e| is_volume(e)) {
        for p in params_of(e).into_iter().flatten() {
            if p["Parameter ID"] == "volume"
                && let Some(o) = p.as_object_mut()
            {
                o.remove("Key Frames");
            }
        }
    }
    list.retain(|e| !is_volume(e) || in_use(e));
}

pub(super) fn key_frames(keys: &[Keyframe], origin: i64) -> Value {
    let [before, after] = ["InBez", "OutBez"];
    let mut out = serde_json::Map::new();
    for k in keys {
        let at = (k.frame - origin) as f64;
        let mut v = json!({ "Value": k.value, "Variant Type": "Double" });
        if let Some(h) = k.in_handle {
            v[before] = handle_value(before, h.frames, h.value);
        }
        if let Some(h) = k.out_handle {
            v[after] = handle_value(after, h.frames, h.value);
        }
        out.insert(frame_key(at), v);
    }
    Value::Object(out)
}

pub(super) fn relative(keys: &[Keyframe], source: i64) -> Vec<Keyframe> {
    keys.iter()
        .map(|k| Keyframe {
            frame: k.frame - source,
            ..*k
        })
        .collect()
}

pub(super) fn read(item: &Value) -> (Vec<Value>, Option<Value>) {
    let (mut kept, mut speed) = (Vec::new(), None);
    let all = item["effects"].as_array();
    let eq_on = all
        .into_iter()
        .flatten()
        .any(|e| is_equalizer(e) && e["metadata"][EDITOR]["Enabled"] != false);
    for e in all.into_iter().flatten() {
        let s = schema(e);
        if s.starts_with("LinearTimeWarp")
            || s.starts_with("FreezeFrame")
            || s.starts_with("TimeEffect")
        {
            speed.get_or_insert_with(|| e.clone());
        } else if in_use(e) || (eq_on && is_eq_band(e)) {
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
    if r["Enabled"] == false || effect == EQ_BAND {
        return false;
    }
    let off = |v: &Value, default: &Value| !same_value(v, default);
    effect == "Dynamic Zoom"
        || effect == EQUALIZER
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
        let eq_on = self
            .list
            .iter()
            .any(|k| is_equalizer(k) && k["metadata"][EDITOR]["Enabled"] != false);
        self.list.retain(|k| in_use(k) || (eq_on && is_eq_band(k)));
    }

    pub fn scale_handles(&mut self, ratio: f64) {
        if ratio == 1.0 {
            return;
        }
        for p in params_mut(&mut self.list) {
            let Some(Value::Object(keys)) = p.get_mut("Key Frames") else {
                continue;
            };
            for k in keys.values_mut() {
                for name in ["InBez", "OutBez"] {
                    if let Some(b) = k
                        .get_mut(name)
                        .and_then(|h| h.get_mut(name))
                        .and_then(Value::as_array_mut)
                        && let Some(frames) = b.first().and_then(Value::as_f64)
                    {
                        b[0] = json!(frames * ratio);
                    }
                }
            }
        }
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

pub(super) fn sound(
    au: &ClipAudio,
    source: i64,
    (head, tail): (bool, bool),
    origin: i64,
) -> Vec<Value> {
    let mut list = au
        .effects
        .as_ref()
        .map_or_else(Vec::new, |e| e.written(source, head, tail));
    let db = au.gain_keys.first().map_or(au.gain_db, |k| k.value);
    let mut level = volume(db)["metadata"][EDITOR]["Parameters"][0].clone();
    if au.is_keyed() {
        level["Key Frames"] = key_frames(&au.gain_keys, origin);
    }
    let carried = list.iter_mut().find(|e| is_volume(e)).and_then(params_of);
    match carried {
        Some(ps) => match ps.iter_mut().find(|p| p["Parameter ID"] == "volume") {
            Some(p) => {
                p["Parameter Value"] = json!(db);
                if let Some(keys) = level.get("Key Frames") {
                    p["Key Frames"] = keys.clone();
                }
            }
            None => ps.push(level),
        },
        None if db != 0.0 || au.is_keyed() => {
            let mut v = volume(db);
            v["metadata"][EDITOR]["Parameters"][0] = level;
            list.insert(0, v);
        }
        None => {}
    }
    if au.pan != 0.0 || au.is_pan_keyed() || !au.pan_on {
        list.push(pan_effect(au, origin));
    }
    if let Some(eq) = &au.equalizer
        && (eq.on || *eq != crate::equalizer::usual())
    {
        drop_equalizer(&mut list, true);
        list.extend(equalizer_effects(eq));
    }
    list
}
