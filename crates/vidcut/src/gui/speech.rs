use std::collections::{HashMap, HashSet};

use eframe::egui;
use serde_json::json;
use vidcut_av::{speech_api, transcribe};
use vidcut_core::looks::{Look, LookKind, Styled};
use vidcut_core::model::Project;
use vidcut_core::request::{FindSpeech, GoogleOptions, SpeechModel, XaiOptions};

use super::dialog;
use super::look_tiles::Tiles;
use super::theme;
use super::workspace::{Workspace, dim, escape_cancels};

const PLAIN: &str = "Plain";

const MODELS: [(SpeechModel, &str); 5] = [
    (
        SpeechModel::ParakeetUltra,
        "25 European languages · times each word",
    ),
    (
        SpeechModel::WhisperLargeV3,
        "100 languages · times each phrase",
    ),
    (SpeechModel::Qwen3Asr, "30 languages · estimated times"),
    (
        SpeechModel::Xai,
        "38 languages · times each word · on xAI's servers",
    ),
    (
        SpeechModel::Google,
        "85 languages · times each word · on Google's servers",
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
    look: String,
    find: FindSpeech,
    sound_db: f64,
    speakers: bool,
    xai: XaiOptions,
    google: GoogleOptions,
    keyterms: String,
    codes: String,
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
            speakers: false,
            xai: XaiOptions::default(),
            google: GoogleOptions::default(),
            keyterms: String::new(),
            codes: String::new(),
        }
    }
}

fn split_list(text: &str) -> Vec<String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

fn hearing(s: &Settings, params: &mut serde_json::Value, send: bool) {
    params["find"] = json!(s.find);
    if s.find == FindSpeech::Sound && s.sound_db != vidcut_core::transcript::SOUND_DB {
        params["sound_db"] = json!(s.sound_db);
    }
    if s.speakers {
        params["speakers"] = json!(true);
    }
    if send {
        params["send"] = json!(true);
    }
    match s.model {
        SpeechModel::Xai => {
            let xai = XaiOptions {
                keyterm: split_list(&s.keyterms),
                ..s.xai.clone()
            };
            if xai != XaiOptions::default() {
                params["xai"] = json!(xai);
            }
        }
        SpeechModel::Google => {
            let google = GoogleOptions {
                language_codes: split_list(&s.codes),
                ..s.google.clone()
            };
            if google != GoogleOptions::default() {
                params["google"] = json!(google);
            }
        }
        _ => {}
    }
}

fn languages(model: SpeechModel) -> Vec<&'static str> {
    match model {
        SpeechModel::Xai => speech_api::XAI_LANGUAGES.to_vec(),
        SpeechModel::Google => speech_api::google_languages(),
        m => transcribe::file_of(m).map_or(Vec::new(), |f| f.languages.to_vec()),
    }
}

fn wanted(s: &Settings) -> Vec<&'static transcribe::File> {
    transcribe::file_of(s.model)
        .into_iter()
        .chain((s.speakers && s.model != SpeechModel::Xai).then_some(&transcribe::SORTFORMER))
        .collect()
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

fn service_rows(ui: &mut egui::Ui, s: &mut Settings, transcribing: bool) {
    match s.model {
        SpeechModel::Xai => {
            dialog::row(ui, "Key Terms", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut s.keyterms)
                        .hint_text("names, jargon")
                        .desired_width(220.0),
                )
                .on_hover_text("Commas between them: 100 at most, 50 characters each");
            });
            dialog::row(ui, "Numbers", |ui| {
                ui.checkbox(&mut s.xai.format, "As Digits")
                    .on_hover_text("“$100” for “one hundred dollars”: needs a language");
            });
            dialog::row(ui, "Filler Words", |ui| {
                let mut keep = s.xai.filler_words.unwrap_or(true);
                if ui
                    .checkbox(&mut keep, "Keep Them")
                    .on_hover_text("Off, Delete Filler Words has nothing to find")
                    .changed()
                {
                    s.xai.filler_words = (!keep).then_some(false);
                }
            });
            dialog::row(ui, "Voice Threshold", |ui| {
                let mut own = s.xai.vad_threshold.is_some();
                ui.checkbox(&mut own, "")
                    .on_hover_text("Off: xAI's 0.5, or 0 when found by sound level");
                let mut v = s.xai.vad_threshold.unwrap_or(0.5);
                ui.add_enabled(
                    own,
                    egui::DragValue::new(&mut v).range(0.0..=1.0).speed(0.01),
                );
                s.xai.vad_threshold = own.then_some(v);
            });
            if transcribing {
                dialog::row(ui, "Channels", |ui| {
                    ui.checkbox(&mut s.xai.multichannel, "Each Separately")
                        .on_hover_text("Each channel on its own; with Speakers, one speaker each");
                });
            }
        }
        SpeechModel::Google => {
            dialog::row(ui, "More Languages", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut s.codes)
                        .hint_text("fi-FI, sv-SE")
                        .desired_width(220.0),
                )
                .on_hover_text("For speech that switches between languages");
            });
        }
        _ => {}
    }
}

