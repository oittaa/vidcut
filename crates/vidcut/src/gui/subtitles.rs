use std::path::PathBuf;

use eframe::egui::{self, Key, RichText};
use serde_json::{Value, json};
use vidcut_core::looks::{LookKind, Styled, SubtitleLook};
use vidcut_core::model::{
    Cue, LetterCase, Place, Project, SpeakerPaint, Speakers, SubtitleStyle, SubtitleTrack,
    SubtitleWords, WordsShow,
};
use vidcut_core::time::{Rational, TimeSpec};

use super::dialog;
use super::export::{Ask, Picked};
use super::folders::Kind;
use super::looks::{Parts, Picker, TextFields};
use super::theme;
use super::title_browser::LeftTab;
use super::workspace::{Workspace, dim};
use crate::live::{Answer, PlayerId};

#[derive(Debug)]
pub struct CueDialog {
    pub cue: String,
    pub text: String,
    pub place: Place,
    speaker: Option<String>,
    color: String,
    person_name: String,
    loaded: bool,
    focus: bool,
    dirty: bool,
    pending: Pending,
    times: Times,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    In,
    Out,
}

#[derive(Debug, Default)]
struct Times {
    typing: Option<(Edge, String)>,
    select: bool,
    held: Option<(i64, i64)>,
}

#[derive(Debug, Default)]
struct Pending {
    sent: Vec<Answer>,
    landed: u64,
}

impl Pending {
    fn waiting(&mut self, rev: u64) -> bool {
        let landed = &mut self.landed;
        self.sent.retain(|a| match a.take() {
            Some(Ok(r)) => {
                *landed = (*landed).max(r.rev);
                false
            }
            Some(Err(_)) => false,
            None => true,
        });
        !self.sent.is_empty() || rev < self.landed
    }
}

#[derive(Debug)]
pub(crate) struct Landing {
    typing: Typing,
    person_name: String,
    pending: Pending,
}

impl Landing {
    pub(crate) fn cue_text(&self) -> (&str, &str) {
        (&self.typing.cue, &self.typing.text)
    }
}

impl CueDialog {
    fn load(&mut self, speakers: &Speakers, c: &Cue) {
        self.text = c.text.clone();
        self.place = c.place;
        self.color = c.color.clone().unwrap_or_default();
        self.take_person(speakers, c.speaker.clone());
        self.loaded = true;
    }

    fn resume(&mut self, l: Landing) {
        self.text = l.typing.text;
        self.place = l.typing.place;
        self.speaker = l.typing.speaker;
        self.color = l.typing.color.unwrap_or_default();
        self.person_name = l.person_name;
        self.pending = l.pending;
    }

    fn take_person(&mut self, speakers: &Speakers, person: Option<String>) {
        self.person_name = person_name(speakers, person.as_deref());
        self.speaker = person;
    }

    fn state(&self) -> (Typing, String) {
        (self.typing(), self.person_name.clone())
    }

    fn typing(&self) -> Typing {
        Typing {
            cue: self.cue.clone(),
            text: self.text.clone(),
            place: self.place,
            speaker: self.speaker.clone(),
            color: field(&self.color),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Typing {
    cue: String,
    text: String,
    place: Place,
    speaker: Option<String>,
    color: Option<String>,
}

fn person_name(speakers: &Speakers, person: Option<&str>) -> String {
    person
        .and_then(|k| speakers.names.get(k).cloned())
        .unwrap_or_default()
}

fn settled(id: &str, speakers: &Speakers, c: &Cue) -> (Typing, String) {
    let color = c.color.clone().unwrap_or_default();
    (
        Typing {
            cue: id.to_string(),
            text: c.text.clone(),
            place: c.place,
            speaker: c.speaker.clone(),
            color: field(&color),
        },
        person_name(speakers, c.speaker.as_deref()),
    )
}

fn cue_copies(dirty: bool, loaded: bool, current: bool) -> bool {
    !loaded || (!dirty && !current)
}

fn cue_saves(dirty: bool, commit: bool) -> bool {
    dirty && commit
}

fn people(p: &Project) -> Vec<(String, String)> {
    let mut keys: Vec<&String> = p
        .subtitles
        .iter()
        .flat_map(|t| t.cues.iter().filter_map(|c| c.speaker.as_ref()))
        .chain(p.speakers.names.keys())
        .chain(p.speakers.colors.keys())
        .collect();
    keys.sort();
    keys.dedup();
    let mut out: Vec<(String, String)> = keys
        .into_iter()
        .map(|k| (k.clone(), p.speakers.name(k)))
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn beside(p: &Project, cue: &str, by: isize) -> Option<(String, i64)> {
    let (t, c) = p.find_cue(cue)?;
    let near = p.subtitles[t].cues.get(c.checked_add_signed(by)?)?;
    Some((near.id.clone(), near.start))
}

fn edge_field(
    ui: &mut egui::Ui,
    t: &mut Times,
    (edge, salt): (Edge, (&str, &str)),
    shown: String,
    width: f32,
) -> Option<String> {
    let mine = matches!(&t.typing, Some((e, _)) if *e == edge);
    let mut text = match &t.typing {
        Some((e, typed)) if *e == edge => typed.clone(),
        _ => shown,
    };
    let out = egui::TextEdit::singleline(&mut text)
        .id_salt(salt)
        .desired_width(width)
        .clip_text(false)
        .show(ui);
    let r = &out.response;
    if r.has_focus() {
        t.select |= !mine;
        if t.select && !ui.input(|i| i.pointer.any_down()) {
            let mut state = out.state;
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            );
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), r.id);
            t.select = false;
        }
        t.typing = Some((edge, text));
        return None;
    }
    if !mine {
        return None;
    }
    let (_, typed) = t.typing.take()?;
    if !r.lost_focus() {
        return None;
    }
    let esc = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
    (!esc).then_some(typed)
}

fn typed_frame(p: &Project, text: &str, base: i64, (playhead, dropped): (i64, i64)) -> Option<i64> {
    let spec = super::viewer::timecode_input(text, dropped)?;
    let sign = match text.trim().chars().next() {
        Some(c @ ('+' | '-')) => c.to_string(),
        _ => String::new(),
    };
    let spec = TimeSpec::parse(&format!("{sign}{spec}")).ok()?;
    p.time_context(Some(playhead))
        .resolve_from(&spec, base)
        .ok()
}

fn command(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn overwrite(cues: &[Cue], id: &str, was: (i64, i64), now: (i64, i64)) -> Vec<Value> {
    let (from, to) = if now.0 < was.0 {
        (now.0, was.0)
    } else if now.1 > was.1 {
        (was.1, now.1)
    } else {
        return Vec::new();
    };
    let set =
        |cue: &str, edge: &str, at: i64| command("subtitles.set", json!({ "cue": cue, edge: at }));
    let mut commands = Vec::new();
    let mut gone = Vec::new();
    for c in cues.iter().filter(|c| c.id != id) {
        let (start, end) = (c.start, c.end());
        if end <= from || start >= to {
            continue;
        }
        match (start < from, end > to) {
            (false, false) => gone.push(c.id.clone()),
            (true, false) => commands.push(set(&c.id, "end", from)),
            (false, true) => commands.push(set(&c.id, "start", to)),
            (true, true) => {}
        }
    }
    if !gone.is_empty() {
        commands.push(command("subtitles.remove", json!({ "cues": gone })));
    }
    commands
}

const LIST_ROWS: f32 = 5.0;
const PAD: f32 = 6.0;
const GAP: f32 = 10.0;

fn shown_text(text: &str) -> String {
    vidcut_core::subtitles::plain(&vidcut_core::subtitles::normalize(text))
}

fn per_second(characters: usize, frames: i64, fps: Rational) -> u64 {
    let read = characters as u64 * fps.num().unsigned_abs();
    let shown = frames.max(1).unsigned_abs() * fps.den().unsigned_abs();
    read.div_ceil(shown)
}

struct Columns {
    number: f32,
    times: f32,
    cps: f32,
}

impl Columns {
    fn text_span(&self, left: f32, right: f32) -> (f32, f32) {
        let x = left + PAD + self.number + GAP + self.times + GAP;
        (x, right - PAD - self.cps - GAP - x)
    }
}

struct ListRow<'a> {
    number: usize,
    times: [String; 2],
    text: &'a str,
    cps: u64,
    on: bool,
}

fn two_lines(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font: egui::FontId,
    color: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, width.max(8.0));
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}

