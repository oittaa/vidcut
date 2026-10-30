use std::collections::{HashMap, HashSet};
use std::ops::Range;

use serde_json::{Value, json};

use super::{EDITOR, VIDCUT, gap, rt, schema};
use crate::model::{
    Clip, Handle, Keyframe, MAX_BORDER, Media, Motion, MotionEase, PictureTransition, Project,
    PushPreset, SlidePreset, Track, Transition, WHITE, WipeDirection,
};
use crate::render::Curve;

pub fn name(t: &Transition) -> String {
    otio_name(&t.otio)
}

pub(super) fn otio_name(o: &Value) -> String {
    [
        &o["name"],
        &o["metadata"][EDITOR]["Effects"]["Name"],
        &o["transition_type"],
    ]
    .into_iter()
    .filter_map(Value::as_str)
    .find(|n| !n.is_empty())
    .unwrap_or("transition")
    .to_string()
}

pub fn dissolves(t: &Transition) -> bool {
    dissolves_in(&t.otio)
}

pub fn wipe_of(t: &Transition) -> Option<WipeDirection> {
    (!t.sound).then(|| wipe_in(&t.otio)).flatten()
}

pub fn dip_of(t: &Transition) -> Option<[u8; 3]> {
    (!t.sound).then(|| dip_in(&t.otio)).flatten()
}

pub fn moving_of(t: &Transition) -> Option<PictureTransition> {
    (!t.sound).then(|| moving_in(&t.otio)).flatten()
}

pub fn kind_of(t: &Transition) -> Option<PictureTransition> {
    if let Some(direction) = wipe_of(t) {
        return Some(PictureTransition::Wipe { direction });
    }
    if let Some(color) = dip_of(t) {
        return Some(PictureTransition::DipToColor { color });
    }
    if let Some(kind) = moving_of(t) {
        return Some(kind);
    }
    (!t.sound && dissolves(t)).then_some(PictureTransition::Dissolve)
}

pub fn mixes(t: &Transition) -> bool {
    dissolves(t) || wipe_of(t).is_some() || dip_of(t).is_some() || moving_of(t).is_some()
}

fn custom(name: &str, kind: i64, parameters: Vec<Value>) -> Value {
    json!({
        "OTIO_SCHEMA": "Transition.1",
        "name": name,
        "transition_type": "Custom_Transition",
        "metadata": { EDITOR: {
            "Effects": {
                "Display Type": 1,
                "Effect Name": name,
                "Enabled": true,
                "Name": name,
                "Parameters": parameters,
                "Type": kind,
            },
            "Transition Type": name,
        }}
    })
}

fn values<'a>(o: &'a Value, name: &str, kind: i64) -> Option<&'a [Value]> {
    let editor = &o["metadata"][EDITOR];
    if !editor["Transition Type"]
        .as_str()?
        .eq_ignore_ascii_case(name)
    {
        return None;
    }
    let effects = &editor["Effects"];
    Some(match effects["Type"].as_i64() {
        Some(k) if k == kind => effects["Parameters"].as_array().map_or(&[], Vec::as_slice),
        _ => &[],
    })
}

fn value<'a>(values: &'a [Value], id: &str) -> Option<&'a Value> {
    values
        .iter()
        .find(|p| p["Parameter ID"] == id)
        .map(|p| &p["Parameter Value"])
}

fn uint(id: &str, value: usize) -> Value {
    json!({
        "Default Parameter Value": 0,
        "Parameter ID": id,
        "Parameter Value": value,
        "Variant Type": "UInt",
    })
}

fn double(id: &str, value: f64, max: f64) -> Value {
    json!({
        "Default Parameter Value": 0.0,
        "Key Frames": {},
        "Parameter ID": id,
        "Parameter Value": value,
        "Variant Type": "Double",
        "maxValue": max,
        "minValue": 0.0,
    })
}

fn color(value: [u8; 3], default: [u8; 3]) -> Value {
    json!({
        "Default Parameter Value": crate::model::hex(default),
        "Parameter ID": "color",
        "Parameter Value": crate::model::hex(value),
        "Variant Type": "Color",
    })
}

const DIP: &str = "Dip To Color Dissolve";