fn model_choice(ui: &mut egui::Ui, s: &mut Settings, d: &Ready, words_only: bool) {
    let c = theme::colors();
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    egui::ComboBox::from_id_salt("speech-model")
        .selected_text(s.model.title())
        .width(190.0)
        .show_ui(ui, |ui| {
            for (i, (model, note)) in MODELS.iter().enumerate() {
                let state = match transcribe::file_of(*model) {
                    Some(f) if d.have[i] => transcribe::size(f.bytes),
                    Some(f) => format!("{} download", transcribe::size(f.bytes)),
                    None if d.key[i] => format!("${:.2} an hour", model.dollars_per_hour()),
                    None => "no key".into(),
                };
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    model.title(),
                    0.0,
                    egui::TextFormat::simple(body.clone(), c.text),
                );
                job.append(
                    &format!("\n{note} · {state}"),
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
    let i = index(s.model);
    match transcribe::file_of(s.model) {
        Some(f) if !d.have[i] => {
            ui.label(dim(format!("{} download", transcribe::size(f.bytes))));
        }
        None if !d.key[i] => {
            ui.label(dim("No key: Preferences > Services"));
        }
        _ => {}
    }
}

fn language_choice(ui: &mut egui::Ui, s: &mut Settings) {
    let shown = s.language.as_deref().map_or("Auto".into(), language_label);
    egui::ComboBox::from_id_salt("speech-language")
        .selected_text(shown)
        .width(190.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut s.language, None, "Auto");
            let mut known = languages(s.model);
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

fn speakers_row(ui: &mut egui::Ui, s: &mut Settings) {
    let tip = match s.model {
        SpeechModel::Xai => "One numbering across every file heard",
        SpeechModel::Google => "One numbering up to 30 minutes, past that per file",
        _ => "Up to 4 in each file, on this computer",
    };
    dialog::row(ui, "Speakers", |ui| {
        ui.checkbox(&mut s.speakers, "Tell Who Speaks")
            .on_hover_text(tip);
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
        .width(190.0)
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

struct Paying {
    model: SpeechModel,
    what: String,
    secs: f64,
    speakers: bool,
}

fn ask_payment(ctx: &egui::Context, p: &Paying, again: &mut bool) -> Option<bool> {
    let company = p.model.company();
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique("speech-payment")).show(ctx, |ui| {
        ui.set_width(dialog::WIDTH);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.heading(format!("Send to {company}?"));
        ui.label(format!(
            "{}: {}, about {:.0} MB",
            p.what,
            length(p.secs),
            vidcut_core::transcript::upload_mb(p.secs, 1).ceil()
        ));
        let cost = vidcut_core::transcript::dollars(vidcut_core::transcript::cost(p.model, p.secs));
        ui.label(format!(
            "{}{}, at ${:.2} an hour",
            cost[..1].to_uppercase(),
            &cost[1..],
            p.model.dollars_per_hour()
        ));
        dialog::note(
            ui,
            "The audio leaves this computer. A reply kept from before is free.",
        );
        if p.model == SpeechModel::Google {
            dialog::note(
                ui,
                "A free-tier key lets Google use the audio for its models.",
            );
            if p.speakers && p.secs > 30.0 * 60.0 {
                dialog::note(ui, "Over 30 minutes, speakers come from this computer.");
            }
        }
        ui.checkbox(again, format!("Don't ask again for {company}"));
        dialog::footer(ui, |ui| {
            if dialog::main_button(ui, "Send", true) {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
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

#[derive(Debug, Clone)]
struct Ready {
    have: [bool; 5],
    key: [bool; 5],
    ask: [bool; 5],
}

#[derive(Debug)]
struct Dialog {
    settings: Settings,
    ready: Ready,
    short: [Option<String>; 5],
    gpu: bool,
    asking: bool,
    paying: Option<(f64, String)>,
    paid: bool,
    again: bool,
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

fn heard_files(
    p: &Project,
    heard: &[String],
    (a, b): (i64, i64),
    skip: &HashSet<String>,
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
                && !skip.contains(&c.media)
                && !out.contains(&c.media)
            {
                out.push(c.media.clone());
            }
        }
    }
    out
}

fn duration(p: &Project, media: &[String]) -> f64 {
    media
        .iter()
        .filter_map(|id| p.media(id))
        .map(|m| m.info.duration)
        .sum()
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
        if transcribe::needed(&dir, &[&transcribe::PARAKEET_ULTRA]).is_empty() {
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
        let needs = transcribe::needed(&dir, &[&transcribe::PARAKEET_ULTRA]);
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
        let prefs = crate::preferences::load().unwrap_or_default();
        self.looks.reread();
        let model = |i: usize| MODELS[i].0;
        let ready = Ready {
            have: std::array::from_fn(|i| {
                transcribe::file_of(model(i)).is_none_or(|f| transcribe::present(&dir, f))
            }),
            key: std::array::from_fn(|i| {
                crate::services::of(model(i))
                    .is_none_or(|s| crate::services::source(s, &prefs.services).is_some())
            }),
            ask: std::array::from_fn(|i| match model(i) {
                SpeechModel::Xai => prefs.services.ask_xai,
                SpeechModel::Google => prefs.services.ask_google,
                _ => false,
            }),
        };
        self.speech.dialog = Some(Dialog {
            settings: self.speech.last.clone().unwrap_or_default(),
            ready,
            short: std::array::from_fn(|i| {
                let f = transcribe::file_of(model(i))?;
                gpu.then(|| transcribe::short_of_memory(f)).flatten()
            }),
            gpu,
            asking: false,
            paying: None,
            paid: false,
            again: false,
            hear: Hear::default(),
            media,
        });
    }

    fn transcribe_media(&mut self, s: &Settings, media: &[String], again: bool, go: Go) {
        let mut params = json!({ "media": media, "model": s.model });
        if let Some(l) = &s.language {
            params["language"] = json!(l);
        }
        if again {
            params["again"] = json!(true);
        }
        hearing(s, &mut params, go.send);
        if go.download {
            params["download"] = json!(true);
        }
        self.send("media.transcribe", params);
        self.export.show_jobs = true;
        self.speech.last = Some(s.clone());
    }

    fn create_subtitles(&mut self, s: &Settings, heard: &[String], go: Go) {
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
        hearing(s, &mut params, go.send);
        if go.download {
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
        let mut heard: Vec<String> = Vec::new();
        let mut open = true;
        let (mut create, mut cancel) = (false, false);
        let transcribing = !d.media.is_empty();
        if transcribing && !s.model.times_words() {
            s.model = SpeechModel::default();
        }
        if s.language
            .as_deref()
            .is_some_and(|l| !languages(s.model).contains(&l))
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
        let ready = d.ready.clone();
        dialog::window(ctx, title, &mut open, |ui| {
            ui.add_enabled_ui(!d.asking && d.paying.is_none(), |ui| {
                if transcribing {
                    let what = match names.as_slice() {
                        [one] => one.clone(),
                        many => files(many.len()),
                    };
                    ui.add(
                        egui::Label::new(dim(format!(
                            "{what} · {}",
                            length(duration(&p, &d.media))
                        )))
                        .truncate(),
                    )
                    .on_hover_text(names.join("\n"));
                } else if let Some(l) = tiles.show(ui, &looks, &s.look) {
                    s.take_look(l);
                }
                dialog::form(ui, "speech-form", |ui| {
                    dialog::row(ui, "Model", |ui| model_choice(ui, s, &ready, transcribing));
                    dialog::row(ui, "Language", |ui| language_choice(ui, s));
                    speakers_row(ui, s);
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
                if !d.gpu && !s.model.paid() {
                    dialog::note(ui, "Runs on the CPU: GPU acceleration is off");
                }
                let unusual = s.find != FindSpeech::Voice
                    || (!transcribing && (s.gap != 0 || !s.from_media))
                    || (s.model == SpeechModel::Xai && s.xai != XaiOptions::default())
                    || !s.keyterms.is_empty()
                    || !s.codes.is_empty();
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
                        service_rows(ui, s, transcribing);
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
                    let missing = heard_files(&p, &heard, (a, b), &self.snap.transcribed);
                    if !missing.is_empty() {
                        let names: Vec<String> = missing
                            .iter()
                            .map(|id| p.media(id).map_or_else(|| id.clone(), |m| m.name.clone()))
                            .collect();
                        dialog::note(
                            ui,
                            format!(
                                "Transcribes {} first ({})",
                                files(missing.len()),
                                length(duration(&p, &missing))
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
                    let can = (transcribing || !heard.is_empty()) && ready.key[index(s.model)];
                    create = dialog::main_button(ui, label, can);
                    cancel = ui.button("Cancel").clicked();
                });
            });
        });
        self.speech.tiles = tiles;
        let s = d.settings.clone();
        if let Some((secs, what)) = d.paying.clone() {
            let paying = Paying {
                model: s.model,
                what,
                secs,
                speakers: s.speakers,
            };
            match ask_payment(ctx, &paying, &mut d.again) {
                Some(true) => {
                    if d.again {
                        let key = match s.model {
                            SpeechModel::Google => "ask_google",
                            _ => "ask_xai",
                        };
                        self.send("preferences.set", json!({ "services": { key: false } }));
                    }
                    d.paying = None;
                    d.paid = true;
                    create = true;
                }
                Some(false) => d.paying = None,
                None => {}
            }
        }
        if create && s.model.paid() && !d.paid {
            let (secs, what) = self.upload_size(&p, &s, &d, &heard, again);
            match d.ready.ask[index(s.model)] {
                true => {
                    d.paying = Some((secs, what));
                    create = false;
                }
                false => d.paid = true,
            }
        }
        let dir = crate::paths::models_dir();
        let files_wanted = wanted(&s);
        if create {
            let i = index(s.model);
            d.ready.have[i] =
                transcribe::file_of(s.model).is_none_or(|f| transcribe::present(&dir, f));
            if transcribe::needed(&dir, &files_wanted).is_empty() {
                let go = Go {
                    download: false,
                    send: d.paid,
                };
                match transcribing {
                    true => self.transcribe_media(&s, &d.media, again, go),
                    false => self.create_subtitles(&s, &heard, go),
                }
                return;
            }
            d.asking = true;
        }
        if d.asking {
            let needs = transcribe::needed(&dir, &files_wanted);
            let go = match transcribing {
                true => "Download and Transcribe",
                false => "Download and Create",
            };
            match ask_download(ctx, "speech-download", &needs, go) {
                Some(true) => {
                    let go = Go {
                        download: true,
                        send: d.paid,
                    };
                    match transcribing {
                        true => self.transcribe_media(&s, &d.media, again, go),
                        false => self.create_subtitles(&s, &heard, go),
                    }
                    return;
                }
                Some(false) => {
                    d.asking = false;
                    d.paid = false;
                }
                None => {}
            }
        }
        if open && !cancel {
            self.speech.dialog = Some(d);
        }
    }

    fn upload_size(
        &self,
        p: &Project,
        s: &Settings,
        d: &Dialog,
        heard: &[String],
        again: bool,
    ) -> (f64, String) {
        let none = HashSet::new();
        let skip = match s.speakers || again {
            true => &none,
            false => &self.snap.transcribed,
        };
        if !d.media.is_empty() {
            let media: Vec<String> = d
                .media
                .iter()
                .filter(|m| !skip.contains(*m))
                .cloned()
                .collect();
            return (duration(p, &media), files(media.len()));
        }
        let (a, b) = match marked(p) {
            Some(marks) if s.in_out => marks,
            _ => (0, p.end()),
        };
        match s.from_media && s.model.times_words() {
            true => {
                let media = heard_files(p, heard, (a, b), skip);
                (duration(p, &media), files(media.len()))
            }
            false => ((b - a) as f64 / p.settings.fps.as_f64(), "The mix".into()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Go {
    download: bool,
    send: bool,
}

#[cfg(test)]
mod tests {
    use super::{files, length, split_list};

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

    #[test]
    fn key_terms_part_at_commas() {
        assert_eq!(split_list("Grok, xAI ,,"), ["Grok", "xAI"]);
    }
}