fn list_heading(ui: &mut egui::Ui, width: f32, cols: &Columns, font: &egui::FontId) {
    let height = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let faint = theme::colors().text_dim;
    let painter = ui.painter();
    let at = |text: &str, x: f32, right: bool| {
        let g = painter.layout_no_wrap(text.to_string(), font.clone(), faint);
        let x = if right { x - g.size().x } else { x };
        painter.galley(egui::pos2(x, rect.top()), g, faint);
    };
    let left = rect.left() + PAD;
    at("#", left + cols.number, true);
    at("In/Out", left + cols.number + GAP, false);
    at("Text", cols.text_span(rect.left(), rect.right()).0, false);
    at("CPS", rect.right() - PAD, true);
}

fn cue_row(
    ui: &mut egui::Ui,
    height: f32,
    cols: &Columns,
    row: &ListRow,
    font: &egui::FontId,
) -> bool {
    let (rect, r) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let [start, end] = &row.times;
    r.widget_info(|| {
        let what = format!(
            "{} {start} {end} {}",
            row.number,
            row.text.replace('\n', " / ")
        );
        egui::WidgetInfo::selected(egui::Role::Button, true, row.on, what)
    });
    let c = theme::colors();
    let (ink, faint) = if row.on {
        ui.painter().rect_filled(rect, 3.0, c.selection);
        (c.selected_text, c.selected_text)
    } else {
        if r.hovered() {
            ui.painter()
                .rect_filled(rect, 3.0, c.text.gamma_multiply(0.07));
        }
        (c.text, c.text_dim)
    };
    let (x, width) = cols.text_span(rect.left(), rect.right());
    let shown = two_lines(ui, row.text, width, font.clone(), ink);
    let painter = ui.painter();
    let mid = rect.center().y;
    painter.galley(egui::pos2(x, mid - shown.size().y / 2.0), shown, ink);
    let at_right = |text: String, x: f32| {
        let g = painter.layout_no_wrap(text, font.clone(), faint);
        painter.galley(egui::pos2(x - g.size().x, mid - g.size().y / 2.0), g, faint);
    };
    at_right(row.number.to_string(), rect.left() + PAD + cols.number);
    at_right(row.cps.to_string(), rect.right() - PAD);
    let line = (height - PAD) / 2.0;
    let x = rect.left() + PAD + cols.number + GAP;
    for (i, tc) in row.times.iter().enumerate() {
        let g = painter.layout_no_wrap(tc.clone(), font.clone(), faint);
        let y = rect.top() + PAD / 2.0 + i as f32 * line;
        painter.galley(egui::pos2(x, y), g, faint);
    }
    r.clicked()
}

#[derive(Debug)]
pub struct TrackDialog {
    track: String,
    name: String,
    language: String,
    hint: String,
    fields: TextFields,
    margin: String,
    bold: bool,
    case: Option<LetterCase>,
    show: Option<WordsShow>,
    spoken: String,
    pop: String,
    paint: Option<SpeakerPaint>,
    base: SubtitleStyle,
    dirty: bool,
    from: Option<(Option<String>, Option<String>, SubtitleStyle)>,
    error: String,
    families: Vec<String>,
    preview: Option<SubtitleStyle>,
    picker: Picker,
}