pub fn dip_otio(c: [u8; 3]) -> Value {
    let parameters = match c {
        [0, 0, 0] => Vec::new(),
        _ => vec![color(c, [0, 0, 0])],
    };
    custom(DIP, 11, parameters)
}

fn dip_in(o: &Value) -> Option<[u8; 3]> {
    let values = values(o, DIP, 11)?;
    let color = value(values, "color").and_then(Value::as_str);
    Some(color.and_then(crate::subtitles::rgb).unwrap_or([0, 0, 0]))
}

const PUSH: &str = "Push";

const SLIDE: &str = "Slide";

const PUSHES: [PushPreset; 4] = [
    PushPreset::Left,
    PushPreset::Right,
    PushPreset::Up,
    PushPreset::Down,
];

const SLIDES: [SlidePreset; 6] = [
    SlidePreset::LeftRight,
    SlidePreset::RightLeft,
    SlidePreset::BottomUp,
    SlidePreset::TopDown,
    SlidePreset::TopLeft,
    SlidePreset::BottomRight,
];

const CURVE: &str = "transitionCustomCurvesKeyframes";

const HANDLE: f64 = 0.3;

pub fn eased(ease: MotionEase, frames: i64) -> Vec<Keyframe> {
    let reach = HANDLE * frames as f64;
    let flat = |frames: f64| Some(Handle { frames, value: 0.0 });
    let (slow_in, slow_out) = match ease {
        MotionEase::In => (true, false),
        MotionEase::Out => (false, true),
        MotionEase::InOut => (true, true),
        _ => (false, false),
    };
    vec![
        Keyframe {
            frame: 0,
            value: 0.0,
            in_handle: None,
            out_handle: if slow_in { flat(reach) } else { None },
        },
        Keyframe {
            frame: frames,
            value: 1.0,
            in_handle: if slow_out { flat(-reach) } else { None },
            out_handle: None,
        },
    ]
}

fn curve_otio(keys: &[Keyframe]) -> Value {
    let point =
        |side: &str, h: &Handle| json!({side: [h.frames, h.value], "Variant Type": "POINTF"});
    let frames: serde_json::Map<String, Value> = keys
        .iter()
        .map(|k| {
            let mut key = json!({"Value": k.value, "Variant Type": "Double"});
            if let Some(h) = &k.in_handle {
                key["InBez"] = point("InBez", h);
            }
            if let Some(h) = &k.out_handle {
                key["OutBez"] = point("OutBez", h);
            }
            (k.frame.to_string(), key)
        })
        .collect();
    json!({
        "Default Parameter Value": 0.0,
        "Key Frames": frames,
        "Parameter ID": CURVE,
        "Parameter Value": 0.0,
        "Variant Type": "Double",
        "maxValue": 1.0,
        "minValue": 0.0,
    })
}

fn curve_in(values: &[Value]) -> Vec<Keyframe> {
    let Some(frames) = values
        .iter()
        .find(|p| p["Parameter ID"] == CURVE)
        .and_then(|p| p["Key Frames"].as_object())
    else {
        return Vec::new();
    };
    let handle = |key: &Value, side: &str| {
        let point = key[side][side].as_array()?;
        Some(Handle {
            frames: point.first()?.as_f64()?,
            value: point.get(1)?.as_f64()?,
        })
    };
    let mut keys: Vec<Keyframe> = frames
        .iter()
        .filter_map(|(at, key)| {
            Some(Keyframe {
                frame: at.trim().parse::<f64>().ok()?.round() as i64,
                value: key["Value"].as_f64()?,
                in_handle: handle(key, "InBez"),
                out_handle: handle(key, "OutBez"),
            })
        })
        .collect();
    keys.sort_by_key(|k| k.frame);
    keys
}

pub fn push_otio(preset: PushPreset, motion: &Motion, frames: i64) -> Value {
    let style = uint("push style", preset as usize);
    moving_otio((PUSH, 50), style, None, motion, frames)
}

pub fn slide_otio(preset: SlidePreset, out: bool, motion: &Motion, frames: i64) -> Value {
    let style = uint("slide style", preset as usize);
    let way = out.then(|| uint("transition direction", 1));
    moving_otio((SLIDE, 49), style, way, motion, frames)
}

