use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{
    Clip, Compound, ImageSequence, Keyframe, Marks, Media, Picture, Project, Speed, Title,
    TrackKind,
};
use crate::store::{absolute_elsewhere, is_drive, windows_absolute};
use crate::time::Rational;

pub mod effects;
pub mod picture;
#[cfg(test)]
mod tests;
pub mod titles;
pub mod transitions;

const VIDCUT: &str = "vidcut";
const EDITOR: &str = "Resolve_OTIO";
pub const COMPOUNDS_KEPT: &str =
    "compound clips and nested timelines kept as blocks, playing their clips";
const EFFECTS_KEPT: &str = "clip effects kept on their clips but not shown or played";

pub fn note_kept(warnings: &mut Vec<String>, names: &[String]) {
    let head = format!("{EFFECTS_KEPT}: ");
    let at = warnings.iter().position(|w| w.starts_with(&head));
    let mut all: Vec<String> = at.map_or_else(Vec::new, |i| {
        warnings[i][head.len()..]
            .split(", ")
            .map(String::from)
            .collect()
    });
    for n in names {
        if !all.contains(n) {
            all.push(n.clone());
        }
    }
    if all.is_empty() {
        return;
    }
    let note = format!("{head}{}", all.join(", "));
    match at {
        Some(i) => warnings[i] = note,
        None => warnings.push(note),
    }
}

fn with(mut small: Value, big: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    for (key, value) in big {
        small[key] = value;
    }
    small
}

fn rt(value: f64, rate: f64) -> Value {
    json!({ "OTIO_SCHEMA": "RationalTime.1", "rate": rate, "value": value })
}

fn range(start: f64, duration: f64, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": rt(start, rate),
        "duration": rt(duration, rate),
    })
}

fn gap(frames: i64, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "Gap.1",
        "name": "",
        "source_range": range(0.0, frames as f64, rate),
        "effects": [],
        "markers": [],
        "enabled": true,
        "metadata": {},
    })
}

fn otio_color(c: &str) -> &'static str {
    match c {
        "red" | "rose" => "RED",
        "pink" | "fuchsia" => "PINK",
        "yellow" | "lemon" | "sand" | "cream" => "YELLOW",
        "green" | "mint" => "GREEN",
        "cyan" | "sky" => "CYAN",
        "purple" | "lavender" => "PURPLE",
        "cocoa" => "ORANGE",
        _ => "BLUE",
    }
}

fn our_color(c: &str) -> &'static str {
    match c.to_ascii_uppercase().as_str() {
        "RED" => "red",
        "PINK" | "MAGENTA" => "pink",
        "ORANGE" => "cocoa",
        "YELLOW" => "yellow",
        "GREEN" => "green",
        "CYAN" => "cyan",
        "PURPLE" => "purple",
        "WHITE" => "cream",
        _ => "blue",
    }
}

fn otio_clip_color(name: &str) -> Option<Value> {
    let (n, [r, g, b]) = crate::ops::CLIP_COLORS.iter().find(|(n, _)| *n == name)?;
    let mut title = n.to_string();
    title[..1].make_ascii_uppercase();
    let x = |v: &u8| f64::from(*v) / 255.0;
    Some(json!({
        "OTIO_SCHEMA": "Color.1",
        "name": title,
        "r": x(r),
        "g": x(g),
        "b": x(b),
        "a": 1.0,
    }))
}

fn clip_color(item: &Value) -> Option<String> {
    let c = &item["color"];
    let name = c["name"].as_str().unwrap_or("").to_ascii_lowercase();
    let colors = crate::ops::CLIP_COLORS;
    if let Some((n, _)) = colors.iter().find(|(n, _)| *n == name) {
        return Some(n.to_string());
    }
    let rgb = [&c["r"], &c["g"], &c["b"]].map(|v| v.as_f64().map(|v| v * 255.0));
    let [Some(r), Some(g), Some(b)] = rgb else {
        return None;
    };
    let off = |[x, y, z]: &[u8; 3]| {
        (f64::from(*x) - r).powi(2) + (f64::from(*y) - g).powi(2) + (f64::from(*z) - b).powi(2)
    };
    colors
        .iter()
        .min_by(|a, b| off(&a.1).total_cmp(&off(&b.1)))
        .map(|(n, _)| n.to_string())
}

#[derive(Debug, Clone, Copy)]
pub enum Locations<'a> {
    Absolute,
    Bundle(&'a HashMap<String, String>),
}

fn start_tc(m: &Media, fps: Rational) -> f64 {
    m.info.timecode.map_or(0.0, |tc| tc.frames_at(fps).round())
}

fn media_reference(p: &Project, m: &Media, locations: Locations, rate: f64) -> Value {
    let fps = p.settings.fps;
    let frames = p.media_frames(m) as f64;
    let bundled = match locations {
        Locations::Bundle(names) => names.get(&m.id).cloned(),
        Locations::Absolute => None,
    };
    if let Some(s) = &m.sequence {
        let (base, s) = match bundled {
            Some(at) => bundled_sequence(&at, s),
            None => {
                let mut dir = m
                    .path
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .to_string_lossy()
                    .into_owned();
                if !dir.ends_with(['/', std::path::MAIN_SEPARATOR]) {
                    dir.push(std::path::MAIN_SEPARATOR);
                }
                (dir, s.clone())
            }
        };
        let (prefix, padding, suffix) = s.parts();
        let pictures = m.info.video.as_ref().map_or(fps, |v| v.fps).as_f64();
        return json!({
            "OTIO_SCHEMA": "ImageSequenceReference.1",
            "name": m.name,
            "available_range": range(0.0, s.count as f64, pictures),
            "available_image_bounds": null,
            "target_url_base": base,
            "name_prefix": prefix,
            "name_suffix": suffix,
            "start_frame": s.first,
            "frame_step": 1,
            "rate": pictures,
            "frame_zero_padding": padding,
            "missing_frame_policy": "error",
            "metadata": {},
        });
    }
    let available = if m.is_still() {
        range(0.0, 1.0, rate)
    } else {
        range(start_tc(m, fps), frames, rate)
    };
    json!({
        "OTIO_SCHEMA": "ExternalReference.1",
        "name": m.name,
        "target_url": bundled.unwrap_or_else(|| m.path.to_string_lossy().into_owned()),
        "available_range": available,
        "available_image_bounds": null,
        "metadata": {},
    })
}

fn bundled_sequence(location: &str, s: &ImageSequence) -> (String, ImageSequence) {
    match location.rsplit_once('/') {
        Some((dir, pattern)) if !pattern.is_empty() => (
            format!("{dir}/"),
            ImageSequence {
                pattern: pattern.to_string(),
                ..s.clone()
            },
        ),
        _ => (location.to_string(), s.clone()),
    }
}

fn written(m: &Media, locations: Locations) -> Media {
    let at = match locations {
        Locations::Bundle(names) => names.get(&m.id),
        Locations::Absolute => None,
    };
    match (at, &m.sequence) {
        (Some(at), Some(s)) => {
            let (dir, s) = bundled_sequence(at, s);
            Media {
                path: PathBuf::from(dir).join(s.name(s.first)),
                sequence: Some(s),
                ..m.clone()
            }
        }
        (Some(path), None) => Media {
            path: PathBuf::from(path),
            ..m.clone()
        },
        (None, _) => m.clone(),
    }
}

fn audio_part(c: &crate::model::Clip, a: i64, b: i64) -> Option<(i64, i64)> {
    let (ra, rb) = c.audio_range()?;
    let (s, e) = (ra.max(a), rb.min(b));
    (e > s).then_some((s, e))
}

fn played<'a>(c: &'a Clip, used: &mut Vec<String>, seen: &mut HashSet<&'a str>) {
    match &c.compound {
        Some(k) => {
            for c in k.tracks.iter().flat_map(|t| &t.clips) {
                played(c, used, seen);
            }
        }
        None if !c.is_block() && seen.insert(&c.media) => used.push(c.media.clone()),
        None => {}
    }
}

pub fn played_by(k: &Compound) -> Vec<String> {
    let (mut used, mut seen) = (Vec::new(), HashSet::new());
    for c in k.tracks.iter().flat_map(|t| &t.clips) {
        played(c, &mut used, &mut seen);
    }
    used
}

pub fn exported(c: &Clip, a: i64, b: i64) -> bool {
    let within = c.start.max(a) < c.end().min(b);
    (within && (c.video || c.compound.is_some())) || audio_part(c, a, b).is_some()
}

fn shape_of(p: &Project, c: &Clip, m: Option<&Media>) -> picture::Shape {
    let frame = (p.settings.width, p.settings.height);
    match m.and_then(|m| m.info.video.as_ref()) {
        Some(v) if c.compound.is_none() => picture::Shape::of(v, frame),
        _ => picture::Shape::whole(frame),
    }
}

