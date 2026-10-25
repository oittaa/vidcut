use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{Words, db};
use crate::duck::{AMOUNT_DB, HOLD_MS, LOOKAHEAD_MS, RECOVERY_MS, RISE_MS, THRESHOLD_DB};
use crate::model::{Clip, DynamicZoom, Marks, Picture, Project, SubtitleStyle, Title, TitleKind};
use crate::time::{Rational, TimeContext, TimeSpec};

type Show<'a> = &'a dyn Fn(&Value) -> String;

#[derive(Clone)]
struct Shown {
    text: String,
    value: String,
}

impl Shown {
    fn of(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            value: text.clone(),
            text,
        }
    }
}

struct Field {
    name: String,
    old: Option<Shown>,
    new: Shown,
    names: bool,
    quiet: bool,
}

struct Fields {
    thing: String,
    kind: &'static str,
    list: Vec<Field>,
}

impl Fields {
    fn new(thing: impl Into<String>, kind: &'static str) -> Self {
        Self {
            thing: thing.into(),
            kind,
            list: Vec::new(),
        }
    }

    fn shown(&mut self, name: impl Into<String>, olds: Vec<Shown>, new: Shown) {
        let old = match olds.split_first() {
            Some((first, rest)) if rest.iter().all(|o| o.value == first.value) => {
                Some(first.clone())
            }
            _ => None,
        };
        self.list.push(Field {
            name: name.into(),
            old,
            new,
            names: false,
            quiet: false,
        });
    }

    fn put(
        &mut self,
        name: impl Into<String>,
        olds: &[Value],
        new: &Value,
        show: Show,
        usual: &Value,
    ) {
        let shown = |v: &Value| match v {
            v if cleared(v) => Shown {
                text: "usual".into(),
                value: if usual.is_null() {
                    "usual".into()
                } else {
                    show(usual)
                },
            },
            v => Shown::of(show(v)),
        };
        self.shown(name, olds.iter().map(shown).collect(), shown(new));
    }

    fn naming(&mut self) {
        if let Some(f) = self.list.last_mut() {
            f.names = true;
        }
    }

    fn quiet(&mut self) {
        if let Some(f) = self.list.last_mut() {
            f.quiet = true;
        }
    }

    fn text(&self) -> String {
        let changed: Vec<&Field> = self
            .list
            .iter()
            .filter(|f| f.old.as_ref().is_none_or(|o| o.value != f.new.value))
            .collect();
        let moved = !changed.is_empty();
        let shown = if moved {
            changed
        } else {
            self.list.iter().collect()
        };
        let loud = shown.iter().filter(|f| !f.quiet).count();
        let parts: Vec<String> = match (shown.len(), loud) {
            (0, _) => vec!["unchanged".into()],
            (_, 0..=2) => shown
                .iter()
                .map(|f| match &f.old {
                    _ if f.quiet => f.name.clone(),
                    Some(o) if moved => format!("{} {} \u{2192} {}", f.name, o.text, f.new.text),
                    _ => format!("{} {}", f.name, f.new.text),
                })
                .collect(),
            _ => shown.iter().map(|f| f.name.clone()).collect(),
        };
        let bare = moved && loud <= 2 && shown.iter().any(|f| f.names && f.old.is_some());
        let thing = if bare { self.kind } else { &self.thing };
        format!("{thing}: {}", parts.join(", "))
    }
}

struct Set<'a, T> {
    f: Fields,
    of: Vec<&'a T>,
    p: &'a Value,
}

impl<'a, T> Set<'a, T> {
    fn new(f: Fields, of: Vec<&'a T>, p: &'a Value) -> Self {
        Self { f, of, p }
    }

    fn olds(&self, old: impl Fn(&T) -> Value) -> Vec<Value> {
        self.of.iter().map(|x| old(x)).collect()
    }

    fn add(
        &mut self,
        name: impl Into<String>,
        new: &Value,
        show: Show,
        usual: Value,
        old: impl Fn(&T) -> Value,
    ) {
        let olds = self.olds(old);
        self.f.put(name, &olds, new, show, &usual);
    }