impl TrackDialog {
    fn load(&mut self, s: &SubtitleStyle) {
        self.fields = TextFields::load(&Parts::of_subtitles(s));
        self.margin = percent(s.margin);
        self.bold = s.bold == Some(true);
        self.case = s.case;
        self.show = s.words.as_ref().map(|w| w.show.unwrap_or_default());
        self.spoken = s
            .words
            .as_ref()
            .and_then(|w| w.color.clone())
            .unwrap_or_default();
        self.pop = percent(s.words.as_ref().and_then(|w| w.pop));
        self.paint = s.speaker_color;
        self.base = s.clone();
    }
}

const CASES: [(Option<LetterCase>, &str); 3] = [
    (None, "As typed"),
    (Some(LetterCase::Upper), "Capitals"),
    (Some(LetterCase::Lower), "Small letters"),
];

const SHOWS: [(Option<WordsShow>, &str); 4] = [
    (None, "All at once"),
    (Some(WordsShow::Highlight), "Highlight the spoken word"),
    (Some(WordsShow::Reveal), "Reveal words as spoken"),
    (Some(WordsShow::Word), "Only the spoken word"),
];

const PAINTS: [(Option<SpeakerPaint>, &str); 4] = [
    (None, "Usual"),
    (Some(SpeakerPaint::Text), "Color the text"),
    (Some(SpeakerPaint::Spoken), "Color the spoken word"),
    (Some(SpeakerPaint::None), "No colors"),
];

pub(crate) type SubtitlesKey = (u64, Option<String>, Vec<Typing>, Option<SubtitleStyle>);

pub(super) fn format_name(f: vidcut_core::request::SubtitleFormat) -> &'static str {
    match f {
        vidcut_core::request::SubtitleFormat::Srt => "SRT",
        vidcut_core::request::SubtitleFormat::Vtt => "WebVTT",
        vidcut_core::request::SubtitleFormat::Ttml => "TTML",
    }
}

pub(super) enum Files {
    PickImport(Picked),
    Importing(Ask, PathBuf),
    Added(String, String),
    PickExport(Picked, String),
    Exporting(Ask),
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PickImport(_) => "PickImport",
            Self::Importing(..) => "Importing",
            Self::Added(..) => "Added",
            Self::PickExport(..) => "PickExport",
            Self::Exporting(_) => "Exporting",
        })
    }
}

const PLACES: [(Place, &str); 3] = [
    (Place::Bottom, "Bottom"),
    (Place::Middle, "Middle"),
    (Place::Top, "Top"),
];