fn with_picture(
    c: &Clip,
    shape: &picture::Shape,
    effects: Vec<Value>,
    g: i64,
    source: i64,
) -> Vec<Value> {
    let Some(x) = &c.picture else {
        return effects;
    };
    let timing = effects::Timing { speed: c.speed() };
    effects
        .into_iter()
        .filter(|e| !picture::is_placing(e))
        .chain(picture::effects(x, shape, g, source, &timing))
        .collect()
}

pub fn out_of_reach(p: &Project, span: Option<(i64, i64)>) -> Vec<String> {
    let (a, b) = span.unwrap_or((0, p.end()));
    p.tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| c.video && exported(c, a, b))
        .filter(|c| {
            c.picture
                .as_ref()
                .is_some_and(|x| !picture::reaches(x, &shape_of(p, c, p.media(&c.media))))
        })
        .map(|c| c.id.clone())
        .collect()
}

pub fn used_media(p: &Project, span: Option<(i64, i64)>) -> Vec<String> {
    let (a, b) = span.unwrap_or((0, p.end()));
    let mut used: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for c in p.tracks.iter().flat_map(|t| &t.clips) {
        if exported(c, a, b) {
            played(c, &mut used, &mut seen);
        }
    }
    used
}

pub fn same_media<'a>(r: &MediaRef, among: &[&'a Media]) -> Option<&'a Media> {
    let kind = || {
        among
            .iter()
            .copied()
            .filter(|m| m.sequence.is_some() == r.sequence.is_some())
    };
    if let Some(m) = kind().find(|m| r.paths.contains(&m.path)) {
        return Some(m);
    }
    let mut named = kind().filter(|m| m.path.file_name().is_some_and(|n| *n == *r.file_name));
    match (named.next(), named.next()) {
        (Some(m), None) => Some(m),
        _ => None,
    }
}

fn bundle_nested(
    v: &mut Value,
    bundled: &[&Media],
    names: &HashMap<String, String>,
    fps: Rational,
) {
    let s = schema(v);
    let found = if s.starts_with("ExternalReference") {
        v["target_url"]
            .as_str()
            .and_then(|url| locate(url, None, fps))
            .map(|r| ("target_url", r))
    } else if s.starts_with("ImageSequenceReference") {
        sequence_ref(v, None, fps)
            .ok()
            .map(|r| ("target_url_base", r))
    } else {
        None
    };
    if let Some((key, r)) = found {
        let m = same_media(&r, bundled);
        match (
            m.and_then(|m| names.get(&m.id)),
            m.and_then(|m| m.sequence.as_ref()),
        ) {
            (Some(to), Some(s)) if key == "target_url_base" => {
                let (base, s) = bundled_sequence(to, s);
                let (prefix, padding, suffix) = s.parts();
                v["target_url_base"] = json!(base);
                v["name_prefix"] = json!(prefix);
                v["name_suffix"] = json!(suffix);
                v["frame_zero_padding"] = json!(padding);
            }
            (Some(to), _) => v[key] = json!(to),
            (None, _) => {}
        }
        return;
    }
    match v {
        Value::Object(o) => o
            .values_mut()
            .for_each(|x| bundle_nested(x, bundled, names, fps)),
        Value::Array(a) => a
            .iter_mut()
            .for_each(|x| bundle_nested(x, bundled, names, fps)),
        _ => {}
    }
}

fn speed_of(effect: Option<Value>) -> Option<Speed> {
    effect.map(|effect| Speed {
        effect: effect.into(),
        skip: 0,
    })
}

fn scalar(speed: Option<&Speed>) -> f64 {
    speed.and_then(Speed::scalar).unwrap_or(1.0)
}

fn first_shown(start: i64, len: i64, speed: f64) -> (i64, i64) {
    if speed >= 0.0 {
        return (start, 0);
    }
    let bottom = start as f64 + (len - 1) as f64 * -speed;
    let first = (bottom + 1e-9).floor() as i64;
    let phase = bottom - first as f64;
    let skip = (0..1000)
        .find(|&k| (crate::render::Pace::of(speed, k).phase() - phase).abs() < 1e-6)
        .unwrap_or(0);
    (first, skip)
}

fn written_start(c: &Clip, s: i64, len: i64) -> i64 {
    let first = c.source_at(s);
    match c.speed() {
        x if x < 0.0 => {
            let top = first as f64 + c.pace().after(s - c.start).sound(0.0);
            (top + len as f64 * x).round() as i64
        }
        _ => first,
    }
}

fn media_in(p: &Project, c: &Clip, m: Option<&Media>, s: i64) -> Option<f64> {
    let x = c.speed();
    if x >= 0.0 {
        return None;
    }
    let top = c.source_at(s) as f64 + c.pace().after(s - c.start).sound(0.0);
    let frames = p.media_limit(m?) as f64;
    Some((frames - top) / -x / p.settings.fps.as_f64())
}

fn own_pace(meta: &mut Value, c: &Clip, s: i64) {
    if let Some(speed) = &c.speed {
        meta["source_in"] = json!(c.source_at(s));
        meta["skip"] = json!(speed.skip + s - c.start);
    }
}

fn our_keys(meta: &Value) -> Option<Vec<Keyframe>> {
    serde_json::from_value(meta.get("gain_keys")?.clone()).ok()
}

fn with_speed(c: &Clip, mut effects: Vec<Value>) -> Vec<Value> {
    if let Some(speed) = &c.speed {
        effects.insert(0, (*speed.effect).clone());
    }
    effects
}

pub fn export(
    p: &Project,
    name: &str,
    span: Option<(i64, i64)>,
    locations: Locations,
) -> Result<Value> {
    export_drawn(p, name, span, locations, &|_| titles::Drawn::default())
}