    fn put(&mut self, key: &str, name: &str, show: Show, old: impl Fn(&T) -> Value) -> bool {
        self.usual(key, name, show, Value::Null, old)
    }

    fn usual(
        &mut self,
        key: &str,
        name: &str,
        show: Show,
        usual: Value,
        old: impl Fn(&T) -> Value,
    ) -> bool {
        let p = self.p;
        let given = !p[key].is_null();
        if given {
            self.add(name, &p[key], show, usual, old);
        }
        given
    }

    fn told(&mut self, key: &str, name: &str, show: Show) {
        let p = self.p;
        if !p[key].is_null() {
            self.f.put(name, &[], &p[key], show, &Value::Null);
        }
    }
}

fn cleared(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn trim(n: f64) -> String {
    let s = format!("{n:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn raw(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => on(*b).into(),
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), trim),
        other => other.to_string(),
    }
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn number(v: &Value) -> String {
    v.as_f64().map_or_else(|| raw(v), trim)
}

fn percent(v: &Value) -> String {
    v.as_f64()
        .map_or_else(|| raw(v), |n| format!("{}%", trim(n * 100.0)))
}

fn percents(v: &Value) -> String {
    match v.as_array() {
        Some(a) => a.iter().map(percent).collect::<Vec<_>>().join(", "),
        None => percent(v),
    }
}

fn degrees(v: &Value) -> String {
    v.as_f64()
        .map_or_else(|| raw(v), |n| format!("{}\u{b0}", trim(n)))
}

fn decibels(v: &Value) -> String {
    db(v)
}

fn ms(v: &Value) -> String {
    v.as_f64()
        .map_or_else(|| raw(v), |n| format!("{} ms", trim(n)))
}

fn level(v: &Value) -> String {
    v.as_f64()
        .map_or_else(|| raw(v), |n| format!("{} dB", trim(n)))
}

fn word(v: &Value) -> String {
    v.as_str().map_or_else(|| raw(v), title_case)
}

