use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontId, Galley, Key, Rect, RichText, Sense, Stroke, Vec2,
    text::{LayoutJob, TextFormat},
};
use serde_json::{Value, json};
use vidcut_core::model::{Project, uncovered};
use vidcut_core::request::ViewerMode;

use super::export::{Ask, near_center};
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
}

#[derive(Debug, Clone, PartialEq)]
struct Item {
    start: i64,
    end: i64,
    text: String,
    joined: bool,
    silence: bool,
    filler: bool,
}

type Words = Result<Arc<Vec<Item>>, String>;

#[derive(Debug, Clone, PartialEq)]
struct Look {
    version: u64,
    selection: Option<(usize, usize)>,
    left_out: Vec<(i64, i64)>,
    search: String,
    width: i32,
}

#[derive(Debug)]
struct Paragraph {
    items: Range<usize>,
    galley: Arc<Galley>,
    chars: Vec<(usize, usize, usize)>,
    playhead: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Pressed {
    Delete,
    Sentence,
    All,
    Copy,
    Clear,
}

enum Act {
    Seek(i64),
    Marks(i64, i64),
    Delete,
    Undelete,
    Edit(&'static str),
    Copy,
    Marker,
    RemoveSilences,
    Fillers,
    Transcribe(Vec<String>),
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
    search: String,
    found: Option<usize>,
    scroll_to: Option<(usize, bool)>,
    hold: bool,
    followed: Option<usize>,
    fillers_in: Option<String>,
    detected: Option<String>,
}

impl TranscriptionUi {
    pub(super) fn take_keys(&mut self, ctx: &egui::Context) {
        let keys = &mut self.keys;
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                let pressed = match e {
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

fn parse(answer: &Value, pause: i64, bounds: Option<(i64, i64)>) -> Vec<Item> {
    let mut words: Vec<Item> = answer
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|e| e["words"].as_array().into_iter().flatten())
        .filter_map(|w| {
            Some(Item {
                start: w["start"].as_i64()?,
                end: w["end"].as_i64()?,
                text: w["text"].as_str()?.to_string(),
                joined: w["joined"].as_bool().unwrap_or(false),
                silence: false,
                filler: w["filler"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    words.sort_by_key(|w| (w.start, w.end));
    let silence = |start: i64, end: i64| Item {
        start,
        end,
        text: "(...)".into(),
        joined: false,
        silence: true,
        filler: false,
    };
    let mut out: Vec<Item> = Vec::with_capacity(words.len() * 2);
    let mut last = bounds.map(|(a, _)| a);
    for w in words {
        if let Some(end) = last
            && w.start - end >= pause
        {
            out.push(silence(end, w.start));
        }
        last = Some(last.map_or(w.end, |end| end.max(w.end)));
        out.push(w);
    }
    if let (Some((_, b)), Some(end)) = (bounds, last)
        && b - end >= pause
    {
        out.push(silence(end, b));
    }
    out
}

fn paragraphs(items: &[Item]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, item) in items.iter().enumerate() {
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
    let needle = look.search.to_lowercase();
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
        if !needle.is_empty() && !item.silence && item.text.to_lowercase().contains(&needle) {
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
    Paragraph {
        items: range,
        galley: ui.painter().layout_job(job),
        chars,
        playhead,
    }
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

impl Workspace {
    pub(super) fn transcription_window(&mut self, ctx: &egui::Context) {
        let mut t = std::mem::take(&mut self.transcription);
        if t.open {
            self.transcription_frame(ctx, &mut t);
        } else {
            t.over = false;
            t.keys.clear();
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
            egui::Window::new("Transcription")
                .open(&mut t.open)
                .default_pos(near_center(ctx, 520.0, 300.0))
                .show(ctx, |ui| ui.label(dim("Open a project first")));
            return;
        };
        let (shown, clips, missing) = self.shown_source(&p);
        let fps = p.settings.fps.as_f64();
        let pause = (fps * PAUSE).round() as i64;
        let want = shown.clone().map(|shown| Asked {
            shown,
            rev: p.rev,
            transcribed: Arc::as_ptr(&self.snap.transcribed) as usize,
            clips: clips.clone(),
            fillers_in: t.fillers_in.clone(),
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
            let bounds = match &asked.shown {
                Shown::Media(m) => p.media(m).map(|m| (0, p.media_frames(m))),
                Shown::Timeline => None,
            };
            if let Ok(r) = &answer {
                t.detected = r
                    .result
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find_map(|e| e["filler_language"].as_str().map(String::from));
            }
            let items = answer
                .map(|r| Arc::new(parse(&r.result, pause, bounds)))
                .map_err(|e| e.message);
            let keep = match (&t.shown, &items) {
                (Some((a, Ok(old))), Ok(new)) if a.shown == asked.shown => {
                    asked.shown != Shown::Timeline || old == new
                }
                _ => false,
            };
            if !keep {
                t.selection = None;
                t.anchor = None;
                t.found = None;
            }
            t.shown = Some((asked.clone(), items));
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
        let items = showing.and_then(|(_, r)| r.as_ref().ok().cloned());
        let error = showing.and_then(|(_, r)| r.as_ref().err().cloned());
        let items = items.filter(|_| have_words);
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
                    Pressed::All if !items.is_empty() => t.selection = Some((0, items.len() - 1)),
                    Pressed::Clear => t.selection = None,
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
        let mut search_go = false;
        egui::Window::new("Transcription")
            .open(&mut open)
            .default_pos(near_center(ctx, 560.0, 620.0))
            .default_size([560.0, 620.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.add(egui::Label::new(RichText::new(&title).strong()).truncate());
                ui.horizontal_wrapped(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut t.search)
                            .hint_text("Search")
                            .desired_width(150.0),
                    );
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        search_go = true;
                        r.request_focus();
                    }
                    let mut follow = !t.hold;
                    if ui
                        .checkbox(&mut follow, "Follow")
                        .on_hover_text("While playing, keep the word under the playhead in view")
                        .changed()
                    {
                        t.hold = !follow;
                    }
                    let silences = items.as_ref().is_some_and(|i| i.iter().any(|x| x.silence));
                    if ui
                        .add_enabled(silences, egui::Button::new("Remove Silent Portions"))
                        .on_hover_text(match shown {
                            Some(Shown::Timeline) => {
                                "Ripple delete every pause of a second or more between words"
                            }
                            _ => "Leave every pause of a second or more out of the edits",
                        })
                        .clicked()
                    {
                        acts.push(Act::RemoveSilences);
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let fillers = items.as_ref().map_or(0, |i| {
                        i.iter()
                            .filter(|x| x.filler && !is_left_out(x, &left_out))
                            .count()
                    });
                    if ui
                        .add_enabled(
                            fillers > 0,
                            egui::Button::new(format!("Delete Filler Words ({fillers})")),
                        )
                        .on_hover_text(match shown {
                            Some(Shown::Timeline) => {
                                "Ripple delete every filler word (dimmed in the text), at the \
                                 quiet points around it"
                            }
                            _ => "Leave every filler word (dimmed in the text) out of the edits",
                        })
                        .clicked()
                    {
                        acts.push(Act::Fillers);
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
                        })
                        .response
                        .on_hover_text(
                            "Whose filler words are marked. Auto: the transcript's language, or \
                             the one its commonest words belong to",
                        );
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
                                            .map(|m| p
                                                .media(m)
                                                .map_or(m.clone(), |x| x.name.clone()))
                                            .collect::<Vec<_>>()
                                    )
                                ),
                                "Transcribe Missing Clips",
                            ),
                            _ => ("Not transcribed yet".to_string(), "Transcribe Audio…"),
                        };
                        ui.label(RichText::new(text).color(ui.visuals().warn_fg_color));
                        if ui.button(button).clicked() {
                            acts.push(Act::Transcribe(missing.clone()));
                        }
                    });
                }
                if let Some(e) = &error {
                    ui.label(RichText::new(e).color(theme::colors().error));
                }
                ui.separator();
                let footer = 64.0;
                let height = (ui.available_height() - footer).max(80.0);
                let mut placed: Vec<(usize, Rect)> = Vec::new();
                let mut visible = Rect::NOTHING;
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
                    Some(items) if items.is_empty() => {
                        ui.label(dim("No words were heard"));
                    }
                    Some(items) => {
                        let width = ui.available_width() - 16.0;
                        let look = Look {
                            version: t.version,
                            selection: t.selection,
                            left_out: left_out.clone(),
                            search: t.search.trim().to_string(),
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
                        egui::ScrollArea::vertical()
                            .id_salt("transcription-text")
                            .auto_shrink([false, false])
                            .max_height(height)
                            .show(ui, |ui| {
                                visible = ui.clip_rect();
                                for (k, para) in paras.iter().enumerate() {
                                    let first = &items[para.items.start];
                                    if !first.silence {
                                        ui.label(
                                            RichText::new(tc(self, first.start))
                                                .small()
                                                .color(theme::colors().text_dim),
                                        );
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
                        }
                    }
                }
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
                            "Click a word to go to it, drag over words for In and Out, X for a \
                             sentence",
                        ));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let any = selected.is_some();
                    let button = |ui: &mut egui::Ui, label: &str, tip: &str| {
                        ui.add_enabled(any, egui::Button::new(label))
                            .on_hover_text(tip)
                            .clicked()
                    };
                    match shown {
                        Some(Shown::Timeline) => {
                            if button(ui, "Delete", "Ripple delete these words (Delete)") {
                                acts.push(Act::Delete);
                            }
                            if button(
                                ui,
                                "Add Marker",
                                "A marker over these words, named after them",
                            ) {
                                acts.push(Act::Marker);
                            }
                        }
                        Some(Shown::Media(_)) => {
                            if button(ui, "Delete", "Leave these words out of the edits (Delete)") {
                                acts.push(Act::Delete);
                            }
                            if button(ui, "Undelete", "Bring these words back") {
                                acts.push(Act::Undelete);
                            }
                            ui.separator();
                            for (label, method, tip) in [
                                (
                                    "Place on Top",
                                    "edit.place_on_top",
                                    "On a track above, at the playhead",
                                ),
                                (
                                    "Insert",
                                    "edit.insert",
                                    "At the timeline In, else the playhead",
                                ),
                                ("Append", "edit.append", "After the last clip"),
                            ] {
                                if button(ui, label, &format!("{tip}, without the words left out"))
                                {
                                    acts.push(Act::Edit(method));
                                }
                            }
                        }
                        None => {}
                    }
                    if button(ui, "Copy", "Copy these words as text (Ctrl+C)") {
                        acts.push(Act::Copy);
                    }
                });
                over = ui.ui_contains_pointer();
            });
        if search_go && let Some(items) = &items {
            let needle = t.search.trim().to_lowercase();
            if !needle.is_empty() {
                let from = t.found.map_or(0, |i| i + 1);
                let found = (from..items.len())
                    .chain(0..from)
                    .find(|&i| !items[i].silence && items[i].text.to_lowercase().contains(&needle));
                if let Some(i) = found {
                    t.found = Some(i);
                    t.selection = Some((i, i));
                    t.anchor = Some(i);
                    t.scroll_to = Some((i, true));
                    acts.push(Act::Seek(items[i].start));
                }
            }
        }
        t.open = open;
        t.over = over;
        if let Some(items) = items {
            for act in acts {
                let m = media.as_ref().map(|m| m.id.as_str());
                self.transcription_act(ctx, act, t, &items, m, &left_out);
            }
        } else {
            for act in acts {
                if let Act::Transcribe(ids) = act {
                    self.open_transcribe(ids);
                }
            }
        }
    }

    fn transcription_act(
        &mut self,
        ctx: &egui::Context,
        act: Act,
        t: &mut TranscriptionUi,
        items: &[Item],
        media: Option<&str>,
        left_out: &[(i64, i64)],
    ) {
        let span = |a: i64, b: i64| json!({ "start": format!("{a}f"), "end": format!("{b}f") });
        let selected = t.selected(items);
        match act {
            Act::Transcribe(ids) => self.open_transcribe(ids),
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
            Act::Copy => {
                if let Some((a, b, _, _)) = selected {
                    ctx.copy_text(text_of(&items[a..=b]));
                }
            }
            Act::Delete | Act::Undelete => {
                let Some((_, _, a, b)) = selected else { return };
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
                        self.send(
                            "edit.ripple_delete_words",
                            json!({ "words": [span(a, b)], "quiet": true }),
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
                let parts = uncovered(a, b, left_out);
                if parts.is_empty() {
                    self.say("Every word selected is left out");
                    return;
                }
                self.edit_parts(method, m, &parts);
            }
        }
    }

    fn edit_parts(&mut self, method: &str, media: &str, parts: &[(i64, i64)]) {
        let p = self.project_shared();
        let playhead = self.snap.timeline.position;
        let mut at = match method {
            "edit.insert" => p.as_ref().and_then(|p| p.marks.mark_in).unwrap_or(playhead),
            _ => playhead,
        };
        let target = self
            .target
            .clone()
            .filter(|_| !matches!(method, "edit.append" | "edit.place_on_top"))
            .filter(|t| p.as_ref().is_some_and(|p| p.track(t).is_some()));
        let commands: Vec<Value> = parts
            .iter()
            .map(|&(a, b)| {
                let mut params = json!({ "media": media, "range": format!("{a}f..{b}f") });
                if self.video_only {
                    params["video_only"] = json!(true);
                }
                if self.audio_only {
                    params["audio_only"] = json!(true);
                }
                if let Some(t) = &target {
                    params["track"] = json!(t);
                }
                if method != "edit.append" {
                    params["at"] = json!(format!("{at}f"));
                }
                if method == "edit.place_on_top" {
                    params["use_marks"] = json!(false);
                }
                at += b - a;
                json!({ "method": method, "params": params })
            })
            .collect();
        let label = format!("{} part(s) of the transcript", commands.len());
        self.send("apply", json!({ "commands": commands, "label": label }));
    }
}