pub(super) fn percent(v: Option<f64>) -> String {
    let Some(n) = v else {
        return String::new();
    };
    let p = n * 100.0;
    let s = format!("{p:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn number(raw: &str, what: &str) -> Result<Option<f64>, String> {
    let t = raw.trim().trim_end_matches('%').trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>()
        .map(|n| Some(n / 100.0))
        .map_err(|_| format!("{what} is a percent, such as 5.5"))
}

pub(super) fn field(raw: &str) -> Option<String> {
    let t = raw.trim();
    (!t.is_empty()).then(|| t.to_string())
}

pub(super) fn kept_number(
    shown: &str,
    stored: Option<f64>,
    what: &str,
) -> Result<Option<f64>, String> {
    if shown.trim() == percent(stored) {
        Ok(stored)
    } else {
        number(shown, what)
    }
}

fn style_of(d: &TrackDialog) -> Result<SubtitleStyle, String> {
    let stored = &d.base;
    let parts = d.fields.parts(&Parts::of_subtitles(stored))?;
    let words = match d.show {
        Some(show) => Some(SubtitleWords {
            show: Some(show),
            color: field(&d.spoken),
            pop: kept_number(&d.pop, stored.words.as_ref().and_then(|w| w.pop), "pop")?,
        }),
        None => None,
    };
    let style = SubtitleStyle {
        font: parts.font,
        size: parts.size,
        color: parts.color,
        outline: parts.outline,
        background: parts.background,
        margin: kept_number(&d.margin, stored.margin, "margin")?,
        bold: d.bold.then_some(true),
        case: d.case,
        shadow: parts.shadow,
        words,
        speaker_color: d.paint,
    };
    vidcut_core::ops::subtitles::checked_style(&style).map_err(|e| e.message)
}

pub(super) fn swatch(ui: &mut egui::Ui, text: &mut String, usual: [u8; 3]) -> bool {
    let mut rgb = vidcut_core::subtitles::color(text)
        .and_then(|c| vidcut_core::subtitles::rgb(&c))
        .unwrap_or(usual);
    let changed = ui.color_edit_button_srgb(&mut rgb).changed();
    if changed {
        let [r, g, b] = rgb;
        *text = format!("#{r:02x}{g:02x}{b:02x}");
    }
    changed
}

pub(super) fn font_list(
    ui: &mut egui::Ui,
    salt: &str,
    font: &mut String,
    families: &[String],
) -> bool {
    let mut changed = false;
    if families.is_empty() {
        return changed;
    }
    egui::ComboBox::from_id_salt(salt)
        .selected_text("")
        .width(0.0)
        .show_ui(ui, |ui| {
            for name in families {
                if ui
                    .selectable_label(font.trim().eq_ignore_ascii_case(name), name)
                    .clicked()
                {
                    *font = name.clone();
                    changed = true;
                }
            }
        });
    changed
}

impl Workspace {
    fn typed(&self, d: &CueDialog) -> Option<serde_json::Value> {
        let cue = self.project()?.cue(&d.cue)?;
        let mut params = json!({ "cue": d.cue });
        if vidcut_core::subtitles::normalize(&d.text) != cue.text {
            params["text"] = json!(d.text);
        }
        if d.place != cue.place {
            params["place"] = json!(d.place);
        }
        if d.speaker != cue.speaker {
            params["speaker"] = json!(d.speaker.clone().unwrap_or_default());
        }
        let own = field(&d.color);
        if own != cue.color {
            params["color"] = json!(own.unwrap_or_default());
        }
        params
            .as_object()
            .is_some_and(|o| o.len() > 1)
            .then_some(params)
    }

    fn save_cue(&mut self, d: &CueDialog) -> Vec<Answer> {
        let mut sent = Vec::new();
        if let Some(params) = self.typed(d) {
            sent.push(self.live.request_answered("subtitles.set", params));
        }
        let Some(person) = d.speaker.clone() else {
            return sent;
        };
        let stored = self
            .project()
            .map(|p| person_name(&p.speakers, Some(&person)));
        let name = d.person_name.trim();
        if stored.is_some_and(|s| s != name) {
            sent.push(
                self.live
                    .request_answered("speakers.set", json!({ "person": person, "name": name })),
            );
        }
        sent
    }

    pub(crate) fn open_cue(&mut self, id: &str) {
        self.left = LeftTab::Inspector;
        self.inspect_cue(id, true);
    }

    pub(super) fn close_cue_form(&mut self) {
        let Some(mut d) = self.timeline.cue_dialog.take() else {
            return;
        };
        if d.dirty {
            let sent = self.save_cue(&d);
            d.pending.sent.extend(sent);
        }
        let rev = self.project().map_or(0, |p| p.rev);
        if d.pending.waiting(rev) {
            self.timeline.cue_landings.push(Landing {
                typing: d.typing(),
                person_name: d.person_name,
                pending: d.pending,
            });
        }
    }

    pub(super) fn close_track_form(&mut self) {
        if let Some(d) = self.timeline.track_dialog.take().filter(|d| d.dirty)
            && let Ok(Some(params)) = self.track_params(&d)
        {
            self.send("track.set", params);
        }
    }

    fn track_params(&self, d: &TrackDialog) -> Result<Option<Value>, String> {
        let p = self.project().ok_or_else(String::new)?;
        let t = p.subtitle_track(&d.track).ok_or_else(String::new)?;
        let mut params = json!({ "track": d.track });
        if d.name.trim() != t.name.as_deref().unwrap_or_default() {
            params["name"] = json!(d.name.trim());
        }
        if d.language.trim() != t.language.as_deref().unwrap_or_default() {
            params["language"] = json!(d.language.trim());
        }
        let style = style_of(d)?;
        if style != t.style {
            params["style"] = serde_json::to_value(&style).unwrap_or(json!({}));
        }
        Ok(params
            .as_object()
            .is_some_and(|o| o.len() > 1)
            .then_some(params))
    }

    pub(super) fn inspect_cue(&mut self, id: &str, focus: bool) {
        self.close_title_form();
        self.close_track_form();
        self.inspector_saw_selection();
        if let Some(d) = &mut self.timeline.cue_dialog
            && d.cue == id
        {
            d.focus |= focus;
            return;
        }
        self.close_cue_form();
        let landing = self
            .timeline
            .cue_landings
            .iter()
            .position(|l| l.typing.cue == id)
            .map(|i| self.timeline.cue_landings.remove(i));
        let mut d = CueDialog {
            cue: id.to_string(),
            text: String::new(),
            place: Place::default(),
            speaker: None,
            color: String::new(),
            person_name: String::new(),
            loaded: false,
            focus,
            dirty: false,
            pending: Pending::default(),
            times: Times::default(),
        };
        if let Some(p) = self.project()
            && let Some(c) = p.cue(id)
        {
            d.load(&p.speakers, c);
        }
        if let Some(l) = landing {
            d.resume(l);
        }
        self.timeline.cue_dialog = Some(d);
    }

    pub(crate) fn open_track_dialog(&mut self, t: &SubtitleTrack) {
        self.left = LeftTab::Inspector;
        self.close_forms();
        self.inspector_saw_selection();
        self.looks.reread();
        let styled = Styled::Subtitles(SubtitleLook {
            style: t.style.clone(),
            ..SubtitleLook::default()
        });
        let picker = Picker::new(
            LookKind::Subtitles,
            self.looks.of(LookKind::Subtitles),
            &styled,
        );
        let mut d = TrackDialog {
            track: t.id.clone(),
            name: t.name.clone().unwrap_or_default(),
            language: t.language.clone().unwrap_or_default(),
            hint: t.display_name(),
            fields: TextFields::default(),
            margin: String::new(),
            bold: false,
            case: None,
            show: None,
            spoken: String::new(),
            pop: String::new(),
            paint: None,
            base: SubtitleStyle::default(),
            dirty: false,
            from: None,
            error: String::new(),
            families: vidcut_av::burn::families(),
            preview: None,
            picker,
        };
        d.load(&t.style);
        self.timeline.track_dialog = Some(d);
    }

    pub(super) fn sync_subtitles(&mut self) {
        let Some(p) = self.project() else {
            return;
        };
        let track = p.subtitles.iter().find(|t| t.shown);
        let typing: Vec<Typing> = self
            .timeline
            .cue_landings
            .iter()
            .map(|l| l.typing.clone())
            .chain(self.timeline.cue_dialog.as_ref().map(CueDialog::typing))
            .collect();
        let styling = self
            .timeline
            .track_dialog
            .as_ref()
            .filter(|d| track.is_some_and(|t| t.id == d.track))
            .and_then(|d| d.preview.clone());
        let key = (
            p.rev,
            track.map(|t| t.id.clone()),
            typing.clone(),
            styling.clone(),
        );
        if self.timeline.subtitles_sent.as_ref() == Some(&key) {
            return;
        }
        let view = track.map(|t| {
            let mut cues = t.cues.clone();
            for ty in &typing {
                let Some(c) = cues.iter_mut().find(|c| c.id == ty.cue) else {
                    continue;
                };
                let text = vidcut_core::subtitles::normalize(&ty.text);
                if !c.words.is_empty()
                    && vidcut_core::subtitles::words(&text).len() != c.words.len()
                {
                    c.words = vidcut_core::subtitles::spread_words(&text, c.duration);
                }
                c.text = text;
                c.place = ty.place;
                c.speaker = ty.speaker.clone();
                c.color = ty.color.clone();
            }
            for c in &mut cues {
                c.color = p.speakers.color_of(c);
            }
            vidcut_av::player::SubtitleView {
                style: styling.unwrap_or_else(|| t.style.clone()),
                cues,
            }
        });
        self.timeline.subtitles_sent = Some(key);
        self.live.show_subtitles(view);
    }

    pub(super) fn subtitle_windows(&mut self, ctx: &egui::Context) {
        let rev = self.project().map_or(0, |p| p.rev);
        self.timeline
            .cue_landings
            .retain_mut(|l| l.pending.waiting(rev));
        if !self.timeline.cue_landings.is_empty() {
            ctx.request_repaint();
        }
        if let Some(result) = self.timeline.adding.as_ref().and_then(Ask::take) {
            self.timeline.adding = None;
            match result {
                Ok(r) => {
                    if let Some(id) = r.result["cue"].as_str() {
                        let id = id.to_string();
                        self.open_cue(&id);
                    }
                }
                Err(e) => self.say(format!("Add Subtitle: {}", e.message)),
            }
        }
        self.subtitle_files(ctx);
        self.retime_window(ctx);
    }

    pub(super) fn cue_form(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(mut d) = self.timeline.cue_dialog.take() else {
            self.timeline.cue_listed = None;
            return false;
        };
        let Some(p) = self.project_shared() else {
            return false;
        };
        let waiting = d.pending.waiting(p.rev);
        if waiting {
            ui.ctx().request_repaint();
        }
        let at = p.find_cue(&d.cue);
        let found = at.map(|(t, c)| (&p.subtitles[t], &p.subtitles[t].cues[c]));
        let shown = found.is_some_and(|(_, cue)| settled(&d.cue, &p.speakers, cue) == d.state());
        match found {
            Some((_, cue)) if cue_copies(d.dirty, d.loaded, waiting || shown) => {
                d.load(&p.speakers, cue);
                d.dirty = false;
            }
            None if d.loaded => return false,
            _ => {}
        }
        let everyone = people(&p);
        let mut done = false;
        let mut step = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Subtitle").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (label, by) in [("Next", 1), ("Previous", -1)] {
                    let to = beside(&p, &d.cue, by);
                    if ui
                        .add_enabled(to.is_some(), egui::Button::new(label))
                        .clicked()
                    {
                        step = to;
                    }
                }
            });
        });
        match found {
            Some((_, cue)) => self.cue_times(ui, &mut d, &p, cue, waiting),
            None => {
                ui.label(dim("Adding…"));
            }
        }
        let before = d.state();
        let r = ui.add(
            egui::TextEdit::multiline(&mut d.text)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Its text; Enter starts a new line"),
        );
        if d.focus {
            r.request_focus();
            d.focus = false;
        }
        dialog::note(
            ui,
            "<i>, <b>, <u> and <font color=\"#ffcc00\"> around words style them",
        );
        let style = found.map(|(t, _)| t.style.clone()).unwrap_or_default();
        dialog::form(ui, "cue-form", |ui| {
            dialog::row(ui, "Speaker", |ui| {
                let name = d
                    .speaker
                    .as_deref()
                    .map_or("None".into(), |k| p.speakers.name(k));
                egui::ComboBox::from_id_salt("cue-speaker")
                    .selected_text(name)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        let mut picked = None;
                        ui.label(dim("This Subtitle"));
                        if ui.selectable_label(d.speaker.is_none(), "None").clicked() {
                            picked = Some(None);
                        }
                        for (key, name) in &everyone {
                            let on = d.speaker.as_deref() == Some(key.as_str());
                            if ui.selectable_label(on, name).clicked() {
                                picked = Some(Some(key.clone()));
                            }
                        }
                        let stored = found.and_then(|(_, c)| c.speaker.clone());
                        let others: Vec<_> = everyone
                            .iter()
                            .filter(|(k, _)| Some(k) != stored.as_ref())
                            .collect();
                        if let Some(from) = stored.as_ref().filter(|_| !others.is_empty()) {
                            ui.separator();
                            ui.label(dim(format!("All of {}'s", p.speakers.name(from))));
                            for (key, name) in others {
                                if ui.selectable_label(false, name).clicked() {
                                    self.send(
                                        "speakers.join",
                                        json!({ "person": from, "into": key }),
                                    );
                                    picked = Some(Some(key.clone()));
                                }
                            }
                        }
                        if let Some(k) = picked {
                            d.take_person(&p.speakers, k);
                        }
                    });
            });
            if let Some(k) = &d.speaker {
                dialog::row(ui, "Name", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut d.person_name)
                            .hint_text(vidcut_core::speakers::Speakers::default().name(k))
                            .desired_width(200.0),
                    );
                });
            }
            dialog::row(ui, "Color", |ui| {
                ui.add_enabled_ui(style.paints() != SpeakerPaint::None, |ui| {
                    let track = match (style.paints(), &style.words) {
                        (SpeakerPaint::Spoken, Some(w)) => w
                            .color
                            .as_deref()
                            .and_then(vidcut_core::subtitles::rgb)
                            .unwrap_or([255, 212, 0]),
                        _ => style.ink(),
                    };
                    let usual = d
                        .speaker
                        .as_ref()
                        .and_then(|k| p.speakers.colors.get(k))
                        .and_then(|c| vidcut_core::subtitles::rgb(c))
                        .unwrap_or(track);
                    swatch(ui, &mut d.color, usual);
                    if ui
                        .add_enabled(!d.color.is_empty(), egui::Button::new("Clear"))
                        .clicked()
                    {
                        d.color.clear();
                    }
                });
                if style.paints() == SpeakerPaint::None {
                    dialog::why(ui, "off in the track's Speakers setting");
                }
            });
            dialog::row(ui, "Position", |ui| {
                dialog::switch(ui, |ui| {
                    for (place, label) in PLACES {
                        ui.selectable_value(&mut d.place, place, label);
                    }
                });
            });
        });
        dialog::footer(ui, |ui| {
            if ui
                .add_enabled(found.is_some(), egui::Button::new("Delete"))
                .clicked()
            {
                self.send("subtitles.remove", json!({ "cues": [d.cue] }));
                done = true;
            }
            if let Some(k) = d.speaker.clone().filter(|_| found.is_some()) {
                let label = format!("Apply to All of {}'s", p.speakers.name(&k));
                if ui.button(label).clicked() {
                    let sent = self.save_cue(&d);
                    d.pending.sent.extend(sent);
                    d.pending.sent.push(
                        self.live
                            .request_answered("subtitles.match_speaker", json!({ "cue": d.cue })),
                    );
                    d.color.clear();
                }
            }
        });
        if d.state() != before {
            d.dirty = true;
        }
        if !done && cue_saves(d.dirty, self.inspector_commits()) {
            let sent = self.save_cue(&d);
            d.pending.sent.extend(sent);
            d.dirty = false;
        }
        if let Some(at) = at {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let track = &p.subtitles[at.0];
                let free = self.timeline.adding.is_none();
                if ui.add_enabled(free, egui::Button::new("Add New")).clicked() {
                    self.add_subtitle(ui.ctx(), Some(&track.id));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(dim(track.display_name()));
                });
            });
            let follow = self.timeline.cue_listed.as_deref() != Some(d.cue.as_str());
            if let Some(to) = self.cue_list(ui, &p, at, &d.text, follow) {
                step = Some(to);
            }
            self.timeline.cue_listed = Some(d.cue.clone());
        }
        ui.separator();
        if !done {
            self.timeline.cue_dialog = Some(d);
        }
        if let Some((id, start)) = step {
            self.step_cue(&id, start);
        }
        true
    }

    fn step_cue(&mut self, id: &str, start: i64) {
        self.inspect_cue(id, false);
        self.send("selection.set", json!({ "cues": [id], "clips": [] }));
        self.seek(PlayerId::Timeline, start, false);
    }

    fn cue_times(
        &self,
        ui: &mut egui::Ui,
        d: &mut CueDialog,
        p: &Project,
        cue: &Cue,
        waiting: bool,
    ) {
        if !waiting {
            d.times.held = None;
        }
        let (start, end) = d.times.held.unwrap_or((cue.start, cue.end()));
        let font = egui::TextStyle::Body.resolve(ui.style());
        let faint = theme::colors().text_dim;
        let wide = |text: String| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), faint)
                .size()
                .x
        };
        let width = wide(self.tc(end)) + 16.0;
        let count = shown_text(&d.text).chars().count();
        let counted = match count {
            1 => "1 Character".to_string(),
            n => format!("{n} Characters"),
        };
        let counted_width = wide(counted.clone());
        let mut put = None;
        ui.horizontal_wrapped(|ui| {
            for (edge, label, at) in [(Edge::In, "In", start), (Edge::Out, "Out", end)] {
                ui.label(dim(label));
                if let Some(text) = edge_field(
                    ui,
                    &mut d.times,
                    (edge, (label, &d.cue)),
                    self.tc(at),
                    width,
                ) {
                    put = Some((edge, text));
                }
            }
            let length = format!("Duration {}", self.src_tc(end - start));
            ui.add(egui::Label::new(dim(length)).extend());
            if ui.available_width() < counted_width {
                ui.end_row();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(dim(counted));
            });
        });
        if let Some((edge, text)) = put {
            self.put_edge(d, p, (edge, &text), (start, end));
        }
    }

    fn put_edge(
        &self,
        d: &mut CueDialog,
        p: &Project,
        (edge, text): (Edge, &str),
        was: (i64, i64),
    ) {
        let Some((t, _)) = p.find_cue(&d.cue) else {
            return;
        };
        let (base, key) = match edge {
            Edge::In => (was.0, "start"),
            Edge::Out => (was.1, "end"),
        };
        let context = (self.snap.timeline.position, self.dropped());
        let Some(at) = typed_frame(p, text, base, context) else {
            return;
        };
        let now = match edge {
            Edge::In => (at, was.1),
            Edge::Out => (was.0, at),
        };
        if now == was || now.0 < 0 || now.1 <= now.0 {
            return;
        }
        let params = json!({ "cue": d.cue, key: at });
        let label = vidcut_core::describe::describe("subtitles.set", &params, Some(p));
        let mut commands = vec![command("subtitles.set", params)];
        commands.extend(overwrite(&p.subtitles[t].cues, &d.cue, was, now));
        let batch = json!({ "commands": commands, "label": label });
        d.pending
            .sent
            .push(self.live.request_answered("apply", batch));
        d.times.held = Some(now);
    }

    fn cue_list(
        &self,
        ui: &mut egui::Ui,
        p: &Project,
        (track, open): (usize, usize),
        typed: &str,
        follow: bool,
    ) -> Option<(String, i64)> {
        let cues = &p.subtitles[track].cues;
        let landed: Vec<(&str, &str)> = self
            .timeline
            .cue_landings
            .iter()
            .map(Landing::cue_text)
            .collect();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let row = 2.0 * ui.text_style_height(&egui::TextStyle::Body) + PAD;
        let gap = ui.spacing().item_spacing.y;
        let stride = row + gap;
        let height = LIST_ROWS * stride - gap;
        let wide = |text: String| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), egui::Color32::WHITE)
                .size()
                .x
        };
        let cols = Columns {
            number: wide(cues.len().to_string()).max(wide("#".into())),
            times: cues
                .last()
                .map_or(0.0, |c| wide(self.tc(c.end())))
                .max(wide("In/Out".into())),
            cps: wide("000".into()).max(wide("CPS".into())),
        };
        let fps = p.settings.fps;
        let mut picked = None;
        let bar = if cues.len() as f32 * stride - gap > height {
            ui.spacing().scroll.allocated_width()
        } else {
            0.0
        };
        list_heading(ui, ui.available_width() - bar, &cols, &font);
        egui::ScrollArea::vertical()
            .id_salt("cue-list")
            .auto_shrink([false, false])
            .max_height(height)
            .show_rows(ui, row, cues.len(), |ui, range| {
                let top = ui.max_rect().top() - range.start as f32 * stride;
                for k in range {
                    let c = &cues[k];
                    let text = if k == open {
                        typed
                    } else {
                        landed
                            .iter()
                            .find(|(id, _)| *id == c.id)
                            .map_or(c.text.as_str(), |(_, t)| *t)
                    };
                    let shown = shown_text(text);
                    let line = ListRow {
                        number: k + 1,
                        times: [self.tc(c.start), self.tc(c.end())],
                        text: &shown,
                        cps: per_second(shown.chars().count(), c.duration, fps),
                        on: k == open,
                    };
                    if cue_row(ui, row, &cols, &line, &font) {
                        picked = Some((c.id.clone(), c.start));
                    }
                }
                let y = top + open as f32 * stride;
                let clip = ui.clip_rect();
                if follow && (y < clip.top() || y + row > clip.bottom()) {
                    let rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), y..=y + row);
                    ui.scroll_to_rect(rect, Some(egui::Align::Center));
                }
            });
        picked
    }

    pub(super) fn track_form(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(mut d) = self.timeline.track_dialog.take() else {
            return false;
        };
        let Some(t) = self
            .project()
            .and_then(|p| p.subtitle_track(&d.track))
            .cloned()
        else {
            return false;
        };
        let now = (t.name.clone(), t.language.clone(), t.style);
        if !d.dirty && d.from.as_ref() != Some(&now) {
            d.name = now.0.clone().unwrap_or_default();
            d.language = now.1.clone().unwrap_or_default();
            d.load(&now.2);
            d.preview = None;
            d.from = Some(now);
        }
        ui.label(RichText::new("Subtitle Track").strong());
        let mut enter = false;
        egui::Grid::new("subtitle-track-fields").show(ui, |ui| {
            ui.label("Name");
            let r = ui.add(
                egui::TextEdit::singleline(&mut d.name)
                    .hint_text(&d.hint)
                    .min_size(egui::vec2(240.0, 0.0)),
            );
            d.dirty |= r.changed();
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.end_row();
            ui.label("Language");
            let r = ui.add(
                egui::TextEdit::singleline(&mut d.language)
                    .hint_text("en, pt-BR, …")
                    .min_size(egui::vec2(240.0, 0.0)),
            );
            d.dirty |= r.changed();
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.end_row();
        });
        ui.label(dim("Exported subtitle files are named with these"));
        ui.add_space(8.0);
        let mut changed = false;
        let current = style_of(&d).map(|style| {
            Styled::Subtitles(SubtitleLook {
                style,
                ..SubtitleLook::default()
            })
        });
        if let Some(Styled::Subtitles(l)) = self.look_picker(ui, &mut d.picker, current) {
            d.load(&l.style);
            changed = true;
        }
        egui::Grid::new("subtitle-style-fields").show(ui, |ui| {
            let (c, e) = d
                .fields
                .rows(ui, "subtitle", &d.families, "5.5% of the picture");
            changed |= c;
            enter |= e;
            let mut row = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str| {
                ui.label(label);
                let r = ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .min_size(egui::vec2(240.0, 0.0)),
                );
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                r.changed()
            };
            changed |= row(ui, "Margin", &mut d.margin, "none: centered at 80% or 20%");
            ui.end_row();
            ui.label("Letters");
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut d.bold, "Bold").changed();
                for (case, label) in CASES {
                    changed |= ui.radio_value(&mut d.case, case, label).changed();
                }
            });
            ui.end_row();
            ui.label("Words");
            egui::ComboBox::from_id_salt("subtitle-words")
                .selected_text(
                    SHOWS
                        .iter()
                        .find(|(s, _)| *s == d.show)
                        .map_or("", |(_, l)| *l),
                )
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (show, label) in SHOWS {
                        changed |= ui.selectable_value(&mut d.show, show, label).changed();
                    }
                });
            ui.end_row();
            if d.show.is_some() {
                let usual = match d.show {
                    Some(WordsShow::Highlight) => "#ffd400",
                    _ => "the text's color",
                };
                changed |= row(ui, "Spoken word", &mut d.spoken, usual);
                changed |= swatch(ui, &mut d.spoken, [255, 212, 0]);
                ui.end_row();
                changed |= row(ui, "Pop", &mut d.pop, "100%: none, up to 200%");
                ui.end_row();
            }
            ui.label("Speakers");
            let usual = SubtitleStyle {
                words: d.show.map(|show| SubtitleWords {
                    show: Some(show),
                    ..Default::default()
                }),
                speaker_color: None,
                ..Default::default()
            }
            .paints();
            let paint_label = |paint: Option<SpeakerPaint>| {
                let named = |p: Option<SpeakerPaint>| {
                    PAINTS.iter().find(|(s, _)| *s == p).map_or("", |(_, l)| *l)
                };
                match paint {
                    None => format!("{} ({})", named(None), named(Some(usual))),
                    p => named(p).to_string(),
                }
            };
            egui::ComboBox::from_id_salt("subtitle-speakers")
                .selected_text(paint_label(d.paint))
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (paint, _) in PAINTS {
                        changed |= ui
                            .selectable_value(&mut d.paint, paint, paint_label(paint))
                            .changed();
                    }
                });
            ui.end_row();
        });
        if changed {
            d.preview = style_of(&d).ok();
        }
        ui.label(dim("An empty field keeps the usual look."));
        if !d.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &d.error);
        }
        d.dirty |= changed;
        if d.dirty && (enter || self.inspector_commits()) {
            match self.track_params(&d) {
                Ok(params) => {
                    if let Some(params) = params {
                        self.send("track.set", params);
                    }
                    d.dirty = false;
                    d.error.clear();
                }
                Err(e) => d.error = e,
            }
        }
        ui.separator();
        self.timeline.track_dialog = Some(d);
        true
    }

    pub(super) fn import_subtitle(&mut self, ctx: &egui::Context) {
        let dialogs = self.dialogs.clone();
        let folder = self
            .folders
            .get(Kind::Media)
            .or_else(|| self.project_folder())
            .unwrap_or_else(|| PathBuf::from("."));
        self.timeline.files = Some(Files::PickImport(Picked::ask(ctx, move || {
            dialogs.pick_file(
                "Import Subtitle",
                &folder,
                &["srt", "vtt", "ttml", "dfxp", "xml"],
            )
        })));
    }

    pub(crate) fn export_subtitle(&mut self, ctx: &egui::Context, track: Option<&str>) {
        let Some(p) = self.project() else {
            return;
        };
        let ti = match track {
            Some(t) => p.subtitle_index(t),
            None => vidcut_core::ops::subtitles::shown(p).ok(),
        };
        let Some(t) = ti.map(|i| &p.subtitles[i]) else {
            self.say("Show a subtitle track first, or export from its header's menu");
            return;
        };
        let stem = self
            .snap
            .project
            .as_ref()
            .and_then(|s| s.path.file_stem())
            .map_or("subtitles".into(), |s| s.to_string_lossy().into_owned());
        let file = format!("{stem}_{}.srt", t.display_name());
        let id = t.id.clone();
        let dialogs = self.dialogs.clone();
        let folder = self
            .folders
            .get(Kind::Exports)
            .or_else(|| self.project_folder())
            .unwrap_or_else(|| PathBuf::from("."));
        self.timeline.files = Some(Files::PickExport(
            Picked::ask(ctx, move || {
                dialogs.save_file("Export Subtitle", &folder, &file, &["srt", "vtt"])
            }),
            id,
        ));
    }

    fn subtitle_files(&mut self, ctx: &egui::Context) {
        let Some(step) = self.timeline.files.take() else {
            return;
        };
        self.timeline.files = match step {
            Files::PickImport(picked) => match picked.take() {
                None => Some(Files::PickImport(picked)),
                Some(None) => None,
                Some(Some(path)) => {
                    self.folders.remember(Kind::Media, &path);
                    let (bin, _) = self.import_from();
                    let params = json!({ "paths": [path], "bin": bin });
                    Some(Files::Importing(
                        Ask::send(&self.live, ctx, "media.add", params),
                        path,
                    ))
                }
            },
            Files::Importing(ask, path) => match ask.take() {
                None => Some(Files::Importing(ask, path)),
                Some(Ok(r)) => {
                    let name = path
                        .file_name()
                        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    let id = r.result["media"][0]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    Some(Files::Added(id, name))
                }
                Some(Err(e)) => {
                    self.say(format!("Import Subtitle: {}", e.message));
                    None
                }
            },
            Files::Added(id, name) => {
                let Some(s) = self
                    .project()
                    .and_then(|p| p.media(&id))
                    .and_then(|m| m.subtitles.clone())
                else {
                    self.timeline.files = Some(Files::Added(id, name));
                    return;
                };
                self.say(format!(
                    "Imported {name}, {} subtitle(s). Drag it onto a subtitle track.",
                    s.cues
                ));
                if !s.warnings.is_empty() {
                    self.show_import_warnings(name, s.warnings);
                }
                None
            }
            Files::PickExport(picked, track) => match picked.take() {
                None => Some(Files::PickExport(picked, track)),
                Some(None) => None,
                Some(Some(mut path)) => {
                    if path.extension().is_none() {
                        path.set_extension("srt");
                    }
                    self.folders.remember(Kind::Exports, &path);
                    Some(Files::Exporting(Ask::send(
                        &self.live,
                        ctx,
                        "subtitles.export",
                        json!({ "out": path, "track": track, "overwrite": true }),
                    )))
                }
            },
            Files::Exporting(ask) => match ask.take() {
                None => Some(Files::Exporting(ask)),
                Some(Ok(r)) => {
                    let mut said = format!(
                        "Wrote {} ({} subtitles)",
                        r.result["path"].as_str().unwrap_or_default(),
                        r.result["cues"]
                    );
                    for w in r.result["warnings"].as_array().into_iter().flatten() {
                        said.push_str(&format!("; {}", w.as_str().unwrap_or_default()));
                    }
                    self.say(said);
                    None
                }
                Some(Err(e)) => {
                    self.say(format!("Export Subtitle: {}", e.message));
                    None
                }
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::cue_copies;
    use super::cue_saves;

    #[test]
    fn a_saved_subtitle_stays_while_the_project_is_still_the_old_text() {
        assert!(!cue_copies(false, true, true));
    }

    #[test]
    fn a_clean_subtitle_copies_the_project_when_it_changes() {
        assert!(cue_copies(false, true, false));
    }

    #[test]
    fn an_edit_in_progress_is_not_replaced_by_the_project() {
        assert!(!cue_copies(true, true, false));
    }

    #[test]
    fn an_edit_is_saved_only_when_the_form_commits() {
        assert!(cue_saves(true, true));
        assert!(!cue_saves(true, false));
        assert!(!cue_saves(false, true));
    }
}