fn moving_otio(
    (name, type_of): (&str, i64),
    style: Value,
    way: Option<Value>,
    motion: &Motion,
    frames: i64,
) -> Value {
    let mut parameters = Vec::new();
    if style["Parameter Value"].as_u64() != Some(0) {
        parameters.push(style);
    }
    parameters.extend(way);
    if motion.border != 0.0 {
        parameters.push(double("border", motion.border, MAX_BORDER));
    }
    if motion.feather {
        parameters.push(json!({
            "Default Parameter Value": false,
            "Parameter ID": "feather",
            "Parameter Value": true,
            "Variant Type": "Bool",
        }));
    }
    if motion.color != WHITE {
        parameters.push(color(motion.color, WHITE));
    }
    if motion.motion_blur != 0.0 {
        parameters.push(double("motion blur", motion.motion_blur, 1.0));
    }
    if !motion.ease.is_none() {
        parameters.push(uint("ease", motion.ease as usize));
    }
    let keys = match motion.ease {
        MotionEase::Custom => motion.curve.clone(),
        e => eased(e, frames),
    };
    if !keys.is_empty() {
        parameters.push(curve_otio(&keys));
    }
    custom(name, type_of, parameters)
}

fn moving_in(o: &Value) -> Option<PictureTransition> {
    let index = |values: &[Value], id: &str| match value(values, id) {
        None => Some(0),
        Some(v) => usize::try_from(v.as_u64()?).ok(),
    };
    let motion = |values: &[Value]| -> Option<Motion> {
        let number = |id: &str| value(values, id).and_then(Value::as_f64).unwrap_or(0.0);
        let ease = *MotionEase::ALL.get(index(values, "ease")?)?;
        Some(Motion {
            border: number("border"),
            feather: value(values, "feather").and_then(Value::as_bool) == Some(true),
            color: value(values, "color")
                .and_then(Value::as_str)
                .and_then(crate::subtitles::rgb)
                .unwrap_or(WHITE),
            motion_blur: number("motion blur"),
            ease,
            curve: match ease {
                MotionEase::Custom => curve_in(values),
                _ => Vec::new(),
            },
        })
    };
    if let Some(values) = values(o, PUSH, 50) {
        return Some(PictureTransition::Push {
            preset: *PUSHES.get(index(values, "push style")?)?,
            motion: motion(values)?,
        });
    }
    let values = values(o, SLIDE, 49)?;
    Some(PictureTransition::Slide {
        preset: *SLIDES.get(index(values, "slide style")?)?,
        out: match index(values, "transition direction")? {
            0 => false,
            1 => true,
            _ => return None,
        },
        motion: motion(values)?,
    })
}

pub fn progress_keys(o: &Value, frames: i64) -> Option<Vec<Keyframe>> {
    let kind = moving_in(o)?;
    let motion = kind.motion()?;
    if motion.ease.is_none() {
        return Some(eased(MotionEase::None, frames));
    }
    let values = values(o, PUSH, 50).or_else(|| values(o, SLIDE, 49))?;
    Some(curve_in(values))
}

const EDGE_WIPE: &str = "Edge Wipe";

pub fn wipe_otio(direction: WipeDirection) -> Value {
    let parameters = match edge_angle(direction) {
        0 => Vec::new(),
        angle => vec![json!({
            "Default Parameter Value": 0,
            "Parameter ID": "angle",
            "Parameter Value": angle,
            "Variant Type": "Int",
            "maxValue": 360.0,
            "minValue": -360.0,
        })],
    };
    json!({
        "OTIO_SCHEMA": "Transition.1",
        "name": EDGE_WIPE,
        "transition_type": "Custom_Transition",
        "metadata": { EDITOR: {
            "Effects": {
                "Display Type": 1,
                "Effect Name": EDGE_WIPE,
                "Enabled": true,
                "Name": EDGE_WIPE,
                "Parameters": parameters,
                "Type": 13,
            },
            "Transition Type": EDGE_WIPE,
        }}
    })
}

fn edge_angle(direction: WipeDirection) -> i64 {
    match direction {
        WipeDirection::Down => 0,
        WipeDirection::Left => 90,
        WipeDirection::Up => 180,
        WipeDirection::Right => -90,
    }
}