fn title_case(s: &str) -> String {
    s.split(['_', ' ', '.'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn text(v: &Value) -> String {
    let Some(s) = v.as_str() else {
        return raw(v);
    };
    let plain = crate::subtitles::plain(s).replace('\n', " ");
    let short: String = plain.chars().take(24).collect();
    if short.len() < plain.len() {
        format!("\u{201c}{}\u{2026}\u{201d}", short.trim_end())
    } else {
        format!("\u{201c}{short}\u{201d}")
    }
}

fn ease(v: &Value) -> String {
    match v.as_str() {
        Some("ease_in_out") => "Ease In and Out".into(),
        _ => word(v),
    }
}

fn upscaler(v: &Value) -> String {
    match v.as_str() {
        Some("ai_fast") => "AI Fast".into(),
        Some("ai_best") => "AI Best".into(),
        _ => word(v),
    }
}

fn color_space(v: &Value) -> String {
    match v.as_str() {
        Some("bt709") => "BT.709".into(),
        Some("bt601") => "BT.601".into(),
        Some("pq") => "BT.2100 PQ".into(),
        Some("hlg") => "BT.2100 HLG".into(),
        _ => word(v),
    }
}

fn rate(v: &Value) -> String {
    v.as_str()
        .and_then(|s| s.parse::<Rational>().ok())
        .map_or_else(|| raw(v), Rational::decimal)
}

fn seconds(v: &Value) -> String {
    v.as_f64()
        .map_or_else(|| raw(v), |n| format!("{}s", trim(n)))
}

const STYLE: [(&str, &str); 20] = [
    ("font", "Font"),
    ("size", "Size"),
    ("color", "Color"),
    ("bold", "Bold"),
    ("case", "Case"),
    ("outline.color", "Outline"),
    ("outline.size", "Outline Size"),
    ("background.color", "Background"),
    ("background.opacity", "Opacity"),
    ("shadow", "Shadow"),
    ("shadow.color", "Shadow Color"),
    ("shadow.opacity", "Shadow Opacity"),
    ("shadow.offset", "Shadow Offset"),
    ("shadow.blur", "Shadow Blur"),
    ("margin", "Margin"),
    ("words", "Words"),
    ("words.show", "Show Words"),
    ("words.color", "Spoken Word"),
    ("words.pop", "Pop"),
    ("speaker_color", "Speaker Color"),
];

fn style_show(key: &str) -> Show<'static> {
    match key {
        "size" | "outline.size" | "background.opacity" | "shadow.opacity" | "margin"
        | "words.pop" => &percent,
        "shadow.offset" => &percents,
        "shadow.blur" => &number,
        "case" | "words.show" | "speaker_color" => &word,
        _ => &raw,
    }
}

fn leaves(style: &Value) -> BTreeMap<String, Value> {
    fn walk(v: &Value, path: &str, out: &mut BTreeMap<String, Value>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    let at = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(x, &at, out);
                }
            }
            Value::Null => {}
            other => {
                out.insert(path.to_string(), other.clone());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(style, "", &mut out);
    for key in PRESENT {
        if style[key].is_object() {
            out.insert(key.to_string(), json!(true));
        }
    }
    out
}

const PRESENT: [&str; 2] = ["shadow", "words"];

fn style(f: &mut Fields, before: &[Value], after: &Value, size: f64) {
    let new = leaves(after);
    let old: Vec<BTreeMap<String, Value>> = before.iter().map(leaves).collect();
    let rank = |k: &str| STYLE.iter().position(|(key, _)| *key == k);
    let mut keys: Vec<&String> = new
        .keys()
        .chain(old.iter().flat_map(|m| m.keys()))
        .collect();
    keys.sort_by(|a, b| (rank(a).is_none(), rank(a), a).cmp(&(rank(b).is_none(), rank(b), b)));
    keys.dedup();
    for key in keys {
        let name = rank(key).map_or_else(|| title_case(key), |i| STYLE[i].1.to_string());
        let usual = if key == "size" {
            json!(size)
        } else {
            Value::Null
        };
        let absent = if PRESENT.contains(&key.as_str()) {
            json!(false)
        } else {
            Value::Null
        };
        let olds: Vec<Value> = old
            .iter()
            .map(|m| m.get(key).cloned().unwrap_or_else(|| absent.clone()))
            .collect();
        let now = new.get(key).cloned().unwrap_or(absent);
        f.put(name, &olds, &now, style_show(key), &usual);
    }
}

impl Words<'_> {
    fn tc(&self) -> impl Fn(&Value) -> String + '_ {
        move |v| match (v.as_i64(), self.project) {
            (Some(n), Some(p)) => p.settings.timecode(n),
            _ => raw(v),
        }
    }

    fn frame_at(&self, key: &str) -> Value {
        match &self.params[key] {
            Value::Null => Value::Null,
            v => self.frame(v).map_or_else(|| v.clone(), |n| json!(n)),
        }
    }

    fn clip_list(&self, key: &str) -> Vec<&Clip> {
        let Some(p) = self.project else {
            return Vec::new();
        };
        self.ids(key).iter().filter_map(|id| p.clip(id)).collect()
    }

    pub(super) fn title_set(&self) -> String {
        let clips = self.clip_list("clip");
        let unnamed = clips
            .iter()
            .all(|c| c.name.as_deref().is_none_or(str::is_empty));
        let titles = clips.iter().filter_map(|c| c.title.as_ref()).collect();
        let fields = Fields::new(format!("Title {}", self.clips("clip")), "Title");
        let mut s = Set::new(fields, titles, self.params);
        if s.put("text", "Text", &text, |t| json!(t.text)) && unnamed {
            s.f.naming();
        }
        s.put("kind", "Kind", &word, |t| json!(t.kind));
        let first = s.of.first().copied();
        let align = first.map_or(Value::Null, |t| json!(t.align()));
        s.usual("align", "Align", &word, align, |t| json!(t.align));
        if self.params["style"].is_object() {
            let size = Title::usual_size(first.map_or(TitleKind::Text, |t| t.kind));
            let olds = s.olds(|t| json!(t.style));
            style(&mut s.f, &olds, &self.params["style"], size);
        }
        let place = first.map_or(Value::Null, |t| {
            json!(Title::usual_position(t.kind, t.align()))
        });
        s.usual("position", "Position", &percents, place, |t| {
            json!(t.position)
        });
        s.usual("zoom", "Zoom", &percent, json!(1.0), |t| json!(t.zoom));
        s.usual("rotation", "Rotation", &degrees, json!(0.0), |t| {
            json!(t.rotation)
        });
        s.usual("opacity", "Title Opacity", &percent, json!(1.0), |t| {
            json!(t.opacity)
        });
        s.told("look", "Look", &text);
        s.f.text()
    }

    pub(super) fn cue_set(&self) -> String {
        let p = self.params;
        let cue = self.project.and_then(|pr| {
            let id = crate::editor::resolve_cue(pr, p["cue"].as_str()?, self.playhead).ok()?;
            pr.cue(&id)
        });
        let fields = Fields::new(format!("Subtitle {}", self.cues("cue")), "Subtitle");
        let mut s = Set::new(fields, cue.into_iter().collect(), p);
        if s.put("text", "Text", &text, |c| json!(c.text)) {
            s.f.naming();
        }
        let tc = self.tc();
        for key in ["start", "end"] {
            if !p[key].is_null() {
                let name = title_case(key);
                let at = self.frame_at(key);
                s.add(name, &at, &tc, Value::Null, |c| {
                    json!(if key == "start" { c.start } else { c.end() })
                });
            }
        }
        s.put("place", "Place", &word, |c| json!(c.place));
        if s.put("words", "Word Times", &raw, |c| json!(c.words)) {
            s.f.quiet();
        }
        let speaker = |v: &Value| match (v.as_str(), self.project) {
            (Some(name), Some(pr)) => pr.speakers.name(name),
            _ => raw(v),
        };
        s.put("speaker", "Speaker", &speaker, |c| json!(c.speaker));
        s.put("color", "Color", &raw, |c| json!(c.color));
        s.f.text()
    }

    pub(super) fn picture_set(&self) -> String {
        let p = self.params;
        let pictures: Vec<Picture> = self
            .clip_list("clips")
            .iter()
            .map(|c| c.picture.clone().unwrap_or_default())
            .collect();
        let (w, h) = self.project.map_or((1920.0, 1080.0), |pr| {
            (f64::from(pr.settings.width), f64::from(pr.settings.height))
        });
        let mut s = Set::new(
            Fields::new(self.clips("clips"), "Clip"),
            pictures.iter().collect(),
            p,
        );
        if let Some(at) = p["position"].as_array() {
            let x = |v: f64| (v - 0.5) * w;
            let y = |v: f64| (0.5 - v) * h;
            let new_x = json!(at.first().and_then(Value::as_f64).map(x));
            let new_y = json!(at.get(1).and_then(Value::as_f64).map(y));
            s.add("Position X", &new_x, &number, json!(0.0), |q| {
                json!(q.position.map(|[a, _]| x(a)))
            });
            s.add("Position Y", &new_y, &number, json!(0.0), |q| {
                json!(q.position.map(|[_, b]| y(b)))
            });
        }
        if !p["zoom"].is_null() {
            let [zx, zy] = match &p["zoom"] {
                Value::Array(a) => [0, 1].map(|k| a.get(k).cloned().unwrap_or(Value::Null)),
                one => [one.clone(), one.clone()],
            };
            let linked = zx == zy && s.of.iter().all(|q| q.zoom.is_none_or(|[a, b]| a == b));
            let across = |q: &Picture| json!(q.zoom.map(|[a, _]| a));
            if linked {
                s.add("Zoom", &zx, &percent, json!(1.0), across);
            } else {
                s.add("Zoom X", &zx, &percent, json!(1.0), across);
                s.add("Zoom Y", &zy, &percent, json!(1.0), |q| {
                    json!(q.zoom.map(|[_, b]| b))
                });
            }
        }
        s.usual("rotation", "Rotation", &degrees, json!(0.0), |q| {
            json!(q.rotation)
        });
        s.usual("anchor", "Anchor", &percents, json!(Picture::MIDDLE), |q| {
            json!(q.anchor)
        });
        s.usual("pitch", "Pitch", &number, json!(0.0), |q| json!(q.pitch));
        s.usual("yaw", "Yaw", &number, json!(0.0), |q| json!(q.yaw));
        s.put("flip_horizontal", "Flip Horizontal", &raw, |q| {
            json!(q.flip_horizontal)
        });
        s.put("flip_vertical", "Flip Vertical", &raw, |q| {
            json!(q.flip_vertical)
        });
        s.put("crop_left", "Crop Left", &percent, |q| json!(q.crop.left));
        s.put("crop_right", "Crop Right", &percent, |q| {
            json!(q.crop.right)
        });
        s.put("crop_top", "Crop Top", &percent, |q| json!(q.crop.top));
        s.put("crop_bottom", "Crop Bottom", &percent, |q| {
            json!(q.crop.bottom)
        });
        s.usual("crop_softness", "Crop Softness", &number, json!(0.0), |q| {
            json!(q.crop_softness)
        });
        s.usual(
            "composite_mode",
            "Composite Mode",
            &word,
            json!("normal"),
            |q| json!(q.composite_mode),
        );
        s.usual("opacity", "Opacity", &percent, json!(1.0), |q| {
            json!(q.opacity)
        });
        if let Some(zoom) = p.get("dynamic_zoom") {
            let on = json!(!zoom.is_null());
            s.add("Dynamic Zoom", &on, &raw, Value::Null, |q| {
                json!(q.dynamic_zoom.is_some())
            });
            let had = s.of.iter().all(|q| q.dynamic_zoom.is_some());
            let was = |q: &Picture| q.dynamic_zoom.unwrap_or_default();
            for side in ["start", "end"].into_iter().filter(|_| had) {
                let of = |z: DynamicZoom| if side == "start" { z.start } else { z.end };
                let label = title_case(side);
                if let Some(size) = zoom[side]["size"].as_f64() {
                    let new = json!(1.0 / size);
                    s.add(format!("{label} Zoom"), &new, &percent, Value::Null, |q| {
                        json!(1.0 / of(was(q)).size)
                    });
                }
                if zoom[side]["center"].is_array() {
                    let center = &zoom[side]["center"];
                    s.add(
                        format!("{label} Position"),
                        center,
                        &percents,
                        Value::Null,
                        |q| json!(of(was(q)).center),
                    );
                }
            }
            if had && !zoom["ease"].is_null() {
                s.add("Ease", &zoom["ease"], &ease, Value::Null, |q| {
                    json!(was(q).ease)
                });
            }
        }
        s.f.text()
    }

    pub(super) fn track_set(&self) -> String {
        let p = self.params;
        let id = self.text("track");
        let old = self
            .project
            .and_then(|pr| match (pr.track(&id), pr.subtitle_track(&id)) {
                (Some(t), _) => Some(json!({
                    "locked": t.locked,
                    "muted": t.muted,
                    "solo": t.solo,
                    "volume_db": t.volume_db,
                    "voice_isolation": t.voice_isolation,
                    "video_enabled": t.video_enabled,
                    "name": t.name,
                    "duck": t.duck,
                })),
                (None, Some(t)) => Some(json!({
                    "locked": t.locked,
                    "name": t.name,
                    "shown": t.shown,
                    "language": t.language,
                    "style": t.style,
                })),
                _ => None,
            });
        let mut s = Set::new(Fields::new(id.clone(), "Track"), old.iter().collect(), p);
        s.put("locked", "Lock", &raw, |t| t["locked"].clone());
        s.put("muted", "Mute", &raw, |t| t["muted"].clone());
        s.put("solo", "Solo", &raw, |t| t["solo"].clone());
        s.put("volume_db", "Volume", &decibels, |t| t["volume_db"].clone());
        s.put("voice_isolation", "Voice Isolation", &number, |t| {
            t["voice_isolation"].clone()
        });
        s.put("video_enabled", "Video", &raw, |t| {
            t["video_enabled"].clone()
        });
        s.put("shown", "Show", &raw, |t| t["shown"].clone());
        if let Some(duck) = p.get("duck") {
            let before = s.olds(|t| t["duck"].clone());
            ducking(&mut s.f, &before, duck);
        }
        s.put("name", "Name", &text, |t| t["name"].clone());
        s.put("language", "Language", &raw, |t| t["language"].clone());
        if p["style"].is_object() {
            let size = SubtitleStyle::default().text_fraction();
            let before = s.olds(|t| t["style"].clone());
            style(&mut s.f, &before, &p["style"], size);
        }
        s.told("look", "Look", &text);
        s.f.text()
    }

    pub(super) fn clip_set(&self) -> String {
        let clips = self.clip_list("clips");
        let named =
            matches!(clips.as_slice(), [c] if c.name.as_deref().is_some_and(|n| !n.is_empty()));
        let mut s = Set::new(Fields::new(self.clips("clips"), "Clip"), clips, self.params);
        let audio = |c: &Clip, get: fn(&crate::model::ClipAudio) -> Value| {
            c.audio.as_ref().map_or(Value::Null, get)
        };
        s.put("enabled", "Enabled", &raw, |c| json!(c.enabled));
        s.put("muted", "Mute", &raw, |c| audio(c, |a| json!(a.muted)));
        let at = self.frame(&self.params["at"]);
        s.put("gain_db", "Volume", &decibels, |c| match (&c.audio, at) {
            (Some(a), Some(t)) if a.is_keyed() => json!(c.gain_at(t as f64)),
            _ => audio(c, |a| json!(a.gain_db)),
        });
        if s.put("name", "Name", &text, |c| json!(c.name)) && named {
            s.f.naming();
        }
        s.put("color", "Color", &raw, |c| json!(c.color));
        s.put("voice_isolation", "Voice Isolation", &number, |c| {
            audio(c, |a| json!(a.voice_isolation))
        });
        s.f.text()
    }

    pub(super) fn media_set(&self) -> String {
        let media = match self.project {
            Some(pr) => self
                .list("media")
                .iter()
                .filter_map(|id| pr.media(id))
                .collect(),
            None => Vec::new(),
        };
        let mut s = Set::new(
            Fields::new(self.media("media"), "Media"),
            media,
            self.params,
        );
        s.put("color", "Color", &raw, |m| json!(m.color));
        s.put("color_space", "Color Space", &color_space, |m| {
            json!(m.color_space)
        });
        s.put("super_scale", "Super Scale", &word, |m| {
            json!(m.super_scale.factor)
        });
        s.put("sharpness", "Sharpness", &word, |m| {
            json!(m.super_scale.sharpness)
        });
        s.put("upscaler", "Upscaler", &upscaler, |m| {
            json!(m.super_scale.upscaler)
        });
        s.f.text()
    }

    pub(super) fn speaker_set(&self) -> String {
        let person = self.text("person");
        let speakers: Vec<_> = self.project.map(|pr| &pr.speakers).into_iter().collect();
        let thing = speakers.first().map_or(person.clone(), |s| s.name(&person));
        let named = speakers.iter().any(|s| s.names.contains_key(&person));
        let mut s = Set::new(Fields::new(thing, "Speaker"), speakers, self.params);
        if s.put("name", "Name", &text, |x| json!(x.names.get(&person))) && named {
            s.f.naming();
        }
        s.put("color", "Color", &raw, |x| json!(x.colors.get(&person)));
        s.f.text()
    }

    pub(super) fn marker_set(&self) -> String {
        let id = self.text("marker");
        let found = self
            .project
            .and_then(|pr| pr.markers.iter().find(|m| m.id == id).map(|m| (pr, m)));
        let thing = found.map_or_else(
            || format!("Marker {id}"),
            |(pr, m)| format!("Marker at {}", pr.settings.timecode(m.at)),
        );
        let markers = found.map(|(_, m)| m).into_iter().collect();
        let mut s = Set::new(Fields::new(thing, "Marker"), markers, self.params);
        s.put("name", "Name", &text, |m| json!(m.name));
        s.put("color", "Color", &raw, |m| json!(m.color));
        s.put("note", "Note", &text, |m| json!(m.note));
        if !self.params["duration"].is_null() {
            let length = |v: &Value| match (v.as_i64(), self.project) {
                (Some(n), Some(pr)) => pr.settings.length_timecode(n),
                _ => raw(v),
            };
            let new = match (&self.params["duration"], self.project) {
                (Value::String(t), Some(pr)) => TimeSpec::parse(t)
                    .ok()
                    .and_then(|t| pr.time_context(None).duration(&t).ok())
                    .map_or_else(|| json!(t), |n| json!(n)),
                (v, _) => v.clone(),
            };
            s.add("Duration", &new, &length, Value::Null, |m| {
                json!(m.duration)
            });
        }
        s.f.text()
    }

    pub(super) fn project_set(&self) -> String {
        let p = self.params;
        let settings: Vec<_> = self.project.map(|pr| &pr.settings).into_iter().collect();
        let first = settings.first().copied();
        let mut s = Set::new(Fields::new("Project", "Project"), settings, p);
        s.put("fps", "Frame Rate", &rate, |x| json!(x.fps));
        if !p["width"].is_null() || !p["height"].is_null() {
            let side = |key: &str, old: Option<u32>| match (&p[key], old) {
                (Value::Null, Some(n)) => json!(n),
                (v, _) => v.clone(),
            };
            let size = json!([
                side("width", first.map(|x| x.width)),
                side("height", first.map(|x| x.height))
            ]);
            let show = |v: &Value| match v.as_array().map(Vec::as_slice) {
                Some([w, h]) => format!("{}\u{d7}{}", raw(w), raw(h)),
                _ => raw(v),
            };
            s.add("Size", &size, &show, Value::Null, |x| {
                json!([x.width, x.height])
            });
        }
        let hz = |v: &Value| format!("{} Hz", raw(v));
        s.put("sample_rate", "Sample Rate", &hz, |x| json!(x.sample_rate));
        s.put("channels", "Channels", &raw, |x| json!(x.channels));
        s.put("ripple_on", "Ripple", &raw, |x| json!(x.ripple_on));
        s.put("drop_frame", "Drop Frame", &raw, |x| json!(x.drop_frame));
        s.put("use_proxies", "Proxies", &raw, |x| json!(x.use_proxies));
        let colors = |v: &Value| match v.as_str() {
            Some("auto") => "Auto".to_string(),
            Some("sdr") => "SDR".to_string(),
            Some("pq") => "HDR PQ".to_string(),
            Some("hlg") => "HDR HLG".to_string(),
            _ => raw(v),
        };
        s.put("timeline_colors", "Timeline Color Space", &colors, |x| {
            json!(x.timeline_colors)
        });
        let spec = |key: &str| serde_json::from_value::<TimeSpec>(p[key].clone()).ok();
        if !p["start_tc"].is_null() {
            let new = first.zip(spec("start_tc")).and_then(|(x, t)| {
                let mut x = x.clone();
                x.set_start_tc(&t).ok().map(|_| json!(x.timecode(0)))
            });
            let new = new.unwrap_or_else(|| p["start_tc"].clone());
            s.add("Start Timecode", &new, &raw, Value::Null, |x| {
                json!(x.timecode(0))
            });
        }
        if !p["still_duration"].is_null() {
            let new = self
                .project
                .zip(spec("still_duration"))
                .and_then(|(pr, t)| {
                    let c = TimeContext {
                        start_tc: 0,
                        ..pr.time_context(None)
                    };
                    let frames = c.resolve(&t).ok()?;
                    Some(json!(pr.settings.fps.frames_to_secs(frames)))
                });
            let new = new.unwrap_or_else(|| p["still_duration"].clone());
            s.add("Still Duration", &new, &seconds, Value::Null, |x| {
                json!(x.still_secs)
            });
        }
        let list = |v: &Value| match v.as_array().map(Vec::len) {
            Some(n) => Shown {
                text: if n == 1 {
                    "1 word".into()
                } else {
                    format!("{n} words")
                },
                value: v.to_string(),
            },
            None => Shown::of("usual"),
        };
        for (language, words) in p["fillers"].as_object().into_iter().flatten() {
            let language = language.trim().to_lowercase();
            let old = s
                .olds(|x| json!(x.fillers.get(&language)))
                .iter()
                .map(list)
                .collect();
            s.f.shown(format!("Filler Words ({language})"), old, list(words));
        }
        s.f.text()
    }

    pub(super) fn marks_set(&self, thing: String, marks: Option<&Marks>) -> String {
        let mut f = Fields::new(thing, "Marks");
        for (key, name) in [("in", "In"), ("out", "Out")] {
            if self.params[key].is_null() {
                continue;
            }
            let old = marks
                .zip(self.project)
                .map(|(m, pr)| {
                    let at = if key == "in" { m.mark_in } else { m.mark_out };
                    at.map_or_else(|| Shown::of("none"), |n| Shown::of(pr.settings.timecode(n)))
                })
                .into_iter()
                .collect();
            f.shown(name, old, Shown::of(self.time(key)));
        }
        if !self.params["duration"].is_null() {
            f.shown("Duration", Vec::new(), Shown::of(self.marked_duration()));
        }
        f.text()
    }
}

