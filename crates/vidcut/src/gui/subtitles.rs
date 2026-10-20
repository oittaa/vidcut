use std::path::PathBuf;

use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::looks::{LookKind, Styled, SubtitleLook};
use vidcut_core::model::{
    Cue, LetterCase, Place, Project, SpeakerPaint, Speakers, SubtitleStyle, SubtitleTrack,
    SubtitleWords, WordsShow,
};

use super::dialog;
use super::export::{Ask, Picked};
use super::folders::Kind;
use super::looks::{Parts, Picker, TextFields};
use super::workspace::{Workspace, dim};

#[derive(Debug, Clone)]
pub struct CueDialog {
    pub cue: String,
    pub text: String,
    pub place: Place,
    speaker: Option<String>,
    color: String,
    person_name: String,
    loaded: bool,
    focus: bool,
}

impl CueDialog {
    fn load(&mut self, speakers: &Speakers, c: &Cue) {
        self.text = c.text.clone();
        self.place = c.place;
        self.color = c.color.clone().unwrap_or_default();
        self.take_person(speakers, c.speaker.clone());
        self.loaded = true;
    }

    fn take_person(&mut self, speakers: &Speakers, person: Option<String>) {
        self.person_name = person_name(speakers, person.as_deref());
        self.speaker = person;
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

pub(crate) type SubtitlesKey = (u64, Option<String>, Option<Typing>, Option<SubtitleStyle>);

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

    fn save_cue(&mut self, d: &CueDialog) {
        if let Some(params) = self.typed(d) {
            self.send("subtitles.set", params);
        }
        let Some(person) = d.speaker.clone() else {
            return;
        };
        let stored = self
            .project()
            .map(|p| person_name(&p.speakers, Some(&person)));
        let name = d.person_name.trim();
        if stored.is_some_and(|s| s != name) {
            self.send("speakers.set", json!({ "person": person, "name": name }));
        }
    }

    pub(crate) fn open_cue(&mut self, id: &str) {
        if let Some(d) = &mut self.timeline.cue_dialog
            && d.cue == id
        {
            d.focus = true;
            return;
        }
        if let Some(d) = self.timeline.cue_dialog.clone() {
            self.save_cue(&d);
        }
        let mut d = CueDialog {
            cue: id.to_string(),
            text: String::new(),
            place: Place::default(),
            speaker: None,
            color: String::new(),
            person_name: String::new(),
            loaded: false,
            focus: true,
        };
        if let Some(p) = self.project()
            && let Some(c) = p.cue(id)
        {
            d.load(&p.speakers, c);
        }
        self.timeline.cue_dialog = Some(d);
    }

    pub(crate) fn open_track_dialog(&mut self, t: &SubtitleTrack) {
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
        let typing = self.timeline.cue_dialog.as_ref().map(CueDialog::typing);
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
            if let Some(ty) = &typing
                && let Some(c) = cues.iter_mut().find(|c| c.id == ty.cue)
            {
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
        self.cue_window(ctx);
        self.track_window(ctx);
        self.subtitle_files(ctx);
        self.retime_window(ctx);
    }

    fn cue_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.cue_dialog.take() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let found = p
            .find_cue(&d.cue)
            .map(|(t, c)| (&p.subtitles[t], &p.subtitles[t].cues[c]));
        match found {
            Some((_, cue)) if !d.loaded => d.load(&p.speakers, cue),
            None if d.loaded => return,
            _ => {}
        }
        let everyone = people(&p);
        let mut open = true;
        let mut done = false;
        dialog::window(ctx, "Subtitle", &mut open, |ui| {
            match found {
                Some((track, cue)) => ui.label(dim(format!(
                    "{}   {} – {}   ({})",
                    track.display_name(),
                    self.tc(cue.start),
                    self.tc(cue.end()),
                    self.src_tc(cue.duration)
                ))),
                None => ui.label(dim("Adding…")),
            };
            let save = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
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
                if dialog::main_button(ui, "Done", true) || save {
                    self.save_cue(&d);
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
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
                        self.save_cue(&d);
                        self.send("subtitles.match_speaker", json!({ "cue": d.cue }));
                        d.color.clear();
                    }
                }
            });
        });
        if open && !done {
            self.timeline.cue_dialog = Some(d);
        }
    }

    fn track_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.track_dialog.take() else {
            return;
        };
        let Some(t) = self
            .project()
            .and_then(|p| p.subtitle_track(&d.track))
            .cloned()
        else {
            return;
        };
        let mut open = true;
        let mut done = false;
        egui::Window::new("Subtitle Track")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let mut enter = false;
                egui::Grid::new("subtitle-track-fields").show(ui, |ui| {
                    ui.label("Name");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut d.name)
                            .hint_text(&d.hint)
                            .min_size(egui::vec2(240.0, 0.0)),
                    );
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.end_row();
                    ui.label("Language");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut d.language)
                            .hint_text("en, pt-BR, …")
                            .min_size(egui::vec2(240.0, 0.0)),
                    );
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
                    let mut row =
                        |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str| {
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
                    egui::ComboBox::from_id_salt("subtitle-speakers")
                        .selected_text(
                            PAINTS
                                .iter()
                                .find(|(s, _)| *s == d.paint)
                                .map_or("", |(_, l)| *l),
                        )
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            for (paint, label) in PAINTS {
                                changed |=
                                    ui.selectable_value(&mut d.paint, paint, label).changed();
                            }
                        });
                    ui.end_row();
                });
                if changed {
                    d.preview = style_of(&d).ok();
                }
                ui.label(dim(
                    "Burned in, and written to an IMSC1 file. An empty field keeps the usual look",
                ));
                if !d.error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, &d.error);
                }
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() || enter {
                        let mut params = json!({ "track": d.track });
                        if d.name.trim() != t.name.as_deref().unwrap_or_default() {
                            params["name"] = json!(d.name.trim());
                        }
                        if d.language.trim() != t.language.as_deref().unwrap_or_default() {
                            params["language"] = json!(d.language.trim());
                        }
                        match style_of(&d) {
                            Ok(style) => {
                                if style != t.style {
                                    params["style"] =
                                        serde_json::to_value(&style).unwrap_or(json!({}));
                                }
                                if params.as_object().is_some_and(|o| o.len() > 1) {
                                    self.send("track.set", params);
                                }
                                done = true;
                            }
                            Err(e) => d.error = e,
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            });
        if open && !done {
            self.timeline.track_dialog = Some(d);
        }
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
            self.say(
                "Show a subtitle track to export, or use Export Subtitle in its header's menu",
            );
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
                    "Imported {name} into the Media Pool, {} subtitle(s): drag it onto a \
                     subtitle track, or right-click it > Insert Selected Subtitles to Timeline \
                     Using Timecode",
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