fn wipe_in(o: &Value) -> Option<WipeDirection> {
    let editor = &o["metadata"][EDITOR];
    if editor["Transition Type"] != EDGE_WIPE {
        return None;
    }
    let angle = match editor["Effects"]["Parameters"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["Parameter ID"] == "angle"))
    {
        Some(p) => p["Parameter Value"].as_i64()?,
        None => 0,
    };
    match angle.rem_euclid(360) {
        0 => Some(WipeDirection::Down),
        90 => Some(WipeDirection::Left),
        180 => Some(WipeDirection::Up),
        270 => Some(WipeDirection::Right),
        _ => None,
    }
}

fn dissolves_in(o: &Value) -> bool {
    let effect = &o["metadata"][EDITOR]["Effects"]["Effect Name"];
    o["transition_type"] == "SMPTE_Dissolve"
        && effect.as_str().is_none_or(|n| n == "Cross Dissolve")
}

pub(super) fn plays_in(o: &Value, sound: bool) -> bool {
    if sound {
        cross_fade_in(o).is_some()
    } else {
        dissolves_in(o) || wipe_in(o).is_some() || dip_in(o).is_some() || moving_in(o).is_some()
    }
}

pub fn cross_fade(t: &Transition) -> Option<Curve> {
    cross_fade_in(&t.otio)
}

fn cross_fade_in(o: &Value) -> Option<Curve> {
    let meta = &o["metadata"][EDITOR];
    match meta["Effects"]["Type"].as_i64() {
        Some(41) => return Some(Curve::Plus3Db),
        Some(40) => return Some(Curve::ZeroDb),
        Some(26) => return Some(Curve::Minus3Db),
        _ => {}
    }
    match meta["Transition Type"].as_str() {
        Some("Cross Fade +3DB") => Some(Curve::Plus3Db),
        Some("Cross Fade 0DB") => Some(Curve::ZeroDb),
        Some("Cross Fade -3DB") => Some(Curve::Minus3Db),
        Some(_) => None,
        None => (o["transition_type"] == "SMPTE_Dissolve").then_some(Curve::ZeroDb),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Playing<'a> {
    pub transition: &'a Transition,
    pub frames: (i64, i64),
    pub before: Option<&'a Clip>,
    pub after: Option<&'a Clip>,
}

pub fn playing<'a>(
    p: &'a Project,
    track: &'a Track,
    sound: bool,
    plays: impl Fn(&Transition) -> bool,
) -> Vec<Playing<'a>> {
    let cuts = Cuts::of(p, track);
    let find = |id: &Option<String>| {
        id.as_ref()
            .and_then(|id| cuts.clips.get(id.as_str()).copied())
    };
    let mut found: Vec<Playing<'a>> = Vec::new();
    for t in track
        .transitions
        .iter()
        .filter(|t| t.sound == sound && plays(t))
    {
        let Some(cut) = cuts.place(p, t) else {
            continue;
        };
        let (a, b) = (cut - t.before, cut + t.after);
        let (before, after) = (find(&t.from), find(&t.to));
        let covers = |c: Option<&Clip>, x: i64, y: i64| {
            c.is_none_or(|c| span(c, sound).is_some_and(|(s, e)| s <= x && e >= y))
        };
        let others_clear = track.clips.iter().all(|c| {
            let side = |o: Option<&Clip>| o.is_some_and(|o| std::ptr::eq(o, c));
            side(before) || side(after) || span(c, sound).is_none_or(|(s, e)| e <= a || s >= b)
        });
        if b > a && covers(before, a, cut) && covers(after, cut, b) && others_clear {
            found.push(Playing {
                transition: t,
                frames: (a, b),
                before,
                after,
            });
        }
    }
    found.sort_by_key(|x| x.frames.0);
    let mut out: Vec<Playing<'a>> = Vec::new();
    for x in found {
        if out.last().is_none_or(|l| l.frames.1 <= x.frames.0) {
            out.push(x);
        }
    }
    out
}

pub fn cross_fades<'a>(p: &'a Project, track: &'a Track) -> Vec<Playing<'a>> {
    if !track.transitions.iter().any(|t| t.sound) {
        return Vec::new();
    }
    playing(p, track, true, |t| cross_fade(t).is_some())
}

