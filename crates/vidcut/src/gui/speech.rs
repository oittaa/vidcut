use std::collections::{HashMap, HashSet};

use eframe::egui;
use serde_json::json;
use vidcut_av::transcribe;
use vidcut_core::looks::{Look, LookKind, Styled};
use vidcut_core::model::Project;
use vidcut_core::request::{FindSpeech, SpeechModel};

use super::dialog;
use super::look_tiles::Tiles;
use super::theme;
use super::workspace::{Workspace, dim, escape_cancels};

const PLAIN: &str = "Plain";

const MODELS: [(SpeechModel, &str); 3] = [
    (
        SpeechModel::ParakeetUltra,
        "25 European languages · times each word",
    ),
    (
        SpeechModel::WhisperLargeV3,
        "100 languages · times each phrase",
    ),
    (SpeechModel::Qwen3Asr, "30 languages · estimated times"),
];

#[derive(Debug, Clone, PartialEq)]
enum Onto {
    Highest,
    Track(String),
    New,
}

#[derive(Debug, Clone)]
struct Settings {
    model: SpeechModel,
    language: Option<String>,
    in_out: bool,
    onto: Onto,
    chars: u32,
    lines: u32,
    gap: u32,
    from_media: bool,
    look: String,
    find: FindSpeech,
    sound_db: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: SpeechModel::default(),
            language: None,
            in_out: true,
            onto: Onto::Highest,
            chars: 42,
            lines: 1,
            gap: 0,
            from_media: true,
            look: PLAIN.into(),
            find: FindSpeech::Voice,
            sound_db: vidcut_core::transcript::SOUND_DB,
        }
    }
}

fn found_by(s: &Settings, params: &mut serde_json::Value) {
    params["find"] = json!(s.find);
    if s.find == FindSpeech::Sound && s.sound_db != vidcut_core::transcript::SOUND_DB {
        params["sound_db"] = json!(s.sound_db);
    }
}

fn find_rows(ui: &mut egui::Ui, s: &mut Settings) {
    dialog::row(ui, "Find Speech By", |ui| {
        dialog::switch(ui, |ui| {
            ui.selectable_value(&mut s.find, FindSpeech::Voice, "Voice")
                .on_hover_text("Where people speak; misses singing");
            ui.selectable_value(&mut s.find, FindSpeech::Sound, "Sound Level")
                .on_hover_text("All sound above a level: singing and speech over music");
        });
    });
    if s.find == FindSpeech::Sound {
        dialog::row(ui, "Silence Below", |ui| {
            ui.add(
                egui::DragValue::new(&mut s.sound_db)
                    .range(-80.0..=-10.0)
                    .speed(0.5)
                    .suffix(" dBFS"),
            )
            .on_hover_text("Quieter than this for half a second is silence");
        });
    }
}

fn model_choice(ui: &mut egui::Ui, s: &mut Settings, have: &[bool; 3], words_only: bool) {
    let c = theme::colors();
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    egui::ComboBox::from_id_salt("speech-model")
        .selected_text(transcribe::file_of(s.model).title)
        .width(170.0)
        .show_ui(ui, |ui| {
            for (i, (model, note)) in MODELS.iter().enumerate() {
                let f = transcribe::file_of(*model);
                let size = match have[i] {
                    true => transcribe::size(f.bytes),
                    false => format!("{} download", transcribe::size(f.bytes)),
                };
                let mut job = egui::text::LayoutJob::default();
                job.append(f.title, 0.0, egui::TextFormat::simple(body.clone(), c.text));
                job.append(
                    &format!("\n{note} · {size}"),
                    0.0,
                    egui::TextFormat::simple(small.clone(), c.text_dim),
                );
                ui.add_enabled_ui(!words_only || model.times_words(), |ui| {
                    ui.selectable_value(&mut s.model, *model, job)
                        .on_disabled_hover_text(
                            "Doesn't time each word, which cutting by words needs",
                        );
                });
            }
        });
    if !have[index(s.model)] {
        ui.label(dim(format!(
            "{} download",
            transcribe::size(transcribe::file_of(s.model).bytes)
        )));
    }
}