pub fn export_drawn(
    p: &Project,
    name: &str,
    span: Option<(i64, i64)>,
    locations: Locations,
    drawn: &dyn Fn(&Title) -> titles::Drawn,
) -> Result<Value> {
    let fps = p.settings.fps;
    let rate = fps.as_f64();
    let (a, b) = span.unwrap_or((0, p.end()));
    if b <= a {
        return Err(Error::invalid("nothing to export: the range is empty"));
    }
    let g = p.settings.start_tc + a;
    let item = |label: &str,
                c: &Clip,
                m: Option<&Media>,
                source: i64,
                frames: i64,
                enabled: bool,
                meta: Value,
                effects: Vec<Value>| {
        let (start, reference) = match m {
            Some(m) if m.is_still() => (0.0, media_reference(p, m, locations, rate)),
            Some(m) if m.sequence.is_some() => {
                (source as f64, media_reference(p, m, locations, rate))
            }
            Some(m) => (
                start_tc(m, fps) + source as f64,
                media_reference(p, m, locations, rate),
            ),
            None => (
                source as f64,
                match (&c.title, &c.generator) {
                    (Some(t), _) => titles::reference(t, p.settings.height, &drawn(t)),
                    (None, Some(g)) => (*g.reference).clone(),
                    (None, None) => Value::Null,
                },
            ),
        };
        with(
            json!({
                "OTIO_SCHEMA": "Clip.2",
                "name": label,
                "active_media_reference_key": "DEFAULT_MEDIA",
                "markers": [],
                "enabled": enabled,
            }),
            [
                ("source_range", range(start, frames as f64, rate)),
                (
                    "media_references",
                    with(json!({}), [("DEFAULT_MEDIA", reference)]),
                ),
                ("effects", Value::Array(effects)),
                ("metadata", meta),
            ],
        )
    };
    let track = |name: &str, kind: &str, children: Vec<Value>, enabled: bool, meta: Value| {
        with(
            json!({
                "OTIO_SCHEMA": "Track.1",
                "name": name,
                "kind": kind,
                "source_range": null,
                "effects": [],
                "markers": [],
                "enabled": enabled,
            }),
            [("children", Value::Array(children)), ("metadata", meta)],
        )
    };
    let bundled: Vec<&Media> = match locations {
        Locations::Bundle(names) => {
            let nested: HashSet<String> = p
                .tracks
                .iter()
                .flat_map(|t| &t.clips)
                .filter(|c| exported(c, a, b))
                .filter_map(|c| c.compound.as_ref())
                .flat_map(played_by)
                .collect();
            p.media
                .iter()
                .filter(|m| names.contains_key(&m.id) && nested.contains(&m.id))
                .collect()
        }
        Locations::Absolute => Vec::new(),
    };
    let block =
        |c: &Clip, k: &Compound, label: &str, source: i64, frames: i64, effects: Vec<Value>| {
            let mut it = compound_item(c, k, label, source, frames, effects, rate);
            if let Locations::Bundle(names) = locations {
                bundle_nested(&mut it["children"], &bundled, names, fps);
            }
            it
        };
    let media: HashMap<&str, &Media> = p.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let shown_color = |c: &Clip| {
        c.color
            .as_deref()
            .or_else(|| media.get(c.media.as_str()).and_then(|m| m.color.as_deref()))
            .and_then(otio_clip_color)
    };
    let (mut video_tracks, mut audio_tracks) = (Vec::new(), Vec::new());
    let mut groups: i64 = 0;
    let links = compound_links(p, a, b);
    let mut given = HashMap::new();
    for t in &p.tracks {
        let (mut video, mut audio) = (Vec::new(), Vec::new());
        let (mut tv, mut ta) = (a, a);
        let mut channels: Vec<u16> = Vec::new();
        let edits = transitions::Edits::of(p, t);
        for c in &t.clips {
            let m = media.get(c.media.as_str()).copied();
            if m.is_none() && !c.is_block() {
                return Err(Error::internal(format!("no media {}", c.media)));
            }
            let label = c
                .name
                .clone()
                .or_else(|| m.map(|m| m.name.clone()))
                .or_else(|| c.title.as_ref().map(|t| titles::name(t).to_string()))
                .unwrap_or_default();
            let (s, e) = (c.start.max(a), c.end().min(b));
            let shows = c.video && e > s;
            let sounds = audio_part(c, a, b).map(|(s, e)| (s.max(ta), e));
            let sounds = sounds.filter(|(s, e)| e > s);
            let group = (shows && sounds.is_some()).then(|| {
                groups += 1;
                groups
            });
            if shows {
                if s > tv {
                    video.push(gap(s - tv, rate));
                }
                if let Some(x) = transitions::head(p, &edits, c, false, s, &video, rate) {
                    video.push(x);
                }
                let mut meta = json!({ "clip": c.id, "media": c.media, "color": c.color });
                if let Some(t) = &c.title {
                    meta["title"] = json!(t);
                }
                if let Some(x) = &c.picture {
                    meta["picture"] = json!(x);
                }
                if let Some(au) = &c.audio {
                    meta["stream"] = json!(au.stream);
                    meta["gain_db"] = json!(au.gain_db);
                    if au.is_keyed() {
                        meta["gain_keys"] = json!(effects::relative(&au.gain_keys, c.source_at(s)));
                    }
                    meta["muted"] = json!(au.muted);
                    if let Some(v) = au.voice_isolation {
                        meta["voice_isolation"] = json!(v);
                    }
                }
                if m.is_some_and(Media::is_still) {
                    meta["source_in"] = json!(c.source_at(s));
                }
                own_pace(&mut meta, c, s);
                let mut metadata = with(json!({}), [(VIDCUT, meta)]);
                if let Some(g) = group {
                    metadata[EDITOR] = json!({ "Link Group ID": g });
                }
                if let Some(x) = media_in(p, c, m, s) {
                    metadata[EDITOR]["Media In"] = json!(x);
                }
                let effects = with_speed(
                    c,
                    c.effects.as_ref().map_or_else(Vec::new, |x| {
                        x.written(c.source_at(s), s > c.start, e < c.end())
                    }),
                );
                let effects = match &c.title {
                    Some(t) => titles::with_opacity(t, effects),
                    None => with_picture(c, &shape_of(p, c, m), effects, g, c.source_at(s)),
                };
                let mut it = match &c.compound {
                    Some(k) => block(c, k, &label, c.source_at(s), e - s, effects),
                    None => item(
                        &label,
                        c,
                        m,
                        written_start(c, s, e - s),
                        e - s,
                        c.enabled,
                        metadata,
                        effects,
                    ),
                };
                if let Some(x) = shown_color(c) {
                    it["color"] = x;
                }
                if let Some(k) = &c.compound {
                    tie(&mut it, c, k, &links, &mut given, &mut groups);
                }
                video.push(it);
                if let Some(x) = transitions::tail(p, &edits, c, false, e, rate) {
                    video.push(x);
                }
                tv = e;
            }
            if let (Some(au), Some((s, e))) = (&c.audio, sounds) {
                if s > ta {
                    audio.push(gap(s - ta, rate));
                }
                if let Some(x) = transitions::head(p, &edits, c, true, s, &audio, rate) {
                    audio.push(x);
                }
                let mut meta = json!({
                    "clip": c.id, "media": c.media, "color": c.color, "stream": au.stream,
                    "gain_db": au.gain_db,
                });
                if let Some(v) = au.voice_isolation {
                    meta["voice_isolation"] = json!(v);
                }
                if au.is_keyed() {
                    meta["gain_keys"] = json!(effects::relative(&au.gain_keys, c.source_at(s)));
                }
                own_pace(&mut meta, c, s);
                let n = m
                    .and_then(|m| m.info.audio.get(au.stream as usize))
                    .map_or(2, |x| x.channels);
                channels.push(n);
                let mut editor = json!({
                    "Channels": (0..n)
                        .map(|ch| json!({ "Source Channel ID": ch, "Source Track ID": au.stream }))
                        .collect::<Vec<_>>(),
                });
                if let Some(g) = group {
                    editor["Link Group ID"] = json!(g);
                }
                if let Some(x) = media_in(p, c, m, s) {
                    editor["Media In"] = json!(x);
                }
                let whole = c.audio_range().expect("has audio");
                let timing = effects::Timing { speed: c.speed() };
                let effects = with_speed(
                    c,
                    effects::sound(au, c.source_at(s), (s > whole.0, e < whole.1), &timing),
                );
                let mut it = item(
                    &label,
                    c,
                    m,
                    written_start(c, s, e - s),
                    e - s,
                    !au.muted,
                    with(json!({}), [(VIDCUT, meta), (EDITOR, editor)]),
                    effects,
                );
                if let Some(x) = shown_color(c) {
                    it["color"] = x;
                }
                audio.push(it);
                if let Some(x) = transitions::tail(p, &edits, c, true, e, rate) {
                    audio.push(x);
                }
                ta = e;
            }
            if let Some(k) = c.compound.as_ref().filter(|_| !c.video) {
                let (s, e) = (c.start.max(a).max(ta), c.end().min(b));
                if e > s {
                    if s > ta {
                        audio.push(gap(s - ta, rate));
                    }
                    let effects = with_speed(
                        c,
                        c.effects.as_ref().map_or_else(Vec::new, |x| {
                            x.written(c.source_at(s), s > c.start, e < c.end())
                        }),
                    );
                    let mut it = block(c, k, &label, c.source_at(s), e - s, effects);
                    tie(&mut it, c, k, &links, &mut given, &mut groups);
                    audio.push(it);
                    ta = e;
                }
            }
        }
        transitions::close(&mut video, rate);
        transitions::close(&mut audio, rate);
        let mut meta = json!({
            "track": t.id, "locked": t.locked, "muted": t.muted, "solo": t.solo,
            "volume_db": t.volume_db, "video_enabled": t.video_enabled, "name": t.name,
        });
        if let Some(v) = t.voice_isolation {
            meta["voice_isolation"] = json!(v);
        }
        if let Some(d) = &t.duck {
            meta["duck"] = json!(d);
        }
        let mut sound = json!({ "Locked": t.locked, "SoloOn": t.solo });
        if let Some(&n) = channels.first()
            && channels.iter().all(|&x| x == n)
            && let Some(kind) = match n {
                1 => Some("Mono"),
                2 => Some("Stereo"),
                _ => None,
            }
        {
            sound["Audio Type"] = json!(kind);
        }
        let sound = json!({ VIDCUT: meta, EDITOR: sound });
        let named = |kind: &str, n: usize| t.name.clone().unwrap_or(format!("{kind} {n}"));
        if t.kind == TrackKind::Video {
            let picture = json!({ VIDCUT: meta, EDITOR: { "Locked": t.locked } });
            let name = named("Video", video_tracks.len() + 1);
            video_tracks.push(track(&name, "Video", video, t.video_enabled, picture));
            if !audio.is_empty() {
                let name = named("Audio", audio_tracks.len() + 1);
                audio_tracks.push(track(&name, "Audio", audio, !t.muted, sound));
            }
        } else {
            let name = named("Audio", audio_tracks.len() + 1);
            audio_tracks.push(track(&name, "Audio", audio, !t.muted, sound));
        }
    }
    let markers: Vec<Value> = p
        .markers
        .iter()
        .filter(|k| span.is_none() || (a..b).contains(&k.at))
        .map(|k| {
            let length = match span {
                Some(_) => k.duration.min(b - k.at),
                None => k.duration,
            };
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": k.name,
                "comment": k.note,
                "color": otio_color(&k.color),
                "marked_range": range((k.at - a) as f64, length as f64, rate),
                "metadata": {
                    VIDCUT: { "marker": k.id, "color": k.color },
                    EDITOR: { "Note": k.note },
                },
            })
        })
        .collect();
    let marks = Marks {
        mark_in: p
            .marks
            .mark_in
            .filter(|i| (a..b).contains(i))
            .map(|i| i - a),
        mark_out: p
            .marks
            .mark_out
            .filter(|o| *o > a && *o <= b)
            .map(|o| o - a),
    };
    let mut ours = json!({ "fps": p.settings.fps, "marks": marks });
    if span.is_none() {
        ours["project"] = json!(ProjectMeta {
            settings: p.settings.clone(),
            media: p.media.iter().map(|m| written(m, locations)).collect(),
            bins: p.bins.clone(),
            tape_marks: p.tape_marks.clone(),
            next_id: p.next_id.clone(),
            subtitles: p.subtitles.clone(),
        });
    }
    let tracks = with(
        json!({
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "source_range": null,
            "effects": [],
            "enabled": true,
            "metadata": {},
        }),
        [
            (
                "children",
                Value::Array(video_tracks.into_iter().chain(audio_tracks).collect()),
            ),
            ("markers", Value::Array(markers)),
        ],
    );
    Ok(with(
        json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": name,
            "global_start_time": rt(g as f64, rate),
        }),
        [
            ("tracks", tracks),
            (
                "metadata",
                with(
                    json!({ EDITOR: { "Resolve OTIO Meta Version": "1.0" } }),
                    [(VIDCUT, ours)],
                ),
            ),
        ],
    ))
}

