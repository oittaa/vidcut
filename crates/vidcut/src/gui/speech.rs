use eframe::egui::{self, RichText};
use serde_json::json;
use vidcut_av::transcribe;
use vidcut_core::model::Project;
use vidcut_core::request::SpeechModel;

use super::export::near_center;
use super::workspace::{Workspace, dim, escape_cancels};

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
}

#[derive(Debug, Default)]
pub struct SpeechUi {
    dialog: Option<Dialog>,
    last: Option<Settings>,
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

fn language_label(code: &str) -> String {
    match language_name(code) {
        "" => code.to_string(),
        name => format!("{name} ({code})"),
    }
}

impl Workspace {
    pub(crate) fn open_speech(&mut self) {
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
        });
    }

    fn create_subtitles(&mut self, s: &Settings, download: bool) {
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
            "chars": s.chars,
            "lines": s.lines,
            "gap": s.gap,
        });
        if let Some(l) = &s.language {
            params["language"] = json!(l);
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
        let mut open = true;
        let (mut create, mut cancel) = (false, false);
        egui::Window::new("Create Subtitles from Audio")
            .collapsible(false)
            .resizable(false)
            .default_pos(near_center(ctx, 460.0, 420.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!d.asking, |ui| {
                    ui.label(dim(
                        "Transcribes the timeline's speech on this computer and adds it as \
                         subtitles. Muted tracks aren't heard.",
                    ));
                    ui.add_space(4.0);
                    ui.label("Model");
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
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut s.model, *model, title);
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
                    ui.separator();
                    ui.label(dim("Jobs shows its progress, and can cancel it"));
                    ui.horizontal(|ui| {
                        create = ui.button("Create").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            });
        if create {
            let dir = crate::paths::models_dir();
            d.have[index(s.model)] = transcribe::present(&dir, file);
            if d.have[index(s.model)] {
                let settings = s.clone();
                self.create_subtitles(&settings, false);
                return;
            }
            d.asking = true;
        }
        if d.asking {
            let mut answer = None;
            let modal = egui::Modal::new(egui::Id::unique("speech-download")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(format!("Download {}?", file.title));
                ui.label(format!(
                    "{} isn't on this computer yet. vidcut downloads it once, {} from Hugging \
                     Face, checks it against its SHA-256 and keeps it in {}.",
                    file.title,
                    transcribe::size(file.bytes),
                    crate::paths::models_dir().display()
                ));
                ui.label(dim("Jobs shows the download's progress, and can cancel it"));
                ui.horizontal(|ui| {
                    if ui.button("Download and Create").clicked() {
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
                    self.create_subtitles(&settings, true);
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