fn language_choice(ui: &mut egui::Ui, s: &mut Settings) {
    let shown = s.language.as_deref().map_or("Auto".into(), language_label);
    egui::ComboBox::from_id_salt("speech-language")
        .selected_text(shown)
        .width(170.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut s.language, None, "Auto");
            let mut known: Vec<&str> = transcribe::file_of(s.model).languages.to_vec();
            known.sort_by_key(|c| language_label(c));
            for code in known {
                ui.selectable_value(
                    &mut s.language,
                    Some(code.to_string()),
                    language_label(code),
                );
            }
        });
}

fn track_choice(ui: &mut egui::Ui, s: &mut Settings, p: &Project) {
    let highest = p.subtitles.last();
    let shown = match (&s.onto, highest) {
        (Onto::Highest, Some(t)) => t.display_name(),
        (Onto::Track(id), _) => p
            .subtitle_track(id)
            .map_or_else(|| id.clone(), |t| t.display_name()),
        (Onto::New, _) | (Onto::Highest, None) => "New Track".into(),
    };
    egui::ComboBox::from_id_salt("speech-track")
        .selected_text(shown)
        .width(170.0)
        .show_ui(ui, |ui| {
            for t in p.subtitles.iter().rev() {
                let on = match &s.onto {
                    Onto::Highest => highest.is_some_and(|h| h.id == t.id),
                    Onto::Track(id) => *id == t.id,
                    Onto::New => false,
                };
                if ui.selectable_label(on, t.display_name()).clicked() {
                    s.onto = Onto::Track(t.id.clone());
                }
            }
            let new = s.onto == Onto::New || (s.onto == Onto::Highest && highest.is_none());
            if ui.selectable_label(new, "New Track").clicked() {
                s.onto = Onto::New;
            }
        });
}

fn words_from(ui: &mut egui::Ui, s: &mut Settings) {
    let can = s.model.times_words();
    let mut media = s.from_media && can;
    dialog::switch(ui, |ui| {
        ui.add_enabled_ui(can, |ui| {
            if ui
                .selectable_label(media, "Each File")
                .on_hover_text("Each clip's file, transcribed once and kept")
                .on_disabled_hover_text("Needs a model that times each word")
                .clicked()
            {
                media = true;
            }
        });
        if ui
            .selectable_label(!media, "Timeline Mix")
            .on_hover_text("The tracks mixed over the range; nothing is kept")
            .clicked()
        {
            media = false;
        }
    });
    if can {
        s.from_media = media;
    }
}

fn ask_download(
    ctx: &egui::Context,
    id: &str,
    needs: &[transcribe::Need],
    go: &str,
) -> Option<bool> {
    let total: u64 = needs.iter().map(|n| n.bytes).sum();
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique(id)).show(ctx, |ui| {
        ui.set_width(dialog::WIDTH);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.heading(format!("Download {}?", transcribe::size(total)));
        ui.add(egui::Label::new(transcribe::listed(needs)).wrap());
        dialog::note(
            ui,
            format!("Kept in {}", crate::paths::models_dir().display()),
        );
        dialog::footer(ui, |ui| {
            if dialog::main_button(ui, go, true) {
                answer = Some(true);
            }
            if ui.button("Not Now").clicked() {
                answer = Some(false);
            }
        });
    });
    if escape_cancels(ctx, &modal) {
        answer = Some(false);
    }
    answer
}

impl Settings {
    fn plain(&self) -> bool {
        self.look.eq_ignore_ascii_case(PLAIN)
    }

    fn take_look(&mut self, look: &Look) {
        if let Styled::Subtitles(l) = &look.styled {
            self.chars = l.chars();
            self.lines = l.lines();
        }
        self.look = look.name.clone();
        if !self.plain() && self.onto == Onto::Highest {
            self.onto = Onto::New;
        }
    }
}

