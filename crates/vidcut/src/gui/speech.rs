use std::collections::{HashMap, HashSet};

use eframe::egui::{self, RichText};
use serde_json::json;
use vidcut_av::transcribe;
use vidcut_core::model::Project;
use vidcut_core::request::{SpeechModel, SubtitleLook};

use super::export::near_center;
use super::workspace::{Workspace, dim, escape_cancels};

const LOOKS: [(SubtitleLook, &str); 5] = [
    (SubtitleLook::Plain, "Plain"),
    (SubtitleLook::Pop, "Pop: the spoken word yellow and popping"),
    (SubtitleLook::Highlight, "Highlight the spoken word"),
    (SubtitleLook::Reveal, "Reveal words as spoken"),
    (SubtitleLook::Word, "Only the spoken word"),
];

const MODELS: [(SpeechModel, &str); 3] = [
    (
        SpeechModel::ParakeetUltra,
        "25 European languages, a time for each word",
    ),
    (
        SpeechModel::WhisperLargeV3,
        "100 languages, a time for each phrase",
    ),
    (
        SpeechModel::Qwen3Asr,
        "30 languages; its times are estimates",
    ),
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
    look: SubtitleLook,
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
            look: SubtitleLook::Plain,
        }
    }
}

impl Settings {
    fn take_look(&mut self, look: SubtitleLook) {
        let (_, chars, lines) = vidcut_core::ops::subtitles::look(look);
        self.look = look;
        self.chars = chars;
        self.lines = lines;
        if look != SubtitleLook::Plain && self.onto == Onto::Highest {
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
    again: bool,
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
        let total: u64 = needs.iter().map(|n| n.bytes).sum();
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::unique("word-times-download")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(format!("Download {}?", transcribe::size(total)));
            ui.label(format!(
                "Timing words needs {}, not on this computer yet. vidcut downloads {} once, \
                 checks each against its SHA-256 and keeps {} in {}.",
                transcribe::listed(&needs),
                if needs.len() == 1 { "it" } else { "them" },
                if needs.len() == 1 { "it" } else { "them" },
                dir.display()
            ));
            ui.horizontal(|ui| {
                if ui.button("Download and Synchronize").clicked() {
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
        match answer {
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

    pub(crate) fn open_animated_speech(&mut self) {
        self.open_hearing(Vec::new());
        if let Some(d) = &mut self.speech.dialog
            && d.settings.look == SubtitleLook::Plain
        {
            d.settings.take_look(SubtitleLook::Pop);
            d.settings.onto = Onto::New;
            d.settings.model = SpeechModel::ParakeetUltra;
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
            again: false,
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
        if s.look != SubtitleLook::Plain {
            params["look"] = json!(s.look);
        }
        match &s.onto {
            Onto::Highest => {}
            Onto::Track(id) => params["track"] = json!(id),
            Onto::New => params["track"] = json!(format!("S{}", p.subtitles.len() + 1)),
        }
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
        let names: Vec<String> = d
            .media
            .iter()
            .map(|id| p.media(id).map_or_else(|| id.clone(), |m| m.name.clone()))
            .collect();
        let kept = d.media.iter().any(|id| self.snap.transcribed.contains(id));
        let title = match transcribing {
            true => "Transcribe Audio",
            false => "Create Subtitles from Audio",
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 460.0, 420.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!d.asking, |ui| {
                    ui.label(dim(match transcribing {
                        true => format!(
                            "Transcribes the sound of {} on this computer and keeps its words \
                             with their times, for the Transcription window and cutting by \
                             words.",
                            listed(&names)
                        ),
                        false => "Transcribes the speech on the tracks it hears, on this \
                                  computer, and adds it as subtitles."
                            .into(),
                    }));
                    ui.add_space(4.0);
                    ui.label("Model");
                    if transcribing && !s.model.times_words() {
                        s.model = SpeechModel::default();
                    }
                    for (i, (model, note)) in MODELS.iter().enumerate() {
                        let f = transcribe::file_of(*model);
                        let size = transcribe::size(f.bytes);
                        let have = match d.have[i] {
                            true => format!("{size}, on this computer"),
                            false => format!("{size}, downloaded when first used"),
                        };
                        let title = match model {
                            SpeechModel::ParakeetUltra => format!("{} (default)", f.title),
                            _ => f.title.to_string(),
                        };
                        let usable = !transcribing || model.times_words();
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(usable, |ui| {
                                ui.radio_value(&mut s.model, *model, title)
                                    .on_disabled_hover_text(
                                        "It doesn't time each word, which cutting by words needs",
                                    );
                            });
                            ui.label(dim(have));
                        });
                        ui.indent(("speech-model-note", i), |ui| ui.label(dim(*note)));
                    }
                    if s.language
                        .as_deref()
                        .is_some_and(|l| !transcribe::file_of(s.model).languages.contains(&l))
                    {
                        s.language = None;
                    }
                    if let Some(w) = &d.short[index(s.model)] {
                        ui.label(
                            RichText::new(format!("⚠ {w}; it runs anyway"))
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                    if !d.gpu {
                        ui.label(dim(
                            "GPU acceleration is off in Preferences: the model runs on the CPU",
                        ));
                    }
                    ui.separator();
                    egui::Grid::new("speech-options")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Language");
                            let shown = s.language.as_deref().map_or("Auto".into(), language_label);
                            egui::ComboBox::from_id_salt("speech-language")
                                .selected_text(shown)
                                .height(320.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut s.language, None, "Auto");
                                    let mut known: Vec<&str> =
                                        transcribe::file_of(s.model).languages.to_vec();
                                    known.sort_by_key(|c| language_label(c));
                                    for code in known {
                                        ui.selectable_value(
                                            &mut s.language,
                                            Some(code.to_string()),
                                            language_label(code),
                                        );
                                    }
                                });
                            ui.end_row();
                            if transcribing {
                                if kept {
                                    ui.label("Transcribed");
                                    ui.checkbox(
                                        &mut d.again,
                                        "Transcribe again, replacing the words kept",
                                    );
                                    ui.end_row();
                                }
                                return;
                            }
                            ui.label("Range");
                            ui.vertical(|ui| {
                                let whole = format!("Whole timeline ({})", self.src_tc(p.end()));
                                if ui.radio(!s.in_out || range.is_none(), whole).clicked() {
                                    s.in_out = false;
                                }
                                let label = match range {
                                    Some((a, b)) => format!(
                                        "In–Out, {} – {} ({})",
                                        self.tc(a),
                                        self.tc(b - 1),
                                        self.src_tc(b - a)
                                    ),
                                    None => "In–Out (not marked)".into(),
                                };
                                ui.add_enabled_ui(range.is_some(), |ui| {
                                    if ui.radio(s.in_out && range.is_some(), label).clicked() {
                                        s.in_out = true;
                                    }
                                });
                            });
                            ui.end_row();
                            ui.label("Hear");
                            heard = d.hear.ui(ui, &p);
                            ui.end_row();
                            ui.label("Words from");
                            ui.vertical(|ui| {
                                let can = s.model.times_words();
                                let mut media = s.from_media && can;
                                ui.add_enabled_ui(can, |ui| {
                                    if ui
                                        .radio(media, "Each clip's file")
                                        .on_hover_text(
                                            "Words kept for a file are used; a file without \
                                             them is transcribed and keeps them, for the \
                                             Transcription window and next time",
                                        )
                                        .on_disabled_hover_text(
                                            "Only Parakeet Ultra times each word, which placing \
                                             a file's words through its clips needs",
                                        )
                                        .clicked()
                                    {
                                        media = true;
                                    }
                                });
                                if ui
                                    .radio(!media, "The timeline's mix")
                                    .on_hover_text(
                                        "Hears the tracks mixed over the range; nothing is kept",
                                    )
                                    .clicked()
                                {
                                    media = false;
                                }
                                if can {
                                    s.from_media = media;
                                }
                            });
                            ui.end_row();
                            ui.label("Track");
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
                                    let new = s.onto == Onto::New
                                        || (s.onto == Onto::Highest && highest.is_none());
                                    if ui.selectable_label(new, "New Track").clicked() {
                                        s.onto = Onto::New;
                                    }
                                });
                            ui.end_row();
                            ui.label("Look");
                            let mut look = s.look;
                            egui::ComboBox::from_id_salt("speech-look")
                                .selected_text(
                                    LOOKS
                                        .iter()
                                        .find(|(l, _)| *l == look)
                                        .map_or("", |(_, t)| *t),
                                )
                                .show_ui(ui, |ui| {
                                    for (l, text) in LOOKS {
                                        ui.selectable_value(&mut look, l, text);
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Animated looks show each word as it is spoken; the track's \
                                     style can be changed later in its Subtitle Track window",
                                );
                            if look != s.look {
                                s.take_look(look);
                            }
                            ui.end_row();
                            ui.label("Max Characters per Line");
                            ui.add(egui::DragValue::new(&mut s.chars).range(1..=60));
                            ui.end_row();
                            ui.label("Lines");
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut s.lines, 1, "Single");
                                ui.radio_value(&mut s.lines, 2, "Double");
                            });
                            ui.end_row();
                            ui.label("Gap Between Subtitles");
                            ui.add(
                                egui::DragValue::new(&mut s.gap)
                                    .range(0..=10)
                                    .suffix(" frames"),
                            );
                            ui.end_row();
                        });
                    let (a, b) = match range {
                        Some(marks) if s.in_out => marks,
                        _ => (0, p.end()),
                    };
                    ui.separator();
                    if !transcribing {
                        match heard.is_empty() {
                            true => ui.label(
                                RichText::new("Tick a track to hear")
                                    .color(ui.visuals().warn_fg_color),
                            ),
                            false => ui.label(format!(
                                "Hears {}, {} – {}",
                                listed(&heard),
                                self.tc(a),
                                self.tc(b - 1)
                            )),
                        };
                        if s.from_media && s.model.times_words() && !heard.is_empty() {
                            let missing = untranscribed(&p, &heard, (a, b), &self.snap.transcribed);
                            let secs: f64 = missing
                                .iter()
                                .filter_map(|id| p.media(id))
                                .map(|m| m.info.duration)
                                .sum();
                            let names: Vec<String> = missing
                                .iter()
                                .map(|id| {
                                    p.media(id).map_or_else(|| id.clone(), |m| m.name.clone())
                                })
                                .collect();
                            ui.label(dim(match missing.len() {
                                0 => "Uses the words kept for each clip's file".to_string(),
                                _ => format!(
                                    "Transcribes {} first, {} in all, and keeps the words",
                                    listed(&names),
                                    length(secs)
                                ),
                            }));
                            if s.language.is_some() {
                                ui.label(dim(
                                    "A file whose words weren't made in this language is transcribed again",
                                ));
                            }
                        }
                    }
                    ui.label(dim("Jobs shows its progress, and can cancel it"));
                    ui.horizontal(|ui| {
                        let label = if transcribing { "Transcribe" } else { "Create" };
                        create = ui
                            .add_enabled(
                                transcribing || !heard.is_empty(),
                                egui::Button::new(label),
                            )
                            .clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            });
        let dir = crate::paths::models_dir();
        if create {
            d.have[index(s.model)] = transcribe::present(&dir, file);
            if transcribe::needed(&dir, Some(file)).is_empty() {
                let settings = s.clone();
                match transcribing {
                    true => self.transcribe_media(&settings, &d.media, d.again, false),
                    false => self.create_subtitles(&settings, &heard, false),
                }
                return;
            }
            d.asking = true;
        }
        if d.asking {
            let needs = transcribe::needed(&dir, Some(file));
            let total: u64 = needs.iter().map(|n| n.bytes).sum();
            let mut answer = None;
            let modal = egui::Modal::new(egui::Id::unique("speech-download")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(format!("Download {}?", transcribe::size(total)));
                ui.label(format!(
                    "Hearing speech needs {}, not on this computer yet. vidcut downloads {} once, \
                     checks each against its SHA-256 and keeps {} in {}.",
                    transcribe::listed(&needs),
                    if needs.len() == 1 { "it" } else { "them" },
                    if needs.len() == 1 { "it" } else { "them" },
                    dir.display()
                ));
                ui.label(dim("Jobs shows the download's progress, and can cancel it"));
                ui.horizontal(|ui| {
                    let go = match transcribing {
                        true => "Download and Transcribe",
                        false => "Download and Create",
                    };
                    if ui.button(go).clicked() {
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
            match answer {
                Some(true) => {
                    let settings = d.settings.clone();
                    match transcribing {
                        true => self.transcribe_media(&settings, &d.media, d.again, true),
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