fn compound_of(item: &Value, fps: Rational) -> Compound {
    let frames = item["children"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            t["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| !schema(i).starts_with("Transition"))
                .map(|i| item_len(i, fps))
                .sum::<i64>()
        })
        .max()
        .unwrap_or(0);
    let mut stack = item.clone();
    if let Some(o) = stack.as_object_mut() {
        for key in ["source_range", "effects", "markers"] {
            o.remove(key);
        }
    }
    if let Some(m) = stack
        .pointer_mut(&format!("/metadata/{EDITOR}"))
        .and_then(Value::as_object_mut)
    {
        m.remove("Link Group ID");
    }
    Compound {
        stack: stack.into(),
        frames,
        tracks: Vec::new(),
    }
}

fn compound_item(
    c: &Clip,
    k: &Compound,
    label: &str,
    source: i64,
    frames: i64,
    effects: Vec<Value>,
    rate: f64,
) -> Value {
    let mut v = (*k.stack).clone();
    v["name"] = json!(label);
    v["source_range"] = range(source as f64, frames as f64, rate);
    v["effects"] = Value::Array(effects);
    v["markers"] = json!([]);
    v["enabled"] = json!(c.enabled);
    v["metadata"][VIDCUT] = json!({ "clip": c.id, "color": c.color });
    if let Some(x) = &c.picture {
        v["metadata"][VIDCUT]["picture"] = json!(x);
    }
    v
}

fn sequence_id(k: &Compound) -> Option<&str> {
    k.stack["metadata"][EDITOR]["Sequence ID"].as_str()
}

fn sequence_of(k: &Compound) -> String {
    sequence_id(k).map_or_else(
        || {
            let h = crate::fingerprint::short_digest(k.stack.to_string().as_bytes());
            format!(
                "{{{}-{}-{}-{}-{}}}",
                &h[..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..]
            )
        },
        str::to_string,
    )
}

type Links<'a> = HashMap<&'a str, (usize, String)>;

fn compound_links(p: &Project, a: i64, b: i64) -> Links<'_> {
    let blocks = |picture: bool| {
        p.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(move |c| c.video == picture && c.start.max(a) < c.end().min(b))
            .filter_map(|c| Some((c, c.compound.as_ref()?)))
    };
    let mut links = HashMap::new();
    for (n, (c, k)) in blocks(true).enumerate() {
        let same = |(s, j): &(&Clip, &Compound)| {
            !links.contains_key(s.id.as_str())
                && (s.start, s.duration, s.source_in) == (c.start, c.duration, c.source_in)
                && s.speed().to_bits() == c.speed().to_bits()
                && sequence_id(j) == sequence_id(k)
        };
        if let Some((s, _)) = blocks(false).find(same) {
            let id = sequence_of(k);
            links.insert(s.id.as_str(), (n, id.clone()));
            links.insert(c.id.as_str(), (n, id));
        }
    }
    links
}

fn tie(
    it: &mut Value,
    c: &Clip,
    k: &Compound,
    links: &Links,
    given: &mut HashMap<usize, i64>,
    groups: &mut i64,
) {
    let editor = &mut it["metadata"][EDITOR];
    match links.get(c.id.as_str()) {
        Some((n, id)) => {
            let g = *given.entry(*n).or_insert_with(|| {
                *groups += 1;
                *groups
            });
            editor["Sequence ID"] = json!(id);
            editor["Link Group ID"] = json!(g);
        }
        None => editor["Sequence ID"] = json!(sequence_of(k)),
    }
}

pub fn compound_kind(k: &Compound) -> String {
    k.stack["metadata"][EDITOR]["Sequence Type"]
        .as_str()
        .filter(|n| !n.is_empty())
        .unwrap_or("compound")
        .to_string()
}

