use std::path::PathBuf;

use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::model::{
    LetterCase, Place, SubtitleBackground, SubtitleOutline, SubtitleStyle, SubtitleTrack,
    SubtitleWords, WordsShow,
};

use super::export::{Ask, Picked};
use super::folders::Kind;
use super::keys;
use super::workspace::{Workspace, dim};

#[derive(Debug, Clone)]
pub struct CueDialog {
    pub cue: String,
    pub text: String,
    pub place: Place,
    loaded: bool,
    focus: bool,
}

#[derive(Debug, Clone)]
pub struct TrackDialog {
    track: String,
    name: String,
    language: String,
    hint: String,
    font: String,
    size: String,
    color: String,
    outline: String,
    outline_size: String,
    background: String,
    opacity: String,
    margin: String,
    bold: bool,
    case: Option<LetterCase>,
    shadow: bool,
    show: Option<WordsShow>,
    spoken: String,
    pop: String,
    error: String,
    families: Vec<String>,
    preview: Option<SubtitleStyle>,
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

pub(crate) type SubtitlesKey = (
    u64,
    Option<String>,
    Option<(String, String, Place)>,
    Option<SubtitleStyle>,
);

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

fn style_of(d: &TrackDialog, stored: &SubtitleStyle) -> Result<SubtitleStyle, String> {
    let background = match field(&d.background) {
        Some(color) => Some(SubtitleBackground {
            color,
            opacity: kept_number(
                &d.opacity,
                stored.background.as_ref().and_then(|b| b.opacity),
                "opacity",
            )?,
        }),
        None if d.opacity.trim().is_empty() => None,
        None => return Err("a background needs a color".into()),
    };
    let outline_color = field(&d.outline);
    let outline_size = kept_number(
        &d.outline_size,
        stored.outline.as_ref().and_then(|o| o.size),
        "outline size",
    )?;
    let words = match d.show {
        Some(show) => Some(SubtitleWords {
            show: Some(show),
            color: field(&d.spoken),
            pop: kept_number(&d.pop, stored.words.as_ref().and_then(|w| w.pop), "pop")?,
        }),
        None => None,
    };
    let style = SubtitleStyle {
        font: field(&d.font),
        size: kept_number(&d.size, stored.size, "size")?,
        color: field(&d.color),
        outline: (outline_color.is_some() || outline_size.is_some()).then_some(SubtitleOutline {
            color: outline_color,
            size: outline_size,
        }),
        background,
        margin: kept_number(&d.margin, stored.margin, "margin")?,
        bold: d.bold.then_some(true),
        case: d.case,
        shadow: d.shadow.then(|| stored.shadow.clone().unwrap_or_default()),
        words,
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
        })
        .response
        .on_hover_text("Installed fonts");
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
        params
            .as_object()
            .is_some_and(|o| o.len() > 1)
            .then_some(params)
    }

    pub(crate) fn open_cue(&mut self, id: &str) {
        if let Some(d) = &mut self.timeline.cue_dialog
            && d.cue == id
        {
            d.focus = true;
            return;
        }
        if let Some(params) = self
            .timeline
            .cue_dialog
            .as_ref()
            .and_then(|d| self.typed(d))
        {
            self.send("subtitles.set", params);
        }
        let found = self.project().and_then(|p| p.cue(id)).cloned();
        self.timeline.cue_dialog = Some(CueDialog {
            cue: id.to_string(),
            text: found.as_ref().map(|c| c.text.clone()).unwrap_or_default(),
            place: found.as_ref().map(|c| c.place).unwrap_or_default(),
            loaded: found.is_some(),
            focus: true,
        });
    }

    pub(crate) fn open_track_dialog(&mut self, t: &SubtitleTrack) {
        let s = &t.style;
        self.timeline.track_dialog = Some(TrackDialog {
            track: t.id.clone(),
            name: t.name.clone().unwrap_or_default(),
            language: t.language.clone().unwrap_or_default(),
            hint: t.display_name(),
            font: s.font.clone().unwrap_or_default(),
            size: percent(s.size),
            color: s.color.clone().unwrap_or_default(),
            outline: s
                .outline
                .as_ref()
                .and_then(|o| o.color.clone())
                .unwrap_or_default(),
            outline_size: percent(s.outline.as_ref().and_then(|o| o.size)),
            background: s
                .background
                .as_ref()
                .map(|b| b.color.clone())
                .unwrap_or_default(),
            opacity: percent(s.background.as_ref().and_then(|b| b.opacity)),
            margin: percent(s.margin),
            bold: s.bold == Some(true),
            case: s.case,
            shadow: s.shadow.is_some(),
            show: s.words.as_ref().map(|w| w.show.unwrap_or_default()),
            spoken: s
                .words
                .as_ref()
                .and_then(|w| w.color.clone())
                .unwrap_or_default(),
            pop: percent(s.words.as_ref().and_then(|w| w.pop)),
            error: String::new(),
            families: vidcut_av::burn::families(),
            preview: None,
        });
    }

    pub(super) fn sync_subtitles(&mut self) {
        let Some(p) = self.project() else {
            return;
        };
        let track = p.subtitles.iter().find(|t| t.shown);
        let typing = self
            .timeline
            .cue_dialog
            .as_ref()
            .map(|d| (d.cue.clone(), d.text.clone(), d.place));
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
            if let Some((id, text, place)) = &typing
                && let Some(c) = cues.iter_mut().find(|c| c.id == *id)
            {
                let text = vidcut_core::subtitles::normalize(text);
                if !c.words.is_empty()
                    && vidcut_core::subtitles::words(&text).len() != c.words.len()
                {
                    c.words = vidcut_core::subtitles::spread_words(&text, c.duration);
                }
                c.text = text;
                c.place = *place;
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
            Some((_, cue)) if !d.loaded => {
                d.text = cue.text.clone();
                d.place = cue.place;
                d.loaded = true;
            }
            None if d.loaded => return,
            _ => {}
        }
        let mut open = true;
        let mut done = false;
        egui::Window::new("Subtitle")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
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
                        .desired_width(380.0)
                        .hint_text("Its text; Enter starts a new line"),
                );
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                ui.label(dim(
                    "<i>, <b>, <u> and <font color=\"#ffcc00\"> around words style them",
                ));
                ui.horizontal(|ui| {
                    ui.label("Position");
                    for (place, label) in PLACES {
                        ui.radio_value(&mut d.place, place, label);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(keys("Done  (Ctrl+Enter)")).clicked() || save {
                        if let Some(params) = self.typed(&d) {
                            self.send("subtitles.set", params);
                        }
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                    if ui
                        .add_enabled(found.is_some(), egui::Button::new("Delete Subtitle"))
                        .clicked()
                    {
                        self.send("subtitles.remove", json!({ "cues": [d.cue] }));
                        done = true;
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
                egui::Grid::new("subtitle-style-fields").show(ui, |ui| {
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
                    changed |= row(ui, "Font", &mut d.font, "Open Sans");
                    changed |= font_list(ui, "subtitle-font", &mut d.font, &d.families);
                    ui.end_row();
                    changed |= row(ui, "Size", &mut d.size, "5.5% of the picture");
                    ui.end_row();
                    changed |= row(ui, "Color", &mut d.color, "#ffffff");
                    changed |= swatch(ui, &mut d.color, [255, 255, 255]);
                    ui.end_row();
                    changed |= row(ui, "Outline", &mut d.outline, "#000000");
                    changed |= swatch(ui, &mut d.outline, [0, 0, 0]);
                    ui.end_row();
                    changed |= row(ui, "Outline size", &mut d.outline_size, "5% of the text");
                    ui.end_row();
                    changed |= row(ui, "Background", &mut d.background, "none");
                    changed |= swatch(ui, &mut d.background, [0, 0, 0]);
                    ui.end_row();
                    changed |= row(ui, "Opacity", &mut d.opacity, "100%");
                    ui.end_row();
                    changed |= row(ui, "Margin", &mut d.margin, "none: centered at 80% or 20%");
                    ui.end_row();
                    ui.label("Letters");
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut d.bold, "Bold").changed();
                        changed |= ui.checkbox(&mut d.shadow, "Shadow").changed();
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
                });
                if changed {
                    d.preview = style_of(&d, &t.style).ok();
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
                        match style_of(&d, &t.style) {
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
