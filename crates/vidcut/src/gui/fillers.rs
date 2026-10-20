use std::collections::BTreeMap;

use eframe::egui;
use serde_json::{Map, Value, json};
use vidcut_core::transcript::{FILLER_LANGUAGES, built_in_fillers, filler_list};

use super::speech::language_label;
use super::workspace::dim;

pub(super) type Lists = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Scope {
    Project,
    All,
}

#[derive(Debug)]
pub(super) struct FillerEditor {
    pub(super) language: String,
    pub(super) scope: Scope,
    text: String,
    loaded: Option<(String, Scope, Vec<String>)>,
}

impl Default for FillerEditor {
    fn default() -> Self {
        Self::new("en".into(), Scope::All)
    }
}

impl FillerEditor {
    pub(super) fn new(language: String, scope: Scope) -> Self {
        Self {
            language,
            scope,
            text: String::new(),
            loaded: None,
        }
    }
}

pub(super) struct Change {
    pub(super) scope: Scope,
    language: String,
    words: Option<Vec<String>>,
}

impl Change {
    pub(super) fn request(&self) -> (&'static str, Value) {
        let mut lists = Map::new();
        lists.insert(self.language.clone(), json!(self.words));
        match self.scope {
            Scope::Project => ("project.set", json!({ "fillers": lists })),
            Scope::All => (
                "preferences.set",
                json!({ "transcripts": { "fillers": lists } }),
            ),
        }
    }
}

fn in_use(
    scope: Scope,
    language: &str,
    project: Option<&Lists>,
    preferred: &Lists,
) -> (Vec<String>, &'static str) {
    if scope == Scope::Project
        && let Some(words) = project.and_then(|p| p.get(language))
    {
        return (words.clone(), "this project's own list");
    }
    if let Some(words) = preferred.get(language) {
        return (words.clone(), "your list for all projects");
    }
    (built_in_fillers(language), "the built-in list")
}

pub(super) fn editor(
    ui: &mut egui::Ui,
    e: &mut FillerEditor,
    project: Option<&Lists>,
    preferred: &Lists,
    busy: bool,
) -> Option<Change> {
    ui.horizontal(|ui| {
        ui.label("Language");
        egui::ComboBox::from_id_salt("filler-language")
            .selected_text(language_label(&e.language))
            .height(320.0)
            .show_ui(ui, |ui| {
                for code in FILLER_LANGUAGES {
                    ui.selectable_value(&mut e.language, code.to_string(), language_label(code));
                }
            });
    });
    if project.is_some() {
        ui.horizontal(|ui| {
            ui.label("For");
            ui.radio_value(&mut e.scope, Scope::Project, "This project");
            ui.radio_value(&mut e.scope, Scope::All, "All projects");
        });
    }
    let (words, from) = in_use(e.scope, &e.language, project, preferred);
    let now = (e.language.clone(), e.scope, words.clone());
    if e.loaded.as_ref() != Some(&now) {
        e.text = words.join(" ");
        e.loaded = Some(now);
    }
    ui.add_enabled(
        !busy,
        egui::TextEdit::multiline(&mut e.text)
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    ui.label(dim(format!(
        "Spaces or commas between words; case doesn't matter. In use: {from}."
    )));
    let typed: Vec<String> = e
        .text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    let typed = filler_list(&typed).unwrap_or_default();
    let mut change = None;
    ui.horizontal(|ui| {
        let save_tip = match e.scope {
            Scope::Project => "Mark these words in this project",
            Scope::All => "Mark these words in every project without its own list",
        };
        if ui
            .add_enabled(!busy && typed != words, egui::Button::new("Save"))
            .on_hover_text(save_tip)
            .clicked()
        {
            change = Some(Change {
                scope: e.scope,
                language: e.language.clone(),
                words: Some(typed.clone()),
            });
        }
        let (label, tip, own) = match e.scope {
            Scope::Project => (
                "Use the List for All Projects",
                "Drop this project's own list",
                project.is_some_and(|p| p.contains_key(&e.language)),
            ),
            Scope::All => (
                "Use the Built-in List",
                "Drop your list for all projects",
                preferred.contains_key(&e.language),
            ),
        };
        if ui
            .add_enabled(!busy && own, egui::Button::new(label))
            .on_hover_text(tip)
            .clicked()
        {
            change = Some(Change {
                scope: e.scope,
                language: e.language.clone(),
                words: None,
            });
        }
    });
    change
}