pub fn generator_kind(g: &crate::model::Generator) -> String {
    let r = &g.reference;
    [&r["generator_kind"], &r["name"]]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|n| !n.is_empty())
        .unwrap_or("generator")
        .to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct MediaRef {
    pub url: String,
    pub shown: String,
    pub paths: Vec<PathBuf>,
    pub file_name: String,
    pub sequence: Option<Rational>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedClip {
    pub key: usize,
    pub id: Option<String>,
    pub media_id: Option<String>,
    pub track: String,
    pub media: MediaRef,
    pub name: String,
    pub start: i64,
    pub duration: i64,
    pub source_in: i64,
    pub timecoded: bool,
    pub video: bool,
    pub enabled: bool,
    pub audio: Option<(i64, i64)>,
    pub stream: u32,
    pub gain_db: f64,
    pub gain_keys: Vec<Keyframe>,
    pub muted: bool,
    pub voice_isolation: Option<f64>,
    pub link: Option<i64>,
    pub effects: Vec<Value>,
    pub audio_effects: Vec<Value>,
    pub generator: Option<Value>,
    pub title: Option<Title>,
    pub title_from: Option<Value>,
    pub picture: Option<Picture>,
    pub compound: Option<Compound>,
    pub speed: Option<Speed>,
    pub color: Option<String>,
    pub own_color: Option<Option<String>>,
}

impl ImportedClip {
    pub fn end(&self) -> i64 {
        self.start + self.duration
    }

    pub fn walk(&mut self, k: i64) {
        let skip = self.speed.as_ref().map_or(0, |s| s.skip);
        let speed = self.speed.as_ref().and_then(Speed::scalar).unwrap_or(1.0);
        self.source_in += crate::render::Pace::of(speed, skip).offset(k);
        if let Some(s) = self.speed.as_mut() {
            s.skip += k;
        }
    }

    pub fn is_block(&self) -> bool {
        self.generator.is_some() || self.title.is_some() || self.compound.is_some()
    }

    pub fn has_picture(&self) -> bool {
        self.video && self.title.is_none() && self.generator.is_none()
    }

    fn block(
        key: usize,
        track: &str,
        item: &Value,
        (start, len): (i64, i64),
        fps: Rational,
    ) -> Self {
        Self {
            key,
            id: our_str(item, "clip"),
            media_id: None,
            track: track.to_string(),
            media: MediaRef::default(),
            name: item["name"].as_str().unwrap_or("").to_string(),
            start,
            duration: len,
            source_in: time(&item["source_range"]["start_time"])
                .map_or(0, |(v, r)| frames(v, r, fps)),
            timecoded: false,
            video: true,
            enabled: item["enabled"].as_bool().unwrap_or(true),
            audio: None,
            stream: 0,
            gain_db: 0.0,
            gain_keys: Vec::new(),
            muted: false,
            voice_isolation: None,
            link: None,
            effects: Vec::new(),
            audio_effects: Vec::new(),
            generator: None,
            title: None,
            title_from: None,
            picture: None,
            compound: None,
            speed: None,
            color: clip_color(item),
            own_color: kept_color(item),
        }
    }

    fn audio_range(&self) -> Option<(i64, i64)> {
        self.audio.map(|(h, t)| (self.start + h, self.end() + t))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTransition {
    pub id: Option<String>,
    pub sound: bool,
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub before: i64,
    pub after: i64,
    pub otio: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    pub id: String,
    pub locked: bool,
    pub muted: bool,
    pub solo: bool,
    pub volume_db: f64,
    pub voice_isolation: Option<f64>,
    pub duck: Option<crate::model::Duck>,
    pub video_enabled: bool,
    pub name: Option<String>,
}

impl ImportedTrack {
    fn of(otio: &Value, id: &str, audio: bool) -> Self {
        let meta = &otio["metadata"][VIDCUT];
        let editor = &otio["metadata"][EDITOR];
        let enabled = otio["enabled"].as_bool().unwrap_or(true);
        Self {
            id: id.to_string(),
            locked: meta["locked"]
                .as_bool()
                .or(editor["Locked"].as_bool())
                .unwrap_or(false),
            muted: meta["muted"].as_bool().unwrap_or(audio && !enabled),
            solo: meta["solo"]
                .as_bool()
                .or(editor["SoloOn"].as_bool().filter(|_| audio))
                .unwrap_or(false),
            volume_db: meta["volume_db"].as_f64().unwrap_or(0.0),
            voice_isolation: meta["voice_isolation"].as_f64(),
            duck: serde_json::from_value(meta["duck"].clone()).ok(),
            video_enabled: meta["video_enabled"].as_bool().unwrap_or(audio || enabled),
            name: match meta.get("name") {
                Some(name) => name.as_str().map(String::from),
                None => otio["name"]
                    .as_str()
                    .filter(|n| !default_track_name(n))
                    .map(String::from),
            },
        }
    }
}

fn default_track_name(name: &str) -> bool {
    let name = name.trim();
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.trim_start().bytes().all(|b| b.is_ascii_digit()))
    };
    name.is_empty() || ["Video", "Audio", "V", "A"].into_iter().any(numbered)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportedMarker {
    pub at: i64,
    pub duration: i64,
    pub name: String,
    pub note: String,
    pub color: String,
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectMeta {
    pub settings: crate::model::Settings,
    pub media: Vec<Media>,
    pub bins: Vec<String>,
    pub tape_marks: crate::model::TapeMarks,
    pub next_id: crate::model::NextIds,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<crate::model::SubtitleTrack>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub name: String,
    pub fps: Rational,
    pub start_tc: i64,
    pub clips: Vec<ImportedClip>,
    pub transitions: Vec<ImportedTransition>,
    pub tracks: Vec<ImportedTrack>,
    pub markers: Vec<ImportedMarker>,
    pub marks: Marks,
    pub has_audio_tracks: bool,
    pub project: Option<ProjectMeta>,
    pub warnings: Vec<String>,
}

fn schema(v: &Value) -> &str {
    v["OTIO_SCHEMA"].as_str().unwrap_or("")
}

fn time(v: &Value) -> Option<(f64, f64)> {
    let (value, rate) = (v["value"].as_f64()?, v["rate"].as_f64()?);
    (value.is_finite() && rate.is_finite() && rate > 0.0).then_some((value, rate))
}

fn rational(rate: f64) -> Result<Rational> {
    let exact = |den: i64| {
        let num = (rate * den as f64).round();
        (num >= 1.0 && (num / den as f64 - rate).abs() < 1e-6)
            .then(|| Rational::new(num as i64, den).ok())
            .flatten()
    };
    exact(1)
        .or_else(|| exact(1001))
        .or_else(|| Rational::snap_standard(rate))
        .or_else(|| exact(100))
        .or_else(|| exact(1000))
        .ok_or_else(|| Error::usage(format!("unsupported frame rate {rate}")))
}

fn frames(value: f64, rate: f64, fps: Rational) -> i64 {
    match rational(rate) {
        Ok(r) if r == fps => value.round() as i64,
        _ => (value * fps.as_f64() / rate).round() as i64,
    }
}

fn between(from: (f64, f64), to: (f64, f64), fps: Rational) -> i64 {
    if from.1 == to.1 {
        frames(to.0 - from.0, to.1, fps)
    } else {
        ((to.0 / to.1 - from.0 / from.1) * fps.as_f64()).round() as i64
    }
}

fn find_timeline(v: &Value) -> Option<&Value> {
    if schema(v).starts_with("Timeline.") {
        return Some(v);
    }
    v["children"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(find_timeline)
}

fn is_clip(item: &Value) -> bool {
    schema(item).starts_with("Clip.")
}

fn media_reference_of(item: &Value) -> &Value {
    if item.get("media_references").is_some() {
        let key = item["active_media_reference_key"]
            .as_str()
            .unwrap_or("DEFAULT_MEDIA");
        &item["media_references"][key]
    } else {
        &item["media_reference"]
    }
}

fn item_len(item: &Value, fps: Rational) -> i64 {
    if let Some((v, r)) = time(&item["source_range"]["duration"]) {
        return frames(v, r, fps);
    }
    if is_clip(item) {
        return time(&media_reference_of(item)["available_range"]["duration"])
            .map_or(0, |(v, r)| frames(v, r, fps));
    }
    let s = schema(item);
    let children = item["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| !schema(c).starts_with("Transition"))
        .map(|c| item_len(c, fps));
    if s.starts_with("Track") {
        children.sum()
    } else if s.starts_with("Stack") {
        children.max().unwrap_or(0)
    } else {
        0
    }
}

fn percent_decoded(s: &str) -> Option<String> {
    let hex = |b: u8| (b as char).to_digit(16);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn is_local_absolute(s: &str) -> bool {
    if cfg!(windows) {
        windows_absolute(s) || s.starts_with("//")
    } else {
        s.starts_with('/')
    }
}

fn native(s: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(s.replace('/', "\\"))
    } else {
        PathBuf::from(s)
    }
}

fn bracketed(name: &str) -> Option<String> {
    let open = name.rfind('[')?;
    let close = open + name[open..].find(']')?;
    let (first, last) = name[open + 1..close].split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(first) && digits(last))
        .then(|| format!("{}{first}{}", &name[..open], &name[close + 1..]))
}

fn locate(url: &str, base: Option<&Path>, fps: Rational) -> Option<MediaRef> {
    let written = url.trim();
    if written.is_empty() {
        return None;
    }
    let (first, second) = match written.strip_prefix("file:") {
        Some(rest) => {
            let path = match rest.strip_prefix("//") {
                None => rest,
                Some(r) if r.starts_with('/') => r,
                Some(r) => match r.strip_prefix("localhost") {
                    Some(r) if r.starts_with('/') => r,
                    _ if windows_absolute(r) => r,
                    _ => return None,
                },
            };
            let path = path
                .strip_prefix('/')
                .filter(|p| windows_absolute(p))
                .unwrap_or(path);
            (percent_decoded(path), Some(path.to_string()))
        }
        None if written.find("://").is_some_and(|i| i > 1) => return None,
        None => (Some(written.to_string()), percent_decoded(written)),
    };
    let mut forms: Vec<String> = Vec::new();
    for f in [first, second].into_iter().flatten() {
        if !forms.contains(&f) {
            forms.push(f);
        }
    }
    let mut shown = forms.first()?.clone();
    let mut file_name = shown.rsplit(['/', '\\']).next().unwrap_or("").to_string();
    let mut sequence = None;
    if let Some(first) = bracketed(&file_name) {
        for f in &mut forms {
            let dir = f.len() - f.rsplit(['/', '\\']).next().unwrap_or("").len();
            if let Some(name) = bracketed(&f[dir..]) {
                f.replace_range(dir.., &name);
            }
        }
        file_name = first;
        shown = forms[0].clone();
        sequence = Some(fps);
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for f in &forms {
        let local = is_local_absolute(f);
        if !local && !absolute_elsewhere(f, cfg!(windows)) {
            if let Some(b) = base {
                paths.push(b.join(native(f)));
                if !cfg!(windows) && f.contains('\\') {
                    paths.push(b.join(f.replace('\\', "/")));
                }
            }
            continue;
        }
        if local {
            paths.push(native(f));
        } else if cfg!(windows) {
            paths.push(PathBuf::from(f));
        }
        if let Some(b) = base {
            let parts: Vec<&str> = f
                .split(['/', '\\'])
                .filter(|s| !s.is_empty() && !is_drive(s))
                .collect();
            for k in (1..=parts.len().min(3)).rev() {
                let near = parts[parts.len() - k..]
                    .iter()
                    .fold(b.to_path_buf(), |p, s| p.join(s));
                paths.push(near);
            }
        }
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for p in paths {
        if !unique.contains(&p) {
            unique.push(p);
        }
    }
    Some(MediaRef {
        url: written.to_string(),
        shown,
        paths: unique,
        file_name,
        sequence,
    })
}

fn sequence_ref(r: &Value, base: Option<&Path>, fps: Rational) -> Result<MediaRef> {
    let bad = |why: &str| Error::media(format!("an image sequence {why}"));
    let url_base = r["target_url_base"]
        .as_str()
        .ok_or_else(|| bad("has no folder"))?;
    if r["frame_step"].as_i64().unwrap_or(1) != 1 {
        return Err(bad("skips numbers (frame_step)"));
    }
    let start = r["start_frame"].as_i64().unwrap_or(1);
    let padding = r["frame_zero_padding"].as_u64().unwrap_or(0) as usize;
    if padding > crate::model::FRAME_PAD_MAX {
        return Err(bad("pads the frame number too much"));
    }
    let first = format!(
        "{}{start:0padding$}{}",
        r["name_prefix"].as_str().unwrap_or(""),
        r["name_suffix"].as_str().unwrap_or("")
    );
    let mut url = url_base.to_string();
    if !url.is_empty() && !url.ends_with(['/', '\\']) {
        url.push('/');
    }
    url.push_str(&first);
    let rate = r["rate"].as_f64().and_then(|x| rational(x).ok());
    let mut m = locate(&url, base, fps).ok_or_else(|| bad(&format!("at {url_base:?}")))?;
    m.sequence = Some(rate.unwrap_or(fps));
    Ok(m)
}

fn clip_source(item: &Value, fps: Rational, base: Option<&Path>) -> Result<(MediaRef, i64, bool)> {
    let reference = media_reference_of(item);
    let name = item["name"].as_str().unwrap_or("");
    let s = schema(reference);
    let unsupported =
        |url: &str| Error::media(format!("clip {name:?}: unsupported media URL {url:?}"));
    let media = if s.starts_with("ExternalReference") {
        let url = reference["target_url"].as_str().unwrap_or("");
        locate(url, base, fps).ok_or_else(|| unsupported(url))?
    } else if s.starts_with("ImageSequenceReference") {
        sequence_ref(reference, base, fps)
            .map_err(|e| Error::media(format!("clip {name:?}: {}", e.message)))?
    } else if let Some(url) = reference["metadata"][EDITOR]["Media Url"]
        .as_str()
        .filter(|_| s.starts_with("MissingReference"))
    {
        locate(url, base, fps).ok_or_else(|| unsupported(url))?
    } else {
        return Err(Error::media(format!(
            "clip {name:?} has no media file ({})",
            match s {
                "" => "no reference",
                s => s,
            }
        )));
    };
    let available = time(&reference["available_range"]["start_time"]);
    Ok(
        match (time(&item["source_range"]["start_time"]), available) {
            (Some(start), Some(base)) => (media, between(base, start, fps), false),
            (Some((v, r)), None) => (media, frames(v, r, fps), true),
            (None, _) => (media, 0, false),
        },
    )
}

fn link_group(item: &Value) -> Option<i64> {
    item["metadata"][EDITOR]["Link Group ID"].as_i64()
}

fn editor_stream(item: &Value) -> Option<u32> {
    let channels = item["metadata"][EDITOR]["Channels"].as_array()?;
    let track = channels.first()?["Source Track ID"].as_u64()?;
    u32::try_from(track).ok()
}

struct Between<'a> {
    item: &'a Value,
    prev: Option<usize>,
    next: usize,
}

fn walk<'a>(
    track: &'a Value,
    fps: Rational,
    warnings: &mut Vec<String>,
) -> (Vec<(i64, i64, &'a Value)>, Vec<Between<'a>>) {
    let mut out = Vec::new();
    let mut between = Vec::new();
    let mut t = 0;
    for item in track["children"].as_array().into_iter().flatten() {
        let s = schema(item);
        if s.starts_with("Transition") {
            between.push(Between {
                item,
                prev: out.len().checked_sub(1),
                next: out.len(),
            });
            continue;
        }
        if s.starts_with("Track") {
            warn(warnings, "nested tracks were left out (gaps instead)");
        } else if !s.starts_with("Gap") && !s.starts_with("Stack") && !is_clip(item) {
            warn(warnings, &format!("{s} items were left out (gaps instead)"));
        }
        let len = item_len(item, fps);
        out.push((t, len, item));
        t += len;
    }
    (out, between)
}

fn transitions_of(
    between: Vec<Between>,
    keys: &[Option<usize>],
    sound: bool,
    fps: Rational,
    out: &mut Vec<ImportedTransition>,
) {
    for b in between {
        let key = |i: Option<usize>| i.and_then(|i| keys.get(i).copied().flatten());
        let (from, to) = (key(b.prev), key(Some(b.next)));
        if from.is_none() && to.is_none() {
            continue;
        }
        let offset = |k: &str| time(&b.item[k]).map_or(0, |(v, r)| frames(v, r, fps));
        let mut otio = b.item.clone();
        if let Some(metadata) = otio.get_mut("metadata").and_then(Value::as_object_mut)
            && let Some(ours) = metadata.get_mut(VIDCUT).and_then(Value::as_object_mut)
        {
            ours.remove("transition");
            if ours.is_empty() {
                metadata.remove(VIDCUT);
            }
        }
        out.push(ImportedTransition {
            id: our_str(b.item, "transition"),
            sound,
            from,
            to,
            before: offset("in_offset"),
            after: offset("out_offset"),
            otio,
        });
    }
}

fn warn(warnings: &mut Vec<String>, w: &str) {
    if !warnings.iter().any(|x| x == w) {
        warnings.push(w.to_string());
    }
}

fn overlaps(a: (i64, i64), b: (i64, i64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

fn timeline_rate(tl: &Value, tracks: &[&Value]) -> Option<f64> {
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for item in tracks
        .iter()
        .flat_map(|t| t["children"].as_array().into_iter().flatten())
    {
        if let Some((_, r)) = time(&item["source_range"]["duration"]) {
            match counts.iter_mut().find(|(x, _)| (x - r).abs() < 1e-6) {
                Some((_, n)) => *n += 1,
                None => counts.push((r, 1)),
            }
        }
    }
    let most = counts.iter().max_by_key(|(_, n)| *n).map(|(r, _)| *r);
    most.or_else(|| time(&tl["global_start_time"]).map(|(_, r)| r))
}

fn kept_color(item: &Value) -> Option<Option<String>> {
    item["metadata"][VIDCUT]["clip"]
        .is_string()
        .then(|| our_str(item, "color"))
}

pub fn as_compound(doc: &Value, name: &str) -> Result<Value> {
    let tl = find_timeline(doc).ok_or_else(|| Error::usage("no Timeline in this OTIO file"))?;
    let tracks: Vec<&Value> = tl["tracks"]["children"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let fps = file_rate(tl, &tracks)?;
    let mut stack = tl["tracks"].clone();
    stack["name"] = json!(name);
    stack["source_range"] = range(0.0, item_len(&tl["tracks"], fps) as f64, fps.as_f64());
    let track = |kind: &str| {
        json!({
            "OTIO_SCHEMA": "Track.1",
            "kind": kind,
            "children": [stack.clone()],
        })
    };
    let children: Vec<Value> = ["Video", "Audio"]
        .into_iter()
        .filter(|kind| tracks.iter().any(|t| t["kind"] == *kind))
        .map(track)
        .collect();
    Ok(json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": name,
        "global_start_time": tl["global_start_time"],
        "tracks": { "OTIO_SCHEMA": "Stack.1", "children": children },
        "metadata": { VIDCUT: { "fps": fps } },
    }))
}

fn file_rate(tl: &Value, tracks: &[&Value]) -> Result<Rational> {
    let ours = tl["metadata"][VIDCUT]["fps"]
        .as_str()
        .and_then(|s| s.parse::<Rational>().ok());
    match ours {
        Some(r) => Ok(r),
        None => rational(
            timeline_rate(tl, tracks)
                .ok_or_else(|| Error::usage("this OTIO file has no frame rate"))?,
        ),
    }
}

pub fn holds_project(doc: &Value) -> bool {
    doc["metadata"][VIDCUT]["project"].is_object()
}

fn our_str(item: &Value, key: &str) -> Option<String> {
    item["metadata"][VIDCUT][key].as_str().map(String::from)
}

fn our_picture(item: &Value) -> Option<Picture> {
    let p = serde_json::from_value(item["metadata"][VIDCUT].get("picture")?.clone()).ok()?;
    crate::ops::picture::checked(&p).ok().flatten()
}

fn marker(k: &Value, at: i64, fps: Rational) -> ImportedMarker {
    let color = match k["metadata"][VIDCUT]["color"].as_str() {
        Some(c) if crate::ops::MARKER_COLORS.contains(&c) => c.to_string(),
        _ => our_color(k["color"].as_str().unwrap_or("")).to_string(),
    };
    let note = k["comment"]
        .as_str()
        .filter(|c| !c.is_empty())
        .or(k["metadata"][EDITOR]["Note"].as_str())
        .unwrap_or("");
    let duration = time(&k["marked_range"]["duration"]).map_or(1, |(v, r)| frames(v, r, fps));
    ImportedMarker {
        at,
        duration: duration.max(1),
        name: k["name"].as_str().unwrap_or("").to_string(),
        note: note.to_string(),
        color,
        id: our_str(k, "marker"),
    }
}

fn clip_markers(item: &Value, start: i64, len: i64, fps: Rational, out: &mut Vec<ImportedMarker>) {
    let source = time(&item["source_range"]["start_time"]);
    for k in item["markers"].as_array().into_iter().flatten() {
        let Some(t) = time(&k["marked_range"]["start_time"]) else {
            continue;
        };
        let offset = match source {
            Some(s) => between(s, t, fps),
            None => frames(t.0, t.1, fps),
        };
        if (0..len).contains(&offset) {
            out.push(marker(k, start + offset, fps));
        }
    }
}

pub fn import(doc: &Value, base: Option<&Path>) -> Result<Imported> {
    let tl = find_timeline(doc).ok_or_else(|| Error::usage("no Timeline in this OTIO file"))?;
    let tracks: Vec<&Value> = tl["tracks"]["children"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let of_kind = |kind: &str| -> Vec<&Value> {
        tracks
            .iter()
            .copied()
            .filter(|t| t["kind"] == kind)
            .collect()
    };
    let (video_tracks, audio_tracks) = (of_kind("Video"), of_kind("Audio"));
    let mut warnings = Vec::new();
    let mut read_titles: Vec<String> = Vec::new();
    let fps = file_rate(tl, &tracks)?;
    let start_tc = time(&tl["global_start_time"]).map_or(0, |(v, r)| frames(v, r, fps));
    let mut markers = Vec::new();

    let mut clips: Vec<ImportedClip> = Vec::new();
    let mut found = Vec::new();
    let mut settings: Vec<ImportedTrack> = Vec::new();
    for (k, v) in video_tracks.iter().enumerate() {
        let id = format!("V{}", k + 1);
        settings.push(ImportedTrack::of(v, &id, false));
        let (items, between) = walk(v, fps, &mut warnings);
        let mut keys = vec![None; items.len()];
        for (i, (start, len, item)) in items.into_iter().enumerate() {
            let stack = schema(item).starts_with("Stack");
            if !(is_clip(item) || stack) || len <= 0 {
                continue;
            }
            let (effects, speed) = effects::read(item);
            let mut speed = speed_of(speed);
            let x = scalar(speed.as_ref());
            let reference = media_reference_of(item);
            if stack || schema(reference).starts_with("GeneratorReference") {
                clip_markers(item, start, len, fps, &mut markers);
                keys[i] = Some(clips.len());
                let mut c = ImportedClip::block(clips.len(), &id, item, (start, len), fps);
                c.effects = effects;
                c.speed = speed;
                if stack {
                    c.compound = Some(compound_of(item, fps));
                    c.picture = our_picture(item);
                } else if let Some(mut t) = titles::read(item, reference, titles::UNKNOWN_HEIGHT) {
                    let opacity = titles::take_opacity(&mut c.effects);
                    if item["metadata"][VIDCUT].get("title").is_none() {
                        t.opacity = opacity;
                    }
                    if c.name == reference["name"].as_str().unwrap_or_default()
                        || c.name == t.label()
                    {
                        c.name.clear();
                    }
                    if item["metadata"][VIDCUT].get("title").is_none() {
                        read_titles.push(item["name"].as_str().unwrap_or("title").to_string());
                        c.title_from = Some(reference.clone());
                    }
                    c.title = Some(t);
                } else {
                    c.generator = Some(reference.clone());
                }
                clips.push(c);
                continue;
            }
            match clip_source(item, fps, base) {
                Ok((media, source_in, timecoded)) => {
                    let meta = &item["metadata"][VIDCUT];
                    let (first, skip) = first_shown(source_in, len, x);
                    if let Some(s) = speed.as_mut() {
                        s.skip = meta["skip"].as_i64().unwrap_or(skip);
                    }
                    clip_markers(item, start, len, fps, &mut markers);
                    keys[i] = Some(clips.len());
                    clips.push(ImportedClip {
                        key: clips.len(),
                        id: our_str(item, "clip"),
                        media_id: our_str(item, "media"),
                        track: id.clone(),
                        media,
                        name: item["name"].as_str().unwrap_or("").to_string(),
                        start,
                        duration: len,
                        source_in: meta["source_in"].as_i64().unwrap_or(first),
                        timecoded: timecoded && meta["source_in"].is_null(),
                        video: true,
                        enabled: item["enabled"].as_bool().unwrap_or(true),
                        audio: None,
                        stream: meta["stream"].as_u64().unwrap_or(0) as u32,
                        gain_db: meta["gain_db"].as_f64().unwrap_or(0.0),
                        gain_keys: our_keys(meta).unwrap_or_default(),
                        muted: meta["muted"].as_bool().unwrap_or(false),
                        voice_isolation: meta["voice_isolation"].as_f64(),
                        link: link_group(item),
                        effects,
                        audio_effects: Vec::new(),
                        generator: None,
                        title: None,
                        title_from: None,
                        picture: our_picture(item),
                        compound: None,
                        speed,
                        color: clip_color(item),
                        own_color: kept_color(item),
                    });
                }
                Err(e) => warnings.push(format!("{}; left as a gap", e.message)),
            }
        }
        transitions_of(between, &keys, false, fps, &mut found);
    }
    let mut pictures: HashMap<String, (Vec<usize>, i64)> = HashMap::new();
    for (i, c) in clips.iter().enumerate().filter(|(_, c)| c.video) {
        let (list, longest) = pictures.entry(c.media.url.clone()).or_default();
        list.push(i);
        *longest = (*longest).max(c.duration);
    }
    for (list, _) in pictures.values_mut() {
        list.sort_by_key(|&i| clips[i].start);
    }
    let mut foreign_audio = 0;
    for a in &audio_tracks {
        let hint = a["metadata"][VIDCUT]["track"].as_str().map(String::from);
        let mut own: Option<String> = None;
        let (items, between) = walk(a, fps, &mut warnings);
        let mut keys = vec![None; items.len()];
        for (i, (start, len, item)) in items.into_iter().enumerate() {
            let stack = schema(item).starts_with("Stack");
            if !(is_clip(item) || stack) || len <= 0 {
                continue;
            }
            let (mut effects, speed) = effects::read(item);
            let mut speed = speed_of(speed);
            let x = scalar(speed.as_ref());
            if stack {
                clip_markers(item, start, len, fps, &mut markers);
                let track = match &hint {
                    Some(h) if h.starts_with('A') => h.clone(),
                    _ => own
                        .get_or_insert_with(|| {
                            foreign_audio += 1;
                            format!("A{foreign_audio}")
                        })
                        .clone(),
                };
                keys[i] = Some(clips.len());
                let mut c = ImportedClip::block(clips.len(), &track, item, (start, len), fps);
                c.video = false;
                c.effects = effects;
                c.speed = speed;
                c.compound = Some(compound_of(item, fps));
                clips.push(c);
                continue;
            }
            let (media, source_in, timecoded, timing) = match clip_source(item, fps, base) {
                Ok((media, start, timecoded)) => {
                    let meta = &item["metadata"][VIDCUT];
                    let (first, skip) = first_shown(start, len, x);
                    if let Some(s) = speed.as_mut() {
                        s.skip = meta["skip"].as_i64().unwrap_or(skip);
                    }
                    let timing = effects::Timing { speed: x };
                    (
                        media,
                        meta["source_in"].as_i64().unwrap_or(first),
                        timecoded,
                        timing,
                    )
                }
                Err(e) => {
                    warnings.push(format!("{}; left as a gap", e.message));
                    continue;
                }
            };
            clip_markers(item, start, len, fps, &mut markers);
            let span = (start, start + len);
            let meta = &item["metadata"][VIDCUT];
            let muted = item["enabled"].as_bool() == Some(false);
            let link = link_group(item);
            let gain = meta["gain_db"].as_f64().or_else(|| effects::gain(item));
            let gain_keys = our_keys(meta).unwrap_or_else(|| effects::gain_keys(item, &timing));
            effects::drop_gain_keys(&mut effects);
            let voice_isolation = meta["voice_isolation"].as_f64();
            let stream = meta["stream"]
                .as_u64()
                .map(|s| s as u32)
                .or_else(|| editor_stream(item));
            let pairs = |c: &ImportedClip| {
                c.video
                    && c.audio.is_none()
                    && c.media.url == media.url
                    && c.timecoded == timecoded
                    && (c.source_in as f64
                        - x * c.start as f64
                        - (source_in as f64 - x * start as f64))
                        .abs()
                        < 1.0
                    && overlaps((c.start, c.end()), span)
                    && match (c.link, link) {
                        (Some(x), Some(y)) => x == y,
                        _ => true,
                    }
            };
            let owner = match &hint {
                Some(h) if h.starts_with('A') => None,
                _ => pictures.get(&media.url).and_then(|(list, longest)| {
                    let end = list.partition_point(|&i| clips[i].start < span.1);
                    list[..end]
                        .iter()
                        .rev()
                        .take_while(|&&i| clips[i].start + longest > span.0)
                        .copied()
                        .filter(|&i| {
                            hint.as_ref().is_none_or(|h| &clips[i].track == h) && pairs(&clips[i])
                        })
                        .min()
                }),
            };
            if let Some(c) = owner.map(|i| &mut clips[i]) {
                c.audio = Some((start - c.start, span.1 - c.end()));
                c.muted |= muted;
                if let Some(g) = gain {
                    c.gain_db = g;
                }
                if !gain_keys.is_empty() {
                    c.gain_keys = gain_keys;
                }
                if let Some(s) = stream {
                    c.stream = s;
                }
                if voice_isolation.is_some() {
                    c.voice_isolation = voice_isolation;
                }
                c.audio_effects = effects;
                if c.speed.is_none() {
                    c.speed = speed;
                }
                keys[i] = Some(c.key);
                continue;
            }
            let track = match &hint {
                Some(h) => h.clone(),
                None => own
                    .get_or_insert_with(|| {
                        foreign_audio += 1;
                        format!("A{foreign_audio}")
                    })
                    .clone(),
            };
            keys[i] = Some(clips.len());
            clips.push(ImportedClip {
                key: clips.len(),
                id: our_str(item, "clip"),
                media_id: our_str(item, "media"),
                track,
                media,
                name: item["name"].as_str().unwrap_or("").to_string(),
                start,
                duration: len,
                source_in,
                timecoded,
                video: false,
                enabled: true,
                audio: Some((0, 0)),
                stream: stream.unwrap_or(0),
                gain_db: gain.unwrap_or(0.0),
                gain_keys,
                muted,
                voice_isolation,
                link,
                effects: Vec::new(),
                audio_effects: effects,
                generator: None,
                title: None,
                title_from: None,
                picture: None,
                compound: None,
                speed,
                color: clip_color(item),
                own_color: kept_color(item),
            });
        }
        transitions_of(between, &keys, true, fps, &mut found);
        match (&hint, own) {
            (Some(h), _) if h.starts_with('A') => settings.push(ImportedTrack::of(a, h, true)),
            (None, Some(id)) => settings.push(ImportedTrack::of(a, &id, true)),
            _ => {}
        }
    }
    clips.sort_by(|x, y| x.track.cmp(&y.track).then(x.start.cmp(&y.start)));
    let (mut dropped, mut gone) = (0, 0);
    let mut kept: Vec<ImportedClip> = Vec::new();
    let (mut first, mut longest) = (0, 0);
    for mut c in clips {
        if kept.get(first).is_some_and(|k| k.track != c.track) {
            (first, longest) = (kept.len(), 0);
        }
        let reach = |k: &ImportedClip| k.end().max(k.audio_range().map_or(k.end(), |a| a.1));
        let near = |r: (i64, i64)| {
            kept[first..]
                .iter()
                .rev()
                .take_while(move |k| k.start + longest > r.0)
        };
        let span = (c.start, c.end());
        let busy = |r: (i64, i64)| {
            near(r).any(|k| {
                (k.video && overlaps((k.start, k.end()), r))
                    || k.audio_range().is_some_and(|a| overlaps(a, r))
                    || (!k.video && overlaps((k.start, k.end()), r))
            })
        };
        if !c.video && busy(span) {
            gone += 1;
            continue;
        }
        if c.video && near(span).any(|k| k.video && overlaps((k.start, k.end()), span)) {
            gone += 1;
            continue;
        }
        if let Some(r) = c.audio_range()
            && near(r).any(|k| k.audio_range().is_some_and(|a| overlaps(a, r)))
        {
            dropped += 1;
            c.audio = None;
            c.audio_effects.clear();
        }
        longest = longest.max(reach(&c) - c.start);
        kept.push(c);
    }
    let mut clips = kept;
    clips.sort_by_key(|c| c.start);
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} clip(s) lost audio that overlapped other audio on their track"
        ));
    }
    if gone > 0 {
        warnings.push(format!(
            "{gone} clip(s) that overlapped others on their track were left out"
        ));
    }
    settings.retain(|t| t.id.starts_with('V') || clips.iter().any(|c| c.track == t.id));
    let mut kept: Vec<String> = Vec::new();
    for c in &clips {
        let placed = c.has_picture() && picture::taken(&c.effects, c.picture.is_some());
        let shown = c
            .effects
            .iter()
            .filter(|e| !(placed && picture::is_placing(e)));
        for e in shown.chain(&c.audio_effects) {
            let name = effects::name(e);
            if !kept.contains(&name) {
                kept.push(name);
            }
        }
    }
    note_kept(&mut warnings, &kept);
    let mut made: Vec<&str> = Vec::new();
    for c in clips.iter().filter(|c| c.generator.is_some()) {
        if !made.contains(&c.name.as_str()) {
            made.push(&c.name);
        }
    }
    read_titles.sort();
    read_titles.dedup();
    if !read_titles.is_empty() {
        warnings.push(format!(
            "titles read without what vidcut's titles lack (rich text, a size, zoom or \
             rotation for each line, keyframes): {}",
            read_titles.join(", ")
        ));
    }
    if !made.is_empty() {
        warnings.push(format!(
            "generators and titles kept as blocks but played as gaps: {}",
            made.join(", ")
        ));
    }
    let mut speeds: Vec<String> = Vec::new();
    for speed in clips
        .iter()
        .filter_map(|c| c.speed.as_ref())
        .filter(|s| s.scalar().is_none())
    {
        let label = speed.label();
        if !speeds.contains(&label) {
            speeds.push(label);
        }
    }
    if !speeds.is_empty() {
        warnings.push(format!(
            "time effects kept on their clips but played at normal speed: {}",
            speeds.join(", ")
        ));
    }
    let mut nested: Vec<&str> = Vec::new();
    for c in clips.iter().filter(|c| c.compound.is_some()) {
        if !nested.contains(&c.name.as_str()) {
            nested.push(&c.name);
        }
    }
    if !nested.is_empty() {
        warnings.push(format!("{COMPOUNDS_KEPT}: {}", nested.join(", ")));
    }
    let have: std::collections::HashSet<usize> = clips.iter().map(|c| c.key).collect();
    found.retain(|t| {
        [t.from, t.to]
            .into_iter()
            .flatten()
            .any(|k| have.contains(&k))
    });
    let mut names: Vec<String> = Vec::new();
    for t in found
        .iter()
        .filter(|t| !transitions::plays_in(&t.otio, t.sound))
    {
        let name = transitions::otio_name(&t.otio);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if !names.is_empty() {
        warnings.push(format!(
            "transitions kept on their edit points but played as cuts: {}",
            names.join(", ")
        ));
    }

    for list in
        std::iter::once(&tl["tracks"]["markers"]).chain(tracks.iter().map(|t| &t["markers"]))
    {
        for k in list.as_array().into_iter().flatten() {
            if let Some((v, r)) = time(&k["marked_range"]["start_time"]) {
                markers.push(marker(k, frames(v, r, fps), fps));
            }
        }
    }
    markers.retain(|k| k.at >= 0);
    markers.sort();
    markers.dedup();
    let marks: Marks =
        serde_json::from_value(tl["metadata"][VIDCUT]["marks"].clone()).unwrap_or_default();
    let project = tl["metadata"][VIDCUT]
        .get("project")
        .and_then(|v| match serde_json::from_value::<ProjectMeta>(v.clone()) {
            Ok(meta) => Some(meta),
            Err(e) => {
                warnings.push(format!(
                    "the vidcut project in the file couldn't be read ({e}): only its timeline was"
                ));
                None
            }
        })
        .map(|mut meta| {
            if let Some(base) = base {
                for m in &mut meta.media {
                    if m.path.is_relative() {
                        m.path = base.join(native(&m.path.to_string_lossy()));
                    }
                }
            }
            meta
        });
    Ok(Imported {
        name: tl["name"].as_str().unwrap_or("").to_string(),
        fps,
        start_tc,
        clips,
        transitions: found,
        tracks: settings,
        markers,
        marks,
        has_audio_tracks: !audio_tracks.is_empty(),
        project,
        warnings,
    })
}

pub fn unheard_ducking(p: &Project, span: Option<(i64, i64)>) -> Option<String> {
    let (a, b) = span.unwrap_or((0, p.end()));
    let named: Vec<String> = p
        .tracks
        .iter()
        .filter(|t| t.duck.as_ref().is_some_and(|d| d.on))
        .filter(|t| {
            t.clips
                .iter()
                .any(|c| c.audio.is_some() && exported(c, a, b))
        })
        .map(|t| t.id.clone())
        .collect();
    (!named.is_empty()).then(|| {
        format!(
            "Ducking stays in vidcut's own metadata, so other editors won't hear it: {}",
            named.join(", ")
        )
    })
}

pub fn unheard_isolation(p: &Project, span: Option<(i64, i64)>) -> Option<String> {
    let (a, b) = span.unwrap_or((0, p.end()));
    let mut named: Vec<String> = Vec::new();
    for t in &p.tracks {
        let clips: Vec<&Clip> = t
            .clips
            .iter()
            .filter(|c| {
                c.audio
                    .as_ref()
                    .is_some_and(|au| au.voice_isolation.is_some_and(|v| v > 0.0))
            })
            .filter(|c| exported(c, a, b))
            .collect();
        match t.voice_isolation {
            Some(_)
                if t.clips
                    .iter()
                    .any(|c| c.audio.is_some() && exported(c, a, b)) =>
            {
                named.push(t.id.clone())
            }
            _ => named.extend(clips.iter().map(|c| c.id.clone())),
        }
    }
    (!named.is_empty()).then(|| {
        format!(
            "Voice Isolation stays in vidcut's own metadata, so other editors won't hear it: {}",
            named.join(", ")
        )
    })
}