pub fn heard_at(fades: &[Playing], c: &Clip, at: i64) -> Option<f64> {
    let (a, b) = c.audio_range()?;
    let side = |into: bool| {
        fades.iter().find(|f| {
            let s = if into { f.after } else { f.before };
            s.is_some_and(|x| std::ptr::eq(x, c))
        })
    };
    let (into, out_of) = (side(true), side(false));
    let (start, end) = (
        into.map_or(a, |f| f.frames.0),
        out_of.map_or(b, |f| f.frames.1),
    );
    if at < start || at >= end {
        return None;
    }
    let share = |f: &Playing, falling: bool| {
        let x = (at - f.frames.0) as f64 / (f.frames.1 - f.frames.0) as f64;
        let curve = cross_fade(f.transition).expect("plays");
        curve.gain(if falling { 1.0 - x } else { x })
    };
    Some(match (into, out_of) {
        (Some(f), _) if at < f.frames.1 => share(f, false),
        (_, Some(f)) if at >= f.frames.0 => share(f, true),
        _ => 1.0,
    })
}

fn span(c: &Clip, sound: bool) -> Option<(i64, i64)> {
    if sound {
        c.audio_range()
    } else {
        c.video.then(|| (c.start, c.end()))
    }
}

pub struct Cuts<'a> {
    clips: HashMap<&'a str, &'a Clip>,
    starts: HashSet<(bool, i64)>,
    ends: HashSet<(bool, i64)>,
    pool: HashMap<&'a str, &'a Media>,
}

impl<'a> Cuts<'a> {
    pub fn of(p: &'a Project, track: &'a Track) -> Self {
        let mut cuts = Self {
            clips: HashMap::new(),
            starts: HashSet::new(),
            ends: HashSet::new(),
            pool: p.pool(),
        };
        for c in &track.clips {
            cuts.clips.entry(c.id.as_str()).or_insert(c);
            for sound in [false, true] {
                if let Some((a, b)) = span(c, sound) {
                    cuts.starts.insert((sound, a));
                    cuts.ends.insert((sound, b));
                }
            }
        }
        cuts
    }

    pub fn place(&self, p: &Project, t: &Transition) -> Option<i64> {
        place(p, self, t)
    }
}

fn source_range(p: &Project, cuts: &Cuts, c: &Clip) -> Option<Range<i64>> {
    if c.is_block() {
        return p.source_range(c);
    }
    cuts.pool.get(c.media.as_str()).map(|m| p.media_range(m, c))
}

fn place(p: &Project, cuts: &Cuts, t: &Transition) -> Option<i64> {
    let clip = |id: &Option<String>| -> Option<Option<&Clip>> {
        match id {
            None => Some(None),
            Some(id) => cuts.clips.get(id.as_str()).copied().map(Some),
        }
    };
    let (from, to) = (clip(&t.from)?, clip(&t.to)?);
    let cut = match (from, to) {
        (Some(f), Some(g)) => {
            let at = span(f, t.sound)?.1;
            (span(g, t.sound)?.0 == at).then_some(at)?
        }
        (Some(f), None) => span(f, t.sound)?.1,
        (None, Some(g)) => span(g, t.sound)?.0,
        (None, None) => return None,
    };
    let meets = |end: bool| {
        let at = (t.sound, cut);
        if end {
            cuts.ends.contains(&at)
        } else {
            cuts.starts.contains(&at)
        }
    };
    if (from.is_none() && meets(true)) || (to.is_none() && meets(false)) {
        return None;
    }
    if let Some(g) = to.filter(|g| !g.is_generator())
        && t.before > 0
        && !shows_media(g, source_range(p, cuts, g)?, cut - t.before, cut)
    {
        return None;
    }
    if let Some(f) = from.filter(|f| !f.is_generator())
        && t.after > 0
        && !shows_media(f, source_range(p, cuts, f)?, cut, cut + t.after)
    {
        return None;
    }
    Some(cut)
}

fn shows_media(c: &Clip, have: Range<i64>, a: i64, b: i64) -> bool {
    [a, b - 1]
        .into_iter()
        .all(|t| have.contains(&c.source_at(t)))
}