#[derive(Debug)]
struct Dialog {
    settings: Settings,
    have: [bool; 3],
    short: [Option<String>; 3],
    gpu: bool,
    asking: bool,
    hear: Hear,
    media: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct Hear(HashMap<String, bool>);

impl Hear {
    pub(super) fn ui(&mut self, ui: &mut egui::Ui, p: &Project) -> Vec<String> {
        let solo = p.any_solo();
        let mut heard = Vec::new();
        ui.horizontal_wrapped(|ui| {
            let sounding: Vec<_> = p
                .tracks
                .iter()
                .filter(|t| t.clips.iter().any(|c| c.audio.is_some()))
                .collect();
            if sounding.is_empty() {
                ui.label(dim("No track has sound"));
            }
            for t in sounding {
                let mut on = self.0.get(&t.id).copied().unwrap_or(t.audible(solo));
                let label = match &t.name {
                    Some(name) => format!("{} {name}", t.id),
                    None => t.id.clone(),
                };
                let r = ui.checkbox(&mut on, label);
                let r = match t.muted {
                    true => r.on_hover_text("Muted on the timeline"),
                    false => r,
                };
                if r.changed() {
                    self.0.insert(t.id.clone(), on);
                }
                if on {
                    heard.push(t.id.clone());
                }
            }
        });
        heard
    }
}

#[derive(Debug, Default)]
pub struct SpeechUi {
    dialog: Option<Dialog>,
    last: Option<Settings>,
    word_times: Option<String>,
    tiles: Tiles,
}

fn index(model: SpeechModel) -> usize {
    MODELS.iter().position(|(m, _)| *m == model).unwrap_or(0)
}

fn marked(p: &Project) -> Option<(i64, i64)> {
    match (p.marks.mark_in, p.marks.mark_out) {
        (Some(a), Some(b)) if b > a => Some((a, b)),
        _ => None,
    }
}

fn untranscribed(
    p: &Project,
    heard: &[String],
    (a, b): (i64, i64),
    kept: &HashSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in p.tracks.iter().filter(|t| heard.contains(&t.id)) {
        for c in &t.clips {
            let (Some(sound), Some((from, to))) = (&c.audio, c.audio_range()) else {
                continue;
            };
            if c.enabled
                && !sound.muted
                && !c.is_block()
                && to > a
                && from < b
                && !kept.contains(&c.media)
                && !out.contains(&c.media)
            {
                out.push(c.media.clone());
            }
        }
    }
    out
}

fn length(secs: f64) -> String {
    let s = secs.round() as u64;
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s} s"),
        (0, m, 0) => format!("{m} min"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

fn files(n: usize) -> String {
    match n {
        1 => "1 file".into(),
        n => format!("{n} files"),
    }
}

pub(super) fn listed(ids: &[String]) -> String {
    match ids {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn language_name(code: &str) -> &'static str {
    match code {
        "af" => "Afrikaans",
        "am" => "Amharic",
        "ar" => "Arabic",
        "as" => "Assamese",
        "az" => "Azerbaijani",
        "ba" => "Bashkir",
        "be" => "Belarusian",
        "bg" => "Bulgarian",
        "bn" => "Bengali",
        "bo" => "Tibetan",
        "br" => "Breton",
        "bs" => "Bosnian",
        "ca" => "Catalan",
        "cs" => "Czech",
        "cy" => "Welsh",
        "da" => "Danish",
        "de" => "German",
        "el" => "Greek",
        "en" => "English",
        "es" => "Spanish",
        "et" => "Estonian",
        "eu" => "Basque",
        "fa" => "Persian",
        "fi" => "Finnish",
        "fil" => "Filipino",
        "fo" => "Faroese",
        "fr" => "French",
        "gl" => "Galician",
        "gu" => "Gujarati",
        "ha" => "Hausa",
        "haw" => "Hawaiian",
        "he" => "Hebrew",
        "hi" => "Hindi",
        "hr" => "Croatian",
        "ht" => "Haitian Creole",
        "hu" => "Hungarian",
        "hy" => "Armenian",
        "id" => "Indonesian",
        "is" => "Icelandic",
        "it" => "Italian",
        "ja" => "Japanese",
        "jw" => "Javanese",
        "ka" => "Georgian",
        "kk" => "Kazakh",
        "km" => "Khmer",
        "kn" => "Kannada",
        "ko" => "Korean",
        "la" => "Latin",
        "lb" => "Luxembourgish",
        "ln" => "Lingala",
        "lo" => "Lao",
        "lt" => "Lithuanian",
        "lv" => "Latvian",
        "mg" => "Malagasy",
        "mi" => "Maori",
        "mk" => "Macedonian",
        "ml" => "Malayalam",
        "mn" => "Mongolian",
        "mr" => "Marathi",
        "ms" => "Malay",
        "mt" => "Maltese",
        "my" => "Burmese",
        "ne" => "Nepali",
        "nl" => "Dutch",
        "nn" => "Norwegian Nynorsk",
        "no" => "Norwegian",
        "oc" => "Occitan",
        "pa" => "Punjabi",
        "pl" => "Polish",
        "ps" => "Pashto",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sa" => "Sanskrit",
        "sd" => "Sindhi",
        "si" => "Sinhala",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "sn" => "Shona",
        "so" => "Somali",
        "sq" => "Albanian",
        "sr" => "Serbian",
        "su" => "Sundanese",
        "sv" => "Swedish",
        "sw" => "Swahili",
        "ta" => "Tamil",
        "te" => "Telugu",
        "tg" => "Tajik",
        "th" => "Thai",
        "tk" => "Turkmen",
        "tl" => "Tagalog",
        "tr" => "Turkish",
        "tt" => "Tatar",
        "uk" => "Ukrainian",
        "ur" => "Urdu",
        "uz" => "Uzbek",
        "vi" => "Vietnamese",
        "yi" => "Yiddish",
        "yo" => "Yoruba",
        "yue" => "Cantonese",
        "zh" => "Chinese",
        _ => "",
    }
}

pub(super) fn language_label(code: &str) -> String {
    match language_name(code) {
        "" => code.to_string(),
        name => format!("{name} ({code})"),
    }
}

impl Workspace {
    pub(crate) fn open_speech(&mut self) {
        self.open_hearing(Vec::new());
    }

    pub(crate) fn time_words(&mut self, track: &str) {
        let dir = crate::paths::models_dir();
        let file = transcribe::file_of(SpeechModel::ParakeetUltra);
        if transcribe::needed(&dir, Some(file)).is_empty() {
            self.send("subtitles.time_words", json!({ "track": track }));
            self.export.show_jobs = true;
        } else {
            self.speech.word_times = Some(track.to_string());
        }
    }

    fn word_times_window(&mut self, ctx: &egui::Context) {
        let Some(track) = self.speech.word_times.clone() else {
            return;
        };
        let dir = crate::paths::models_dir();
        let needs = transcribe::needed(&dir, Some(transcribe::file_of(SpeechModel::ParakeetUltra)));
        match ask_download(
            ctx,
            "word-times-download",
            &needs,
            "Download and Synchronize",
        ) {
            Some(true) => {
                self.send(
                    "subtitles.time_words",
                    json!({ "track": track, "download": true }),
                );
                self.export.show_jobs = true;
                self.speech.word_times = None;
            }
            Some(false) => self.speech.word_times = None,
            None => {}
        }
    }

    pub(crate) fn open_transcribe(&mut self, media: Vec<String>) {
        if !media.is_empty() {
            self.open_hearing(media);
        }
    }

    fn open_hearing(&mut self, media: Vec<String>) {
        if self.project().is_none() {
            return;
        }
        let dir = crate::paths::models_dir();
        let gpu = crate::preferences::gpu_acceleration();
        self.looks.reread();
        let file = |i: usize| transcribe::file_of(MODELS[i].0);
        self.speech.dialog = Some(Dialog {
            settings: self.speech.last.clone().unwrap_or_default(),
            have: std::array::from_fn(|i| transcribe::present(&dir, file(i))),
            short: std::array::from_fn(|i| {
                gpu.then(|| transcribe::short_of_memory(file(i))).flatten()
            }),
            gpu,
            asking: false,
            hear: Hear::default(),
            media,
        });
    }

    fn transcribe_media(&mut self, s: &Settings, media: &[String], again: bool, download: bool) {
        let mut params = json!({ "media": media, "model": s.model });
        if let Some(l) = &s.language {
            params["language"] = json!(l);
        }
        if again {
            params["again"] = json!(true);
        }
        found_by(s, &mut params);
        if download {
            params["download"] = json!(true);
        }
        self.send("media.transcribe", params);
        self.export.show_jobs = true;
        self.speech.last = Some(s.clone());
    }

    fn create_subtitles(&mut self, s: &Settings, heard: &[String], download: bool) {
        let Some(p) = self.project() else {
            return;
        };
        let range = match s.in_out && marked(p).is_some() {
            true => "in..out",
            false => "start..end",
        };
        let mut params = json!({
            "model": s.model,
            "range": range,
            "tracks": heard,
            "chars": s.chars,
            "lines": s.lines,
            "gap": s.gap,
            "hear": if s.from_media && s.model.times_words() { "media" } else { "mix" },
        });
        if let Some(l) = &s.language {
            params["language"] = json!(l);
        }
        if !s.plain() {
            params["look"] = json!(s.look);
        }
        match &s.onto {
            Onto::Highest => {}
            Onto::Track(id) => params["track"] = json!(id),
            Onto::New => params["track"] = json!(format!("S{}", p.subtitles.len() + 1)),
        }
        found_by(s, &mut params);
        if download {
            params["download"] = json!(true);
        }
        self.send("subtitles.transcribe", params);
        self.export.show_jobs = true;
        self.speech.last = Some(s.clone());
    }

    pub(super) fn speech_window(&mut self, ctx: &egui::Context) {
        self.word_times_window(ctx);
        let Some(mut d) = self.speech.dialog.take() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let looks = self.looks.of(LookKind::Subtitles).to_vec();
        let mut tiles = std::mem::take(&mut self.speech.tiles);
        let s = &mut d.settings;
        if let Onto::Track(id) = &s.onto
            && p.subtitle_index(id).is_none()
        {
            s.onto = Onto::Highest;
        }
        let range = marked(&p);
        let file = transcribe::file_of(s.model);
        let mut heard: Vec<String> = Vec::new();
        let mut open = true;
        let (mut create, mut cancel) = (false, false);
        let transcribing = !d.media.is_empty();
        if transcribing && !s.model.times_words() {
            s.model = SpeechModel::default();
        }
        if s.language
            .as_deref()
            .is_some_and(|l| !file.languages.contains(&l))
        {
            s.language = None;
        }
        let names: Vec<String> = d
            .media
            .iter()
            .map(|id| p.media(id).map_or_else(|| id.clone(), |m| m.name.clone()))
            .collect();
        let kept = d
            .media
            .iter()
            .filter(|id| self.snap.transcribed.contains(*id))
            .count();
        let again = transcribing && kept == d.media.len();
        let title = match transcribing {
            true => "Transcribe Audio",
            false => "Create Subtitles from Audio",
        };
        dialog::window(ctx, title, &mut open, |ui| {
            ui.add_enabled_ui(!d.asking, |ui| {
                if transcribing {
                    let secs: f64 = d
                        .media
                        .iter()
                        .filter_map(|id| p.media(id))
                        .map(|m| m.info.duration)
                        .sum();
                    let what = match names.as_slice() {
                        [one] => one.clone(),
                        many => files(many.len()),
                    };
                    ui.add(egui::Label::new(dim(format!("{what} · {}", length(secs)))).truncate())
                        .on_hover_text(names.join("\n"));
                } else if let Some(l) = tiles.show(ui, &looks, &s.look) {
                    s.take_look(l);
                }
                dialog::form(ui, "speech-form", |ui| {
                    dialog::row(ui, "Model", |ui| model_choice(ui, s, &d.have, transcribing));
                    dialog::row(ui, "Language", |ui| language_choice(ui, s));
                    if transcribing {
                        return;
                    }
                    dialog::row(ui, "Range", |ui| {
                        let in_out = s.in_out && range.is_some();
                        dialog::switch(ui, |ui| {
                            if ui.selectable_label(!in_out, "Whole Timeline").clicked() {
                                s.in_out = false;
                            }
                            ui.add_enabled_ui(range.is_some(), |ui| {
                                if ui
                                    .selectable_label(in_out, "In–Out")
                                    .on_disabled_hover_text("Mark an In and an Out first")
                                    .clicked()
                                {
                                    s.in_out = true;
                                }
                            });
                        });
                        let (a, b) = match range {
                            Some(marks) if in_out => marks,
                            _ => (0, p.end()),
                        };
                        ui.label(dim(self.src_tc(b - a))).on_hover_text(format!(
                            "{} – {}",
                            self.tc(a),
                            self.tc(b - 1)
                        ));
                    });
                    heard = dialog::row(ui, "Hear", |ui| d.hear.ui(ui, &p));
                    dialog::row(ui, "Track", |ui| track_choice(ui, s, &p));
                    dialog::row(ui, "Lines", |ui| {
                        dialog::switch(ui, |ui| {
                            ui.selectable_value(&mut s.lines, 1, "Single");
                            ui.selectable_value(&mut s.lines, 2, "Double");
                        });
                        ui.label(dim("up to"));
                        ui.add(egui::DragValue::new(&mut s.chars).range(1..=60));
                        ui.label(dim("characters"));
                    });
                });
                if let Some(w) = &d.short[index(s.model)] {
                    dialog::warn(ui, format!("{w}; it runs anyway"));
                }
                if !d.gpu {
                    dialog::note(ui, "Runs on the CPU: GPU acceleration is off");
                }
                let unusual =
                    s.find != FindSpeech::Voice || (!transcribing && (s.gap != 0 || !s.from_media));
                dialog::advanced(ui, "speech-advanced", unusual, |ui| {
                    dialog::form(ui, "speech-advanced-form", |ui| {
                        if !transcribing {
                            dialog::row(ui, "Words From", |ui| words_from(ui, s));
                            dialog::row(ui, "Gap", |ui| {
                                ui.add(
                                    egui::DragValue::new(&mut s.gap)
                                        .range(0..=10)
                                        .suffix(" frames"),
                                )
                                .on_hover_text("Between one subtitle and the next");
                            });
                        }
                        find_rows(ui, s);
                    });
                });
                if transcribing {
                    match kept {
                        0 => {}
                        _ if again => {
                            dialog::note(ui, "Already transcribed");
                        }
                        k => {
                            dialog::note(
                                ui,
                                format!("{k} of {} already transcribed", d.media.len()),
                            );
                        }
                    }
                } else if heard.is_empty() {
                    dialog::warn(ui, "Tick a track to hear");
                } else if s.from_media && s.model.times_words() {
                    let (a, b) = match range {
                        Some(marks) if s.in_out => marks,
                        _ => (0, p.end()),
                    };
                    let missing = untranscribed(&p, &heard, (a, b), &self.snap.transcribed);
                    if !missing.is_empty() {
                        let secs: f64 = missing
                            .iter()
                            .filter_map(|id| p.media(id))
                            .map(|m| m.info.duration)
                            .sum();
                        let names: Vec<String> = missing
                            .iter()
                            .map(|id| p.media(id).map_or_else(|| id.clone(), |m| m.name.clone()))
                            .collect();
                        dialog::note(
                            ui,
                            format!(
                                "Transcribes {} first ({})",
                                files(missing.len()),
                                length(secs)
                            ),
                        )
                        .on_hover_text(names.join("\n"));
                    }
                }
                dialog::footer(ui, |ui| {
                    let label = match (transcribing, again) {
                        (true, true) => "Transcribe Again",
                        (true, false) => "Transcribe",
                        (false, _) => "Create",
                    };
                    create = dialog::main_button(ui, label, transcribing || !heard.is_empty());
                    cancel = ui.button("Cancel").clicked();
                });
            });
        });
        self.speech.tiles = tiles;
        let dir = crate::paths::models_dir();
        if create {
            d.have[index(s.model)] = transcribe::present(&dir, file);
            if transcribe::needed(&dir, Some(file)).is_empty() {
                let settings = s.clone();
                match transcribing {
                    true => self.transcribe_media(&settings, &d.media, again, false),
                    false => self.create_subtitles(&settings, &heard, false),
                }
                return;
            }
            d.asking = true;
        }
        if d.asking {
            let needs = transcribe::needed(&dir, Some(file));
            let go = match transcribing {
                true => "Download and Transcribe",
                false => "Download and Create",
            };
            match ask_download(ctx, "speech-download", &needs, go) {
                Some(true) => {
                    let settings = d.settings.clone();
                    match transcribing {
                        true => self.transcribe_media(&settings, &d.media, again, true),
                        false => self.create_subtitles(&settings, &heard, true),
                    }
                    return;
                }
                Some(false) => d.asking = false,
                None => {}
            }
        }
        if open && !cancel {
            self.speech.dialog = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{files, length};

    #[test]
    fn one_file_is_singular() {
        assert_eq!(files(1), "1 file");
    }

    #[test]
    fn several_files_are_plural() {
        assert_eq!(files(3), "3 files");
    }

    #[test]
    fn a_length_under_an_hour_reads_in_minutes_and_seconds() {
        assert_eq!(length(754.4), "12 min 34 s");
    }
}
