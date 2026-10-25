use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontId, Galley, Key, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2, pos2,
    text::{CCursor, LayoutJob, TextFormat},
};
use serde_json::{Value, json};
use vidcut_core::lru::Lru;
use vidcut_core::model::Project;
use vidcut_core::request::ViewerMode;

use super::export::Ask;
use super::fillers::{FillerEditor, Lists, Scope};
use super::theme;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

const PAUSE: f64 = 1.0;
const FOUND: Color32 = Color32::from_rgba_premultiplied(110, 90, 20, 140);

#[derive(Debug, Clone, PartialEq)]
enum Shown {
    Media(String),
    Timeline,
}

#[derive(Debug, Clone, PartialEq)]
struct Asked {
    shown: Shown,
    rev: u64,
    transcribed: usize,
    clips: Vec<String>,
    fillers_in: Option<String>,
    preferred: Arc<Lists>,
}

#[derive(Debug, Clone, PartialEq)]
struct Heard {
    media: String,
    stream: u32,
    file: (f64, f64),
}

#[derive(Debug, Clone, PartialEq)]
struct Who {
    person: String,
    name: String,
    speaker: u32,
}

#[derive(Debug, Clone, PartialEq)]
struct Item {
    start: i64,
    end: i64,
    text: String,
    lower: String,
    joined: bool,
    silence: bool,
    filler: bool,
    heard: Option<Heard>,
    who: Option<Who>,
    differs: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct Transcript {
    key: String,
    label: String,
    main: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Pick {
    from: String,
    text: String,
    names: String,
}

#[derive(Debug, Clone, PartialEq)]
struct Difference {
    media: String,
    stream: u32,
    first: usize,
    last: usize,
    start: f64,
    end: f64,
    picks: Vec<Pick>,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Read {
    items: Vec<Item>,
    differences: Vec<Difference>,
    media: Option<String>,
    stream: u32,
    transcripts: Vec<Transcript>,
}

#[derive(Debug, Clone, PartialEq)]
struct Typing {
    media: String,
    stream: u32,
    start: f64,
    end: f64,
    text: String,
    focus: bool,
}

type Words = Result<Arc<Read>, String>;

#[derive(Debug, Clone, PartialEq)]
struct Look {
    version: u64,
    selection: Option<(usize, usize)>,
    left_out: Vec<(i64, i64)>,
    hits: Arc<[usize]>,
    width: i32,
}

#[derive(Debug)]
struct Paragraph {
    items: Range<usize>,
    galley: Arc<Galley>,
    chars: Vec<(usize, usize, usize)>,
    dotted: Vec<[Pos2; 2]>,
    playhead: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Pressed {
    Delete,
    Sentence,
    All,
    Copy,
    Cut,
    Paste,
    Clear,
    Pick(usize),
}

const JUMPS: [(&str, Jump); 4] = [
    ("First", Jump::First),
    ("Previous", Jump::Previous),
    ("Next", Jump::Next),
    ("Last", Jump::Last),
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Jump {
    First,
    Previous,
    Next,
    Last,
}

fn jump(hits: &[usize], found: Option<usize>, to: Jump) -> usize {
    let n = hits.len();
    let k = match to {
        Jump::First => 0,
        Jump::Last => n - 1,
        Jump::Next => found.map_or(0, |f| hits.partition_point(|&h| h <= f) % n),
        Jump::Previous => found.map_or(n - 1, |f| (hits.partition_point(|&h| h < f) + n - 1) % n),
    };
    hits[k]
}

enum Act {
    Seek(i64),
    Marks(i64, i64),
    Delete,
    Undelete,
    Edit(&'static str),
    Copy,
    Cut,
    Paste,
    Marker,
    RemoveSilences,
    Fillers,
    Transcribe(Vec<String>),
    Name(String, String),
    Link(Heard, u32, String),
    Assign(Heard, f64, String),
    Main(String),
    Remove(String),
    Pick(usize, usize),
    Retype,
}

const FOOTER: f32 = 64.0;

const SEARCHES: usize = 16;

#[derive(Debug, Default)]
struct Searches {
    version: u64,
    recent: Lru<String, Arc<[usize]>, SEARCHES>,
}

impl Searches {
    fn hits(&mut self, version: u64, needle: &str, items: &[Item]) -> Arc<[usize]> {
        if self.version != version {
            self.recent = Lru::new();
            self.version = version;
        }
        let needle = needle.to_string();
        if let Some(hits) = self.recent.get(&needle) {
            return hits.clone();
        }
        let keep = |i: &usize| items[*i].lower.contains(needle.as_str());
        let wider = self
            .recent
            .iter()
            .filter(|(n, _)| needle.contains(n.as_str()))
            .max_by_key(|(n, _)| n.len());
        let hits: Arc<[usize]> = match wider {
            Some((_, wider)) => wider.iter().copied().filter(keep).collect(),
            None => (0..items.len()).filter(keep).collect(),
        };
        self.recent.insert(needle, hits.clone());
        hits
    }
}

#[derive(Debug, Default)]
pub(super) struct TranscriptionUi {
    pub(super) open: bool,
    pub(super) over: bool,
    keys: Vec<Pressed>,
    asking: Option<(Asked, Ask)>,
    shown: Option<(Asked, Words)>,
    version: u64,
    look: Option<(Look, Vec<Paragraph>)>,
    selection: Option<(usize, usize)>,
    anchor: Option<usize>,
    dragging: bool,
    moved: bool,
    footer: f32,
    search: String,
    searches: Searches,
    found: Option<usize>,
    scroll_to: Option<(usize, bool)>,
    hold: bool,
    followed: Option<usize>,
    fillers_in: Option<String>,
    detected: Option<String>,
    editing: Option<FillerEditor>,
    saving: Option<(Scope, Ask)>,
    edit_error: Option<String>,
    naming: String,
    difference: Option<usize>,
    picking: Option<usize>,
    typing: Option<Typing>,
}

fn digit(key: Key) -> Option<usize> {
    [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ]
    .iter()
    .position(|k| *k == key)
}

impl TranscriptionUi {
    pub(super) fn take_keys(&mut self, ctx: &egui::Context) {
        let keys = &mut self.keys;
        let picking = self.picking.is_some();
        let timeline = self
            .shown
            .as_ref()
            .is_some_and(|(a, _)| a.shown == Shown::Timeline);
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                let pressed = match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if picking && !modifiers.any() && digit(*key).is_some() => {
                        digit(*key).map(Pressed::Pick)
                    }
                    egui::Event::Key {
                        key: Key::Delete | Key::Backspace,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if !modifiers.any() => Some(Pressed::Delete),
                    egui::Event::Key {
                        key: Key::X,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if !modifiers.any() => Some(Pressed::Sentence),
                    egui::Event::Key {
                        key: Key::A,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command_only() => Some(Pressed::All),
                    egui::Event::Key {
                        key: Key::C,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command_only() => Some(Pressed::Copy),
                    egui::Event::Copy => Some(Pressed::Copy),
                    egui::Event::Cut if timeline => Some(Pressed::Cut),
                    egui::Event::Paste(_) if timeline => Some(Pressed::Paste),
                    egui::Event::Key {
                        key: Key::Escape,
                        pressed: true,
                        modifiers,
                        ..
                    } if !modifiers.any() => Some(Pressed::Clear),
                    _ => None,
                };
                match pressed {
                    Some(k) => {
                        if !keys.contains(&k) {
                            keys.push(k);
                        }
                        false
                    }
                    None => true,
                }
            });
        });
    }

    fn selected(&self, items: &[Item]) -> Option<(usize, usize, i64, i64)> {
        let (a, b) = self.selection?;
        let (first, last) = (items.get(a)?, items.get(b)?);
        Some((a, b, first.start, last.end))
    }
}

fn word(entry: &Value, w: &Value) -> Option<Item> {
    let text = w["text"].as_str()?;
    Some(Item {
        start: w["start"].as_i64()?,
        end: w["end"].as_i64()?,
        text: text.to_string(),
        lower: text.to_lowercase(),
        joined: w["joined"].as_bool().unwrap_or(false),
        silence: false,
        filler: w["filler"].as_bool().unwrap_or(false),
        heard: Some(Heard {
            media: entry["media"].as_str().unwrap_or_default().to_string(),
            stream: entry["stream"].as_u64().unwrap_or(0) as u32,
            file: (
                w["file_time"][0].as_f64().unwrap_or(0.0),
                w["file_time"][1].as_f64().unwrap_or(0.0),
            ),
        }),
        who: w["person"].as_str().map(|person| Who {
            person: person.to_string(),
            name: w["name"].as_str().unwrap_or(person).to_string(),
            speaker: w["speaker"].as_u64().unwrap_or(0) as u32,
        }),
        differs: None,
    })
}

fn silence(start: i64, end: i64) -> Item {
    Item {
        start,
        end,
        text: "(...)".into(),
        lower: String::new(),
        joined: false,
        silence: true,
        filler: false,
        heard: None,
        who: None,
        differs: None,
    }
}

fn differences(entry: &Value, at: impl Fn(u64) -> Option<usize>) -> Vec<Difference> {
    let main = entry["transcript"].as_str().unwrap_or_default();
    let labels: HashMap<&str, &str> = entry["transcripts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| Some((t["key"].as_str()?, t["label"].as_str()?)))
        .collect();
    let pick = |o: &Value| {
        let keys: Vec<&str> = o["transcripts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let from = match keys.contains(&main) {
            true => main,
            false => keys.first().copied().unwrap_or_default(),
        };
        let names: Vec<&str> = keys
            .iter()
            .map(|k| labels.get(k).copied().unwrap_or(k))
            .collect();
        Pick {
            from: from.to_string(),
            text: o["text"].as_str().unwrap_or_default().to_string(),
            names: names.join(", "),
        }
    };
    entry["differences"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let first = at(d["words"][0].as_u64()?)?;
            let last = at(d["words"][1].as_u64()?)?;
            Some(Difference {
                media: entry["media"].as_str().unwrap_or_default().to_string(),
                stream: entry["stream"].as_u64().unwrap_or(0) as u32,
                first: first.min(last),
                last: first.max(last),
                start: d["start"].as_f64()?,
                end: d["end"].as_f64()?,
                picks: d["options"].as_array()?.iter().map(pick).collect(),
            })
        })
        .collect()
}

fn holes(p: &Project) -> Vec<(i64, i64)> {
    let mut spans: Vec<(i64, i64)> = p
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| (c.start, c.end())))
        .collect();
    spans.sort_unstable();
    let mut out = Vec::new();
    let mut reach: Option<i64> = None;
    for (a, b) in spans {
        if let Some(r) = reach
            && a > r
        {
            out.push((r, a));
        }
        reach = Some(reach.map_or(b, |r| r.max(b)));
    }
    out
}

fn parse(answer: &Value, pause: i64, bounds: Option<(i64, i64)>, holes: &[(i64, i64)]) -> Read {
    let entries: Vec<&Value> = answer.as_array().into_iter().flatten().collect();
    let mut words: Vec<(usize, u64, Item)> = entries
        .iter()
        .enumerate()
        .flat_map(|(e, entry)| {
            entry["words"]
                .as_array()
                .into_iter()
                .flatten()
                .zip(0..)
                .filter_map(move |(w, k)| Some((e, k, word(entry, w)?)))
        })
        .collect();
    words.sort_by_key(|(_, _, w)| (w.start, w.end));
    let mut items: Vec<Item> = Vec::with_capacity(words.len() * 2);
    let mut at: HashMap<(usize, u64), usize> = HashMap::with_capacity(words.len());
    let mut last = bounds.map(|(a, _)| a);
    for (e, k, w) in words {
        if let Some(end) = last {
            let hole = holes.iter().find(|&&(a, b)| a < w.start && b > end);
            match hole {
                _ if w.start - end >= pause => items.push(silence(end, w.start)),
                Some(&(a, b)) => items.push(silence(a.max(end), b.min(w.start))),
                None => {}
            }
        }
        last = Some(last.map_or(w.end, |end| end.max(w.end)));
        at.insert((e, k), items.len());
        items.push(w);
    }
    if let (Some((_, b)), Some(end)) = (bounds, last)
        && b - end >= pause
    {
        items.push(silence(end, b));
    }
    let mut found: Vec<Difference> = entries
        .iter()
        .enumerate()
        .flat_map(|(e, entry)| differences(entry, |k| at.get(&(e, k)).copied()))
        .collect();
    found.sort_by_key(|d| (d.first, d.last));
    for (n, d) in found.iter().enumerate() {
        for item in &mut items[d.first..=d.last] {
            let same = item
                .heard
                .as_ref()
                .is_some_and(|h| h.media == d.media && h.stream == d.stream);
            if same && item.differs.is_none() {
                item.differs = Some(n);
            }
        }
    }
    let first = entries.first();
    let file = |e: &Value| (e["media"].clone(), e["stream"].clone());
    let one = first.filter(|f| entries.iter().all(|e| file(e) == file(f)));
    Read {
        items,
        differences: found,
        media: one.and_then(|e| e["media"].as_str()).map(String::from),
        stream: first.and_then(|e| e["stream"].as_u64()).unwrap_or(0) as u32,
        transcripts: first
            .and_then(|e| e["transcripts"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|t| {
                Some(Transcript {
                    key: t["key"].as_str()?.to_string(),
                    label: t["label"].as_str()?.to_string(),
                    main: t["main"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
    }
}

fn person(item: &Item) -> Option<&str> {
    item.who.as_ref().map(|w| w.person.as_str())
}

fn people(items: &[Item]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for w in items.iter().filter_map(|i| i.who.as_ref()) {
        if !out.iter().any(|(p, _)| *p == w.person) {
            out.push((w.person.clone(), w.name.clone()));
        }
    }
    out
}

fn paragraphs(items: &[Item]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, item) in items.iter().enumerate() {
        if i > start && !item.silence && person(item) != person(&items[i - 1]) {
            out.push(start..i);
            start = i;
        }
        if item.silence {
            if start < i {
                out.push(start..i);
            }
            out.push(i..i + 1);
            start = i + 1;
        }
    }
    if start < items.len() {
        out.push(start..items.len());
    }
    out
}

fn text_of(items: &[Item]) -> String {
    let mut out = String::new();
    for item in items.iter().filter(|i| !i.silence) {
        if !out.is_empty() && !item.joined {
            out.push(' ');
        }
        out.push_str(&item.text);
    }
    out
}

fn sentence_end(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', ')', '”', '»'])
        .ends_with(['.', '!', '?', '…'])
}

fn sentence(items: &[Item], at: usize) -> (usize, usize) {
    let mut a = at;
    while a > 0 && !items[a - 1].silence && !sentence_end(&items[a - 1].text) {
        a -= 1;
    }
    let mut b = at;
    while b + 1 < items.len() && !items[b + 1].silence && !sentence_end(&items[b].text) {
        b += 1;
    }
    (a, b)
}

fn is_left_out(item: &Item, spans: &[(i64, i64)]) -> bool {
    spans.iter().any(|&(a, b)| item.start >= a && item.end <= b)
}

fn layout(
    ui: &egui::Ui,
    items: &[Item],
    range: Range<usize>,
    look: &Look,
    playhead: Option<usize>,
) -> Paragraph {
    let playhead = playhead.filter(|i| range.contains(i));
    let c = theme::colors();
    let mut job = LayoutJob::default();
    job.wrap.max_width = look.width.max(40) as f32;
    let mut chars = Vec::new();
    let mut count = 0;
    let picked = |i: usize| look.selection.is_some_and(|(a, b)| a <= i && i <= b);
    for i in range.clone() {
        let item = &items[i];
        let font = FontId::proportional(15.0);
        if i > range.start && !item.joined {
            let mut gap = TextFormat::simple(font.clone(), c.text);
            if picked(i) && picked(i - 1) {
                gap.background = c.selection;
            }
            job.append(" ", 0.0, gap);
            count += 1;
        }
        let dim = item.silence || item.filler;
        let mut f = TextFormat::simple(font, if dim { c.text_dim } else { c.text });
        f.italics = dim;
        if is_left_out(item, &look.left_out) {
            f.color = c.marked;
            f.strikethrough = Stroke::new(1.5, c.marked);
        }
        if look.hits.binary_search(&i).is_ok() {
            f.background = FOUND;
        }
        if picked(i) {
            f.background = c.selection;
            f.color = c.selected_text;
        }
        if playhead == Some(i) {
            f.underline = Stroke::new(2.0, c.playhead);
        }
        let n = item.text.chars().count();
        job.append(&item.text, 0.0, f);
        chars.push((count, count + n, i));
        count += n;
    }
    let galley = ui.painter().layout_job(job);
    let dotted = chars
        .iter()
        .filter(|(_, _, i)| items[*i].differs.is_some())
        .filter_map(|&(a, b, _)| {
            let mut from = CCursor::new(a);
            from.prefer_next_row = true;
            let (left, right) = (
                galley.pos_from_cursor(from),
                galley.pos_from_cursor(CCursor::new(b)),
            );
            ((left.bottom() - right.bottom()).abs() < 1.0).then(|| {
                let y = left.bottom() - 1.0;
                [pos2(left.left(), y), pos2(right.left(), y)]
            })
        })
        .collect();
    Paragraph {
        items: range,
        galley,
        chars,
        dotted,
        playhead,
    }
}

fn word_spot(p: &Paragraph, i: usize) -> Option<Pos2> {
    let (a, _, _) = p.chars.iter().find(|(_, _, k)| *k == i)?;
    let mut at = CCursor::new(*a);
    at.prefer_next_row = true;
    Some(p.galley.pos_from_cursor(at).left_bottom())
}

fn hit(p: &Paragraph, at: Vec2) -> usize {
    let index = p.galley.cursor_from_pos(at).index.0;
    p.chars
        .iter()
        .rev()
        .find(|(start, _, _)| *start <= index)
        .or(p.chars.first())
        .map_or(p.items.start, |(_, _, i)| *i)
}

fn at_frame(items: &[Item], frame: i64) -> Option<usize> {
    let i = items.partition_point(|x| x.start <= frame);
    (i > 0 && frame < items[i - 1].end).then(|| i - 1)
}

fn speaker_menu(
    ui: &mut egui::Ui,
    w: &Who,
    heard: &Heard,
    everyone: &[(String, String)],
    naming: &mut String,
    acts: &mut Vec<Act>,
) {
    let c = theme::colors();
    let r = ui.add(
        egui::Button::new(RichText::new(&w.name).small().strong().color(c.accent)).frame(false),
    );
    if r.clicked() {
        *naming = w.name.clone();
    }
    egui::Popup::menu(&r).show(|ui| {
        ui.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(naming).desired_width(140.0));
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("Rename").clicked() || enter {
                acts.push(Act::Name(w.person.clone(), naming.trim().to_string()));
                ui.close();
            }
        });
        if w.speaker == 0 {
            return;
        }
        let others: Vec<_> = everyone.iter().filter(|(p, _)| *p != w.person).collect();
        if !others.is_empty() {
            ui.menu_button("Same Person As", |ui| {
                for (person, name) in others {
                    if ui.button(name).clicked() {
                        acts.push(Act::Link(heard.clone(), w.speaker, person.clone()));
                        ui.close();
                    }
                }
            });
        }
        if ui.button("Separate").clicked() {
            let voice = vidcut_core::speakers::voice(&heard.media, heard.stream, w.speaker);
            acts.push(Act::Link(heard.clone(), w.speaker, voice));
            ui.close();
        }
    });
}

impl Workspace {
    pub(super) fn transcription_window(&mut self, ctx: &egui::Context) {
        let mut t = std::mem::take(&mut self.transcription);
        if t.open {
            self.transcription_frame(ctx, &mut t);
        } else {
            t.over = false;
            t.keys.clear();
            t.editing = None;
        }
        self.transcription = t;
    }

    fn shown_source(&self, p: &Project) -> (Option<Shown>, Vec<String>, Vec<String>) {
        if self.snap.viewer == ViewerMode::Timeline {
            let mut clips: Vec<(i64, String)> = Vec::new();
            let mut missing: Vec<String> = Vec::new();
            for c in p.tracks.iter().flat_map(|t| &t.clips) {
                if c.audio.is_none() || c.is_block() {
                    continue;
                }
                if self.snap.transcribed.contains(&c.media) {
                    clips.push((c.start, c.id.clone()));
                } else if !missing.contains(&c.media) {
                    missing.push(c.media.clone());
                }
            }
            clips.sort();
            let clips = clips.into_iter().map(|(_, id)| id).collect();
            return (Some(Shown::Timeline), clips, missing);
        }
        match self.snap.source_at() {
            Some((media, _)) => {
                let missing = match self.snap.transcribed.contains(media) {
                    true => Vec::new(),
                    false => vec![media.to_string()],
                };
                (Some(Shown::Media(media.to_string())), Vec::new(), missing)
            }
            None => (None, Vec::new(), Vec::new()),
        }
    }

    fn transcription_frame(&mut self, ctx: &egui::Context, t: &mut TranscriptionUi) {
        let Some(p) = self.project_shared() else {
            super::dialog::tool_window(ctx, "Transcription", &mut t.open, [520.0, 300.0], |ui| {
                super::dialog::note(ui, "Open a project first")
            });
            return;
        };
        let (shown, clips, missing) = self.shown_source(&p);
        let fps = p.settings.fps.as_f64();
        let pause = (fps * PAUSE).round() as i64;
        let preferred = self.snap.fillers.clone();
        let want = shown.clone().map(|shown| Asked {
            shown,
            rev: p.rev,
            transcribed: Arc::as_ptr(&self.snap.transcribed) as usize,
            clips: clips.clone(),
            fillers_in: t.fillers_in.clone(),
            preferred: preferred.clone(),
        });
        let have_words = match &want {
            Some(Asked {
                shown: Shown::Timeline,
                ..
            }) => !clips.is_empty(),
            Some(Asked {
                shown: Shown::Media(_),
                ..
            }) => missing.is_empty(),
            None => false,
        };
        if let Some(want) = want.filter(|_| have_words)
            && t.shown.as_ref().map(|(a, _)| a) != Some(&want)
            && t.asking.as_ref().map(|(a, _)| a) != Some(&want)
        {
            let mut params = match &want.shown {
                Shown::Media(m) => json!({ "media": m }),
                Shown::Timeline => json!({ "clips": want.clips }),
            };
            if let Some(language) = &want.fillers_in {
                params["language"] = json!(language);
            }
            t.asking = Some((
                want.clone(),
                Ask::send(&self.live, ctx, "analyze.words", params),
            ));
        }
        if let Some((asked, ask)) = &t.asking
            && let Some(answer) = ask.take()
        {
            let (bounds, gaps) = match &asked.shown {
                Shown::Media(m) => (p.media(m).map(|m| (0, p.media_frames(m))), Vec::new()),
                Shown::Timeline => (None, holes(&p)),
            };
            if let Ok(r) = &answer {
                t.detected = r
                    .result
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find_map(|e| e["filler_language"].as_str().map(String::from));
            }
            let read = answer
                .map(|r| Arc::new(parse(&r.result, pause, bounds, &gaps)))
                .map_err(|e| e.message);
            let keep = match (&t.shown, &read) {
                (Some((a, Ok(old))), Ok(new)) if a.shown == asked.shown => {
                    asked.shown != Shown::Timeline || old.items == new.items
                }
                _ => false,
            };
            if !keep {
                t.selection = None;
                t.anchor = None;
                t.found = None;
                t.difference = None;
                t.typing = None;
            }
            t.picking = None;
            t.shown = Some((asked.clone(), read));
            t.version += 1;
            t.asking = None;
        }
        let media = match &shown {
            Some(Shown::Media(m)) => p.media(m).cloned(),
            _ => None,
        };
        let left_out = media
            .as_ref()
            .map(|m| m.left_out.clone())
            .unwrap_or_default();
        let showing = t
            .shown
            .as_ref()
            .filter(|(a, _)| Some(&a.shown) == shown.as_ref());
        let read = showing
            .and_then(|(_, r)| r.as_ref().ok().cloned())
            .filter(|_| have_words);
        let error = showing.and_then(|(_, r)| r.as_ref().err().cloned());
        let items = read.as_deref().map(|r| r.items.as_slice());
        let found = read
            .as_deref()
            .map_or(&[][..], |r| r.differences.as_slice());
        let playhead = match &shown {
            Some(Shown::Timeline) => Some(self.snap.timeline.position),
            Some(Shown::Media(_)) => self.snap.source_at().map(|(_, f)| f),
            None => None,
        };
        let playing = match &shown {
            Some(Shown::Timeline) => self.snap.timeline.playing,
            Some(Shown::Media(_)) => self.snap.source.playing,
            None => false,
        };
        let tc = |ws: &Workspace, f: i64| match shown {
            Some(Shown::Timeline) => ws.tc(f),
            _ => ws.src_tc(f),
        };
        let mut acts: Vec<Act> = Vec::new();
        if let Some(items) = &items {
            for key in std::mem::take(&mut t.keys) {
                match key {
                    Pressed::Delete if t.selection.is_some() => acts.push(Act::Delete),
                    Pressed::Copy if t.selection.is_some() => acts.push(Act::Copy),
                    Pressed::Cut if t.selection.is_some() => acts.push(Act::Cut),
                    Pressed::Paste => acts.push(Act::Paste),
                    Pressed::All if !items.is_empty() => t.selection = Some((0, items.len() - 1)),
                    Pressed::Clear if t.picking.is_some() => t.picking = None,
                    Pressed::Clear => t.selection = None,
                    Pressed::Pick(k) => {
                        if let Some(n) = t.picking
                            && found.get(n).is_some_and(|d| k < d.picks.len())
                        {
                            acts.push(Act::Pick(n, k));
                        }
                    }
                    Pressed::Sentence => {
                        let at = t
                            .anchor
                            .or_else(|| playhead.and_then(|f| at_frame(items, f)))
                            .filter(|i| *i < items.len());
                        if let Some(at) = at {
                            let (a, b) = sentence(items, at);
                            t.selection = Some((a, b));
                            acts.push(Act::Marks(items[a].start, items[b].end));
                        }
                    }
                    _ => {}
                }
            }
        }
        t.keys.clear();
        let title = match (&shown, &media) {
            (Some(Shown::Timeline), _) => "Timeline".to_string(),
            (Some(Shown::Media(_)), Some(m)) => m.name.clone(),
            _ => "No source clip".to_string(),
        };
        let mut open = t.open;
        let mut over = false;
        let mut go = None;
        let mut go_difference = None;
        let mut opened = false;
        let needle = t.search.trim().to_lowercase();
        let hits = match &items {
            Some(items) if !needle.is_empty() => t.searches.hits(t.version, &needle, items),
            _ => Arc::default(),
        };
        super::dialog::tool_window(ctx, "Transcription", &mut open, [560.0, 620.0], |ui| {
            ui.add(egui::Label::new(RichText::new(&title).strong()).truncate());
            super::dialog::form(ui, "transcription-form", |ui| {
                super::dialog::row(ui, "Search", |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut t.search)
                            .hint_text("a word")
                            .desired_width(150.0),
                    );
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let back = ui.input(|i| i.modifiers.shift);
                        go = Some(if back { Jump::Previous } else { Jump::Next });
                        r.request_focus();
                    }
                });
                if !needle.is_empty() {
                    ui.label("");
                    ui.horizontal_wrapped(|ui| {
                        let any = !hits.is_empty();
                        for (label, to) in JUMPS {
                            if ui.add_enabled(any, egui::Button::new(label)).clicked() {
                                go = Some(to);
                            }
                        }
                        match t.found.and_then(|f| hits.iter().position(|&h| h == f)) {
                            Some(k) => {
                                ui.label(format!("{} of {}", k + 1, hits.len()));
                            }
                            None if hits.len() == 1 => {
                                ui.label("1 match");
                            }
                            None if any => {
                                ui.label(format!("{} matches", hits.len()));
                            }
                            None => super::dialog::why(ui, "no matches"),
                        }
                    });
                    ui.end_row();
                }
                let transcripts = read
                    .as_deref()
                    .map_or(&[][..], |r| r.transcripts.as_slice());
                if read.as_ref().is_some_and(|r| r.media.is_some()) && !transcripts.is_empty() {
                    super::dialog::row(ui, "Transcript", |ui| {
                        let main = transcripts.iter().find(|x| x.main).or(transcripts.first());
                        egui::ComboBox::from_id_salt("transcription-main")
                            .selected_text(main.map_or("", |m| m.label.as_str()))
                            .show_ui(ui, |ui| {
                                for x in transcripts {
                                    if ui.selectable_label(x.main, &x.label).clicked() && !x.main {
                                        acts.push(Act::Main(x.key.clone()));
                                    }
                                }
                            });
                        let others: Vec<&Transcript> =
                            transcripts.iter().filter(|x| !x.main).collect();
                        if !others.is_empty() {
                            ui.menu_button("Remove", |ui| {
                                for x in others {
                                    if ui.button(&x.label).clicked() {
                                        acts.push(Act::Remove(x.key.clone()));
                                        ui.close();
                                    }
                                }
                            });
                        }
                    });
                }
                if !found.is_empty() || transcripts.len() > 1 {
                    super::dialog::row(ui, "Differences", |ui| {
                        for (label, to) in JUMPS {
                            if ui
                                .add_enabled(!found.is_empty(), egui::Button::new(label))
                                .clicked()
                            {
                                go_difference = Some(to);
                            }
                        }
                        match t.difference.filter(|d| *d < found.len()) {
                            Some(d) => {
                                ui.label(format!("{} of {}", d + 1, found.len()));
                            }
                            None if found.len() == 1 => {
                                ui.label("1 difference");
                            }
                            None if !found.is_empty() => {
                                ui.label(format!("{} differences", found.len()));
                            }
                            None => super::dialog::why(ui, "the transcripts agree"),
                        }
                    });
                }
                super::dialog::row(ui, "Playhead", |ui| {
                    let mut follow = !t.hold;
                    if ui.checkbox(&mut follow, "Follow").changed() {
                        t.hold = !follow;
                    }
                });
                super::dialog::row(ui, "Silences", |ui| {
                    let silences = items.as_ref().is_some_and(|i| i.iter().any(|x| x.silence));
                    if ui
                        .add_enabled(silences, egui::Button::new("Remove Silent Portions"))
                        .clicked()
                    {
                        acts.push(Act::RemoveSilences);
                    }
                    if !silences {
                        super::dialog::why(ui, "no pause of a second or more");
                    }
                });
                super::dialog::row(ui, "Filler Words", |ui| {
                    let fillers = items.as_ref().map_or(0, |i| {
                        i.iter()
                            .filter(|x| x.filler && !is_left_out(x, &left_out))
                            .count()
                    });
                    if ui
                        .add_enabled(fillers > 0, egui::Button::new(format!("Delete {fillers}")))
                        .clicked()
                    {
                        acts.push(Act::Fillers);
                    }
                    if t.fillers_in.is_none() && t.detected.is_none() {
                        super::dialog::why(ui, "no language known: pick one");
                    }
                    let auto = match &t.detected {
                        Some(code) => format!("Auto: {}", super::speech::language_label(code)),
                        None => "Auto".to_string(),
                    };
                    let chosen = t
                        .fillers_in
                        .as_deref()
                        .map_or_else(|| auto.clone(), super::speech::language_label);
                    egui::ComboBox::from_id_salt("transcription-fillers")
                        .selected_text(chosen)
                        .height(320.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut t.fillers_in, None, auto.clone());
                            for code in vidcut_core::transcript::FILLER_LANGUAGES {
                                ui.selectable_value(
                                    &mut t.fillers_in,
                                    Some(code.to_string()),
                                    super::speech::language_label(code),
                                );
                            }
                        });
                    if ui.button("Edit…").clicked() {
                        let language = t
                            .fillers_in
                            .clone()
                            .or_else(|| t.detected.clone())
                            .unwrap_or_else(|| "en".into());
                        t.editing = Some(FillerEditor::new(language, Scope::Project));
                        t.edit_error = None;
                    }
                });
            });
            if !missing.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    let (text, button) = match shown {
                        Some(Shown::Timeline) => (
                            format!(
                                "The media of {} isn't transcribed",
                                super::speech::listed(
                                    &missing
                                        .iter()
                                        .map(|m| p.media(m).map_or(m.clone(), |x| x.name.clone()))
                                        .collect::<Vec<_>>()
                                )
                            ),
                            "Transcribe Missing Clips",
                        ),
                        _ => ("Not transcribed yet".to_string(), "Transcribe Audio…"),
                    };
                    super::dialog::warn(ui, text);
                    if ui.button(button).clicked() {
                        acts.push(Act::Transcribe(missing.clone()));
                    }
                });
            }
            if let Some(e) = &error {
                super::dialog::warn(ui, e.clone());
            }
            ui.separator();
            let height = (ui.available_height() - t.footer.max(FOOTER)).max(80.0);
            let mut placed: Vec<(usize, Rect)> = Vec::new();
            let mut visible = Rect::NOTHING;
            let mut spot: Option<Pos2> = None;
            match &items {
                None if shown.is_none() => {
                    ui.label(dim(
                        "Open a clip in the Source viewer, or show the timeline",
                    ));
                }
                None if have_words => {
                    ui.label(dim("Reading the words…"));
                }
                None => {}
                Some([]) => {
                    ui.label(dim("No words were heard"));
                }
                Some(items) => {
                    let width = ui.available_width() - 16.0;
                    let look = Look {
                        version: t.version,
                        selection: t.selection,
                        left_out: left_out.clone(),
                        hits: hits.clone(),
                        width: width as i32,
                    };
                    let now = playhead.and_then(|f| at_frame(items, f));
                    if !t.hold && playing && now.is_some() && now != t.followed {
                        t.scroll_to = now.map(|i| (i, false));
                    }
                    t.followed = now;
                    match &mut t.look {
                        Some((l, paras)) if *l == look => {
                            for para in paras.iter_mut() {
                                let here = now.filter(|i| para.items.contains(i));
                                if para.playhead != here {
                                    *para = layout(ui, items, para.items.clone(), &look, now);
                                }
                            }
                        }
                        _ => {
                            let laid = paragraphs(items)
                                .into_iter()
                                .map(|r| layout(ui, items, r, &look, now))
                                .collect();
                            t.look = Some((look, laid));
                        }
                    }
                    let paras = &t.look.as_ref().expect("laid out").1;
                    let everyone = people(items);
                    egui::ScrollArea::vertical()
                        .id_salt("transcription-text")
                        .auto_shrink([false, false])
                        .max_height(height)
                        .show(ui, |ui| {
                            visible = ui.clip_rect();
                            let mut speaking: Option<&str> = None;
                            for (k, para) in paras.iter().enumerate() {
                                let first = &items[para.items.start];
                                if !first.silence {
                                    let new = first
                                        .who
                                        .as_ref()
                                        .filter(|w| speaking != Some(w.person.as_str()));
                                    ui.horizontal(|ui| {
                                        if let (Some(w), Some(heard)) = (new, &first.heard) {
                                            speaker_menu(
                                                ui,
                                                w,
                                                heard,
                                                &everyone,
                                                &mut t.naming,
                                                &mut acts,
                                            );
                                        }
                                        ui.label(
                                            RichText::new(tc(self, first.start))
                                                .small()
                                                .color(theme::colors().text_dim),
                                        );
                                    });
                                    if let Some(p) = person(first) {
                                        speaking = Some(p);
                                    }
                                }
                                let (rect, _) = ui.allocate_exact_size(
                                    para.galley.size(),
                                    Sense::click_and_drag(),
                                );
                                ui.painter().galley(
                                    rect.min,
                                    para.galley.clone(),
                                    theme::colors().text,
                                );
                                for [a, b] in &para.dotted {
                                    let line = [rect.min + a.to_vec2(), rect.min + b.to_vec2()];
                                    ui.painter().extend(Shape::dotted_line(
                                        &line,
                                        theme::colors().accent,
                                        3.0,
                                        0.9,
                                    ));
                                }
                                if let Some(d) = t.picking.and_then(|n| found.get(n))
                                    && para.items.contains(&d.first)
                                {
                                    spot = word_spot(para, d.first).map(|p| rect.min + p.to_vec2());
                                }
                                if let Some((i, center)) = t.scroll_to
                                    && para.items.contains(&i)
                                {
                                    let align = center.then_some(egui::Align::Center);
                                    ui.scroll_to_rect(rect, align);
                                    t.scroll_to = None;
                                }
                                placed.push((k, rect));
                                ui.add_space(if first.silence { 2.0 } else { 6.0 });
                            }
                        });
                }
            }
            if let Some(items) = &items
                && let Some((_, paras)) = &t.look
            {
                let pointer = ui.input(|i| i.pointer.interact_pos());
                let hovered = pointer
                    .filter(|pos| visible.contains(*pos) && ui.rect_contains_pointer(visible))
                    .and_then(|pos| {
                        let (k, rect) = placed.iter().find(|(_, r)| r.contains(pos))?;
                        Some(hit(&paras[*k], pos - rect.min))
                    });
                let (pressed, down, released) = ui.input(|i| {
                    (
                        i.pointer.primary_pressed(),
                        i.pointer.primary_down(),
                        i.pointer.primary_released(),
                    )
                });
                if pressed && let Some(h) = hovered {
                    t.anchor = Some(h);
                    t.selection = Some((h, h));
                    t.dragging = true;
                    t.moved = false;
                }
                if t.dragging
                    && down
                    && let (Some(a), Some(h)) = (t.anchor, hovered)
                {
                    t.moved |= h != a;
                    t.selection = Some((a.min(h), a.max(h)));
                }
                if t.dragging && released {
                    t.dragging = false;
                    if let Some((a, _, start, end)) = t.selected(items) {
                        match t.moved {
                            false => acts.push(Act::Seek(items[a].end)),
                            true => acts.push(Act::Marks(start, end)),
                        }
                        t.picking = items[a].differs.filter(|_| !t.moved);
                        if t.picking.is_some() {
                            t.difference = t.picking;
                            opened = true;
                        }
                    }
                }
            }
            if let Some(n) = t.picking
                && let (Some(d), Some(items), Some(at)) = (found.get(n), items, spot)
                && visible.contains(at)
            {
                let now = text_of(&items[d.first..=d.last]);
                let mut typing = false;
                let area = egui::Area::new(egui::Id::unique("transcription-picks"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(at + Vec2::new(0.0, 2.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            for (k, pick) in d.picks.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let text = match pick.text.is_empty() {
                                        true => "(no words)",
                                        false => pick.text.as_str(),
                                    };
                                    let label = format!("{}  {text}", k + 1);
                                    if ui.selectable_label(pick.text == now, label).clicked() {
                                        acts.push(Act::Pick(n, k));
                                    }
                                    ui.label(
                                        RichText::new(&pick.names)
                                            .small()
                                            .color(theme::colors().text_dim),
                                    );
                                });
                            }
                            ui.separator();
                            typing = ui.button("Edit…").clicked();
                        });
                    });
                if typing {
                    t.typing = Some(Typing {
                        media: d.media.clone(),
                        stream: d.stream,
                        start: d.start,
                        end: d.end,
                        text: now,
                        focus: true,
                    });
                    t.picking = None;
                } else if area.response.clicked_elsewhere() && !opened {
                    t.picking = None;
                }
            }
            let footer_top = ui.cursor().top();
            ui.separator();
            let selected = items.as_ref().and_then(|i| t.selected(i));
            ui.horizontal_wrapped(|ui| match (&items, selected) {
                (Some(items), Some((a, b, start, end))) => {
                    let words = items[a..=b].iter().filter(|i| !i.silence).count();
                    ui.label(format!(
                        "{words} word(s), {} – {} ({})",
                        tc(self, start),
                        tc(self, end - 1),
                        self.src_tc(end - start)
                    ));
                }
                _ => {
                    ui.label(dim(
                        "Click a word to go there; drag over words to mark them.",
                    ));
                }
            });
            ui.horizontal_wrapped(|ui| {
                let any = selected.is_some();
                let button = |ui: &mut egui::Ui, label: &str| {
                    ui.add_enabled(any, egui::Button::new(label)).clicked()
                };
                match shown {
                    Some(Shown::Timeline) => {
                        if button(ui, "Cut") {
                            acts.push(Act::Cut);
                        }
                        let pasted = self.snap.clipboard > 0;
                        if ui.add_enabled(pasted, egui::Button::new("Paste")).clicked() {
                            acts.push(Act::Paste);
                        }
                        if button(ui, "Delete") {
                            acts.push(Act::Delete);
                        }
                        if button(ui, "Add Marker") {
                            acts.push(Act::Marker);
                        }
                    }
                    Some(Shown::Media(_)) => {
                        if button(ui, "Delete") {
                            acts.push(Act::Delete);
                        }
                        if button(ui, "Undelete") {
                            acts.push(Act::Undelete);
                        }
                        ui.separator();
                        for (label, method) in [
                            ("Place on Top", "edit.place_on_top"),
                            ("Insert", "edit.insert"),
                            ("Append", "edit.append"),
                        ] {
                            if button(ui, label) {
                                acts.push(Act::Edit(method));
                            }
                        }
                    }
                    None => {}
                }
                if button(ui, "Copy") {
                    acts.push(Act::Copy);
                }
                if !any {
                    super::dialog::why(ui, "select words first");
                }
                let one_file = selected.zip(items).and_then(|((a, b, _, _), items)| {
                    let mut heard = items[a..=b].iter().filter_map(|i| i.heard.as_ref());
                    let first = heard.next()?;
                    let mut end = first.file.1;
                    for h in heard {
                        if (&h.media, h.stream) != (&first.media, first.stream) {
                            return None;
                        }
                        end = end.max(h.file.1);
                    }
                    let spoken = items[a..=b].iter().any(|i| i.who.is_some());
                    Some((first.clone(), end, text_of(&items[a..=b]), spoken))
                });
                if let Some((first, end, text, spoken)) = one_file {
                    if ui.button("Edit").clicked() {
                        t.typing = Some(Typing {
                            media: first.media.clone(),
                            stream: first.stream,
                            start: first.file.0,
                            end,
                            text,
                            focus: true,
                        });
                        t.picking = None;
                    }
                    if let (true, Some(items)) = (spoken, items) {
                        ui.menu_button("Assign To", |ui| {
                            for (person, name) in people(items) {
                                if ui.button(name).clicked() {
                                    acts.push(Act::Assign(first.clone(), end, person));
                                }
                            }
                        });
                    }
                }
            });
            let mut cancel = false;
            if let Some(typing) = &mut t.typing {
                ui.horizontal(|ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut typing.text)
                            .desired_width((ui.available_width() - 150.0).max(120.0)),
                    );
                    if typing.focus {
                        field.request_focus();
                        typing.focus = false;
                    }
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let ready = !typing.text.trim().is_empty();
                    cancel = ui.button("Cancel").clicked();
                    if ui.add_enabled(ready, egui::Button::new("Change")).clicked()
                        || (enter && ready)
                    {
                        acts.push(Act::Retype);
                    }
                });
            }
            if cancel {
                t.typing = None;
            }
            t.footer = (ui.cursor().top() - footer_top).ceil();
            over = ui.ui_contains_pointer();
        });
        if let (Some(to), Some(items)) = (go, &items)
            && !hits.is_empty()
        {
            let i = jump(&hits, t.found, to);
            t.found = Some(i);
            t.selection = Some((i, i));
            t.anchor = Some(i);
            t.scroll_to = Some((i, true));
            acts.push(Act::Seek(items[i].start));
        }
        if let (Some(to), Some(items)) = (go_difference, items)
            && !found.is_empty()
        {
            let firsts: Vec<usize> = found.iter().map(|d| d.first).collect();
            let i = jump(&firsts, t.anchor, to);
            if let Some(n) = found.iter().position(|d| d.first == i) {
                t.difference = Some(n);
                t.picking = Some(n);
                t.selection = Some((found[n].first, found[n].last));
                t.anchor = Some(i);
                t.scroll_to = Some((i, true));
                acts.push(Act::Seek(items[i].start));
            }
        }
        t.open = open;
        t.over = over;
        self.filler_words_window(ctx, t, &p.settings.fillers, &preferred);
        if let Some(read) = &read {
            for act in acts {
                let m = media.as_ref().map(|m| m.id.as_str());
                self.transcription_act(ctx, act, t, read, m, &left_out);
            }
        } else {
            for act in acts {
                if let Act::Transcribe(ids) = act {
                    self.open_transcribe(ids);
                }
            }
        }
    }

    fn filler_words_window(
        &mut self,
        ctx: &egui::Context,
        t: &mut TranscriptionUi,
        project: &Lists,
        preferred: &Lists,
    ) {
        if let Some((scope, ask)) = &t.saving
            && let Some(answer) = ask.take()
        {
            match answer {
                Ok(_) => {
                    t.edit_error = None;
                    if *scope == Scope::All {
                        self.reread_preferences(ctx);
                    }
                }
                Err(e) => t.edit_error = Some(e.message),
            }
            t.saving = None;
        }
        let Some(e) = &mut t.editing else { return };
        let mut open = true;
        let mut change = None;
        super::dialog::window(ctx, "Filler Words", &mut open, |ui| {
            change = super::fillers::editor(ui, e, Some(project), preferred, t.saving.is_some());
            if let Some(error) = &t.edit_error {
                super::dialog::warn(ui, error.clone());
            }
        });
        if !open {
            t.editing = None;
            t.edit_error = None;
        }
        if let Some(change) = change {
            let (method, params) = change.request();
            t.saving = Some((change.scope, Ask::send(&self.live, ctx, method, params)));
        }
    }

    fn transcription_act(
        &mut self,
        ctx: &egui::Context,
        act: Act,
        t: &mut TranscriptionUi,
        read: &Read,
        media: Option<&str>,
        left_out: &[(i64, i64)],
    ) {
        let items = read.items.as_slice();
        let span = |a: i64, b: i64| json!({ "start": format!("{a}f"), "end": format!("{b}f") });
        let selected = t.selected(items);
        match act {
            Act::Transcribe(ids) => self.open_transcribe(ids),
            Act::Name(person, name) => {
                self.send("speakers.set", json!({ "person": person, "name": name }));
            }
            Act::Link(h, speaker, person) => self.send(
                "speakers.link",
                json!({
                    "media": h.media,
                    "stream": h.stream,
                    "speaker": speaker,
                    "person": person,
                }),
            ),
            Act::Assign(h, end, person) => self.send(
                "speakers.assign",
                json!({
                    "media": h.media,
                    "stream": h.stream,
                    "start": h.file.0,
                    "end": end,
                    "person": person,
                }),
            ),
            Act::Main(key) => {
                if let Some(m) = &read.media {
                    self.send(
                        "media.transcripts.main",
                        json!({ "media": m, "stream": read.stream, "transcript": key }),
                    );
                }
            }
            Act::Remove(key) => {
                if let Some(m) = &read.media {
                    self.send(
                        "media.transcripts.remove",
                        json!({ "media": m, "stream": read.stream, "transcripts": [key] }),
                    );
                }
            }
            Act::Pick(n, k) => {
                let Some(d) = read.differences.get(n) else {
                    return;
                };
                let Some(pick) = d.picks.get(k) else { return };
                self.send(
                    "media.words.set",
                    json!({
                        "media": d.media,
                        "stream": d.stream,
                        "start": d.start,
                        "end": d.end,
                        "from": pick.from,
                    }),
                );
                t.picking = None;
            }
            Act::Retype => {
                let Some(typing) = t.typing.take() else {
                    return;
                };
                self.send(
                    "media.words.set",
                    json!({
                        "media": typing.media,
                        "stream": typing.stream,
                        "start": typing.start,
                        "end": typing.end,
                        "text": typing.text.trim(),
                    }),
                );
            }
            Act::Seek(frame) => match media {
                Some(_) => {
                    let offset = self
                        .snap
                        .source_at()
                        .map_or(0, |(_, f)| self.snap.source.position - f);
                    self.seek(PlayerId::Source, frame + offset, false);
                }
                None => self.seek(PlayerId::Timeline, frame, false),
            },
            Act::Marks(a, b) => match media {
                Some(m) => self.send(
                    "media.marks.set",
                    json!({ "media": m, "in": format!("{a}f"), "out": format!("{b}f") }),
                ),
                None => self.send(
                    "marks.set",
                    json!({ "in": format!("{a}f"), "out": format!("{b}f") }),
                ),
            },
            Act::Copy | Act::Cut => {
                let Some((first, last, a, b)) = selected else {
                    return;
                };
                ctx.copy_text(text_of(&items[first..=last]));
                if media.is_some() {
                    return;
                }
                let range = json!({ "range": format!("{a}f..{b}f") });
                match act {
                    Act::Cut => {
                        self.send("edit.cut", range);
                        t.selection = None;
                        t.anchor = None;
                    }
                    _ => self.send("edit.copy", range),
                }
            }
            Act::Paste => {
                if media.is_some() {
                    return;
                }
                let ripple = self.project().is_some_and(|p| p.settings.ripple_on);
                let at = selected.map_or(self.snap.timeline.position, |(_, _, a, _)| a);
                let mut params = json!({ "at": format!("{at}f"), "insert": ripple });
                if let Some((_, _, a, b)) = selected {
                    params["duration"] = json!(format!("{}f", b - a));
                }
                self.send("edit.paste", params);
            }
            Act::Delete | Act::Undelete => {
                let Some((first, last, a, b)) = selected else {
                    return;
                };
                match media {
                    Some(m) => self.send(
                        "media.leave_out",
                        json!({
                            "media": m,
                            "words": [span(a, b)],
                            "back": matches!(act, Act::Undelete),
                        }),
                    ),
                    None => {
                        let marked = self.project().is_some_and(|p| {
                            p.marks.mark_in == Some(a) && p.marks.mark_out == Some(b)
                        });
                        let ripple = self.project().is_some_and(|p| p.settings.ripple_on);
                        self.send(
                            "edit.ripple_delete_words",
                            json!({
                                "words": [{
                                    "start": format!("{a}f"),
                                    "end": format!("{b}f"),
                                    "text": text_of(&items[first..=last]),
                                }],
                                "quiet": true,
                                "clear_marks": marked,
                                "lift": !ripple,
                            }),
                        );
                        t.selection = None;
                        t.anchor = None;
                    }
                }
            }
            Act::RemoveSilences | Act::Fillers => {
                let fillers = matches!(act, Act::Fillers);
                let words: Vec<Value> = items
                    .iter()
                    .filter(|i| match fillers {
                        true => i.filler && !is_left_out(i, left_out),
                        false => i.silence,
                    })
                    .map(|i| span(i.start, i.end))
                    .collect();
                if words.is_empty() {
                    return;
                }
                match media {
                    Some(m) => self.send("media.leave_out", json!({ "media": m, "words": words })),
                    None => {
                        self.send(
                            "edit.ripple_delete_words",
                            json!({ "words": words, "quiet": true }),
                        );
                        t.selection = None;
                        t.anchor = None;
                    }
                }
            }
            Act::Marker => {
                let Some((a, b, start, end)) = selected else {
                    return;
                };
                let name: String = text_of(&items[a..=b]).chars().take(60).collect();
                let mut params = json!({ "at": format!("{start}f"), "name": name });
                if end - start > 1 {
                    params["duration"] = json!(format!("{}f", end - start));
                }
                self.send("markers.add", params);
            }
            Act::Edit(method) => {
                let (Some(m), Some((_, _, a, b))) = (media, selected) else {
                    return;
                };
                let source = json!({ "media": m, "range": format!("{a}f..{b}f") });
                self.edit_from(method, vec![source]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(texts: &[&str]) -> Vec<Item> {
        texts
            .iter()
            .map(|t| Item {
                start: 0,
                end: 0,
                text: t.to_string(),
                lower: t.to_lowercase(),
                joined: false,
                silence: false,
                filler: false,
                heard: None,
                who: None,
                differs: None,
            })
            .collect()
    }

    #[test]
    fn a_search_finds_words_whatever_their_case() {
        let mut searches = Searches::default();
        let hits = searches.hits(1, "the", &words(&["The", "cat", "other"]));
        assert_eq!(&*hits, &[0, 2]);
    }

    #[test]
    fn a_longer_search_keeps_the_shorter_ones_matches_that_still_match() {
        let items = words(&["then", "thin", "other"]);
        let mut searches = Searches::default();
        searches.hits(1, "th", &items);
        assert_eq!(&*searches.hits(1, "the", &items), &[0, 2]);
    }

    #[test]
    fn a_new_transcript_forgets_the_searches() {
        let items = words(&["a"]);
        let mut searches = Searches::default();
        searches.hits(1, "a", &items);
        searches.hits(2, "b", &items);
        assert_eq!(searches.recent.iter().count(), 1);
    }

    #[test]
    fn next_goes_to_the_first_match_after_the_found_word() {
        assert_eq!(jump(&[2, 5, 9], Some(5), Jump::Next), 9);
    }

    #[test]
    fn next_from_the_last_match_wraps_to_the_first() {
        assert_eq!(jump(&[2, 5, 9], Some(9), Jump::Next), 2);
    }

    #[test]
    fn previous_from_the_first_match_wraps_to_the_last() {
        assert_eq!(jump(&[2, 5, 9], Some(2), Jump::Previous), 9);
    }

    #[test]
    fn previous_from_a_word_between_matches_goes_to_the_one_before_it() {
        assert_eq!(jump(&[2, 5, 9], Some(7), Jump::Previous), 5);
    }

    #[test]
    fn previous_with_nothing_found_goes_to_the_last_match() {
        assert_eq!(jump(&[2, 5, 9], None, Jump::Previous), 9);
    }
}