pub fn prune(p: &mut Project) -> usize {
    let mut dropped = 0;
    for ti in 0..p.tracks.len() {
        let track = &p.tracks[ti];
        let cuts = Cuts::of(p, track);
        let fits: Vec<bool> = track
            .transitions
            .iter()
            .map(|t| cuts.place(p, t).is_some())
            .collect();
        let mut keep = fits.into_iter();
        let before = p.tracks[ti].transitions.len();
        p.tracks[ti]
            .transitions
            .retain(|_| keep.next().unwrap_or(false));
        dropped += before - p.tracks[ti].transitions.len();
    }
    dropped
}

pub fn dropped(n: usize) -> String {
    format!("{n} transition(s) no longer fit their edit points and were dropped")
}

fn written(t: &Transition, rate: f64) -> Value {
    let mut v = (*t.otio).clone();
    v["in_offset"] = rt(t.before as f64, rate);
    v["out_offset"] = rt(t.after as f64, rate);
    v["metadata"][VIDCUT]["transition"] = Value::String(t.id.clone());
    v
}

pub(super) struct Edits<'a> {
    cuts: Cuts<'a>,
    into: HashMap<(bool, &'a str), &'a Transition>,
    out_of: HashMap<(bool, &'a str), &'a Transition>,
}

impl<'a> Edits<'a> {
    pub(super) fn of(p: &'a Project, track: &'a Track) -> Self {
        let mut edits = Self {
            cuts: Cuts::of(p, track),
            into: HashMap::new(),
            out_of: HashMap::new(),
        };
        for t in &track.transitions {
            match (&t.from, &t.to) {
                (_, Some(to)) => {
                    edits.into.entry((t.sound, to.as_str())).or_insert(t);
                }
                (Some(from), None) => {
                    edits.out_of.entry((t.sound, from.as_str())).or_insert(t);
                }
                (None, None) => {}
            }
        }
        edits
    }
}

pub(super) fn head(
    p: &Project,
    edits: &Edits,
    c: &Clip,
    sound: bool,
    s: i64,
    items: &[Value],
    rate: f64,
) -> Option<Value> {
    let t = *edits.into.get(&(sound, c.id.as_str()))?;
    if edits.cuts.place(p, t)? != s {
        return None;
    }
    let follows = match (&t.from, items.last()) {
        (Some(f), Some(last)) => is_clip_of(last, f),
        (None, Some(last)) => schema(last).starts_with("Gap"),
        (None, None) => true,
        (Some(_), None) => false,
    };
    follows.then(|| written(t, rate))
}

pub(super) fn tail(
    p: &Project,
    edits: &Edits,
    c: &Clip,
    sound: bool,
    e: i64,
    rate: f64,
) -> Option<Value> {
    let t = *edits.out_of.get(&(sound, c.id.as_str()))?;
    (edits.cuts.place(p, t)? == e).then(|| written(t, rate))
}

fn is_clip_of(item: &Value, id: &str) -> bool {
    item["metadata"][VIDCUT]["clip"] == id
}

pub(super) fn close(items: &mut Vec<Value>, rate: f64) {
    if let Some(last) = items.last()
        && schema(last).starts_with("Transition")
    {
        let after = last["out_offset"]["value"].as_f64().unwrap_or(0.0);
        if after > 0.0 {
            items.push(gap(after.round() as i64, rate));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::prune;
    use crate::model::Transition;
    use crate::test_support::Fixture;

    #[test]
    fn a_dissolve_needing_media_past_a_fast_clips_end_is_dropped() {
        let mut f = Fixture::new(&[(4.6, true), (4.0, true)]);
        f.append("m1", 0, 100);
        f.ok(json!({"method": "edit.change_speed", "params": {"clips": "c1", "speed": 2.0}}));
        f.append("m2", 10, 60);
        let mut p = f.editor.project().clone();
        p.track_mut("V1").unwrap().transitions.push(Transition {
            id: "t1".into(),
            sound: false,
            from: Some("c1".into()),
            to: Some("c2".into()),
            before: 0,
            after: 12,
            otio: json!({"OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve"})
                .into(),
        });
        assert_eq!(prune(&mut p), 1);
    }
}