fn ducking(f: &mut Fields, before: &[Value], duck: &Value) {
    let was_on = |d: &Value| d.is_object() && d["on"].as_bool() != Some(false);
    let old: Vec<Value> = before.iter().map(|d| json!(was_on(d))).collect();
    let now = match duck {
        Value::Object(_) => duck["on"]
            .as_bool()
            .unwrap_or_else(|| before.first().is_none_or(|d| d.is_null() || was_on(d))),
        _ => false,
    };
    f.put("Ducking", &old, &json!(now), &raw, &Value::Null);
    if !duck.is_object() {
        return;
    }
    if let Some(sources) = duck["sources"].as_array() {
        let names = |a: &[Value]| match a {
            [] => Shown::of("none"),
            a => Shown::of(a.iter().map(raw).collect::<Vec<_>>().join(", ")),
        };
        let old = before
            .iter()
            .map(|d| {
                d["sources"]
                    .as_array()
                    .map_or_else(|| Shown::of("usual"), |a| names(a))
            })
            .collect();
        f.shown("Sources", old, names(sources));
    }
    let fields: [(&str, &str, Show, Value); 7] = [
        ("amount_db", "Duck Amount", &level, json!(AMOUNT_DB)),
        ("lookahead_ms", "Lookahead", &ms, json!(LOOKAHEAD_MS)),
        ("rise_ms", "Rise Time", &ms, json!(RISE_MS)),
        ("hold_ms", "Hold", &ms, json!(HOLD_MS)),
        ("recovery_ms", "Recovery Time", &ms, json!(RECOVERY_MS)),
        ("detect", "Detect", &word, json!("speech")),
        ("threshold_db", "Threshold", &level, json!(THRESHOLD_DB)),
    ];
    for (key, name, show, usual) in fields {
        if !duck[key].is_null() {
            let old: Vec<Value> = before.iter().map(|d| d[key].clone()).collect();
            f.put(name, &old, &duck[key], show, &usual);
        }
    }
}

pub(super) fn source_marks<'a>(
    project: Option<&'a Project>,
    media: &[String],
) -> Option<&'a Marks> {
    let [id] = media else {
        return None;
    };
    project?.media(id).map(|m| &m.marks)
}
