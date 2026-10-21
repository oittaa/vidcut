use std::time::{Duration, Instant};

use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::looks::{Look, LookKind, Styled, SubtitleLook};
use vidcut_core::model::{
    SubtitleBackground, SubtitleOutline, SubtitleStyle, TitleShadow, TitleStyle,
};

use super::export::Ask;
use super::subtitles::{field, font_list, kept_number, percent, swatch};
use super::workspace::{Workspace, dim};

const FRESH: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub(super) struct Looks {
    subtitles: Vec<Look>,
    titles: Vec<Look>,
    read: Option<Instant>,
}

impl Looks {
    pub(super) fn of(&mut self, kind: LookKind) -> &[Look] {
        if self.read.is_none_or(|t| t.elapsed() > FRESH) {
            self.reread();
        }
        match kind {
            LookKind::Subtitles => &self.subtitles,
            LookKind::Titles => &self.titles,
        }
    }

    pub(super) fn reread(&mut self) {
        let env = crate::env::AvEnv::new();
        self.subtitles = vidcut_core::looks::all(&env, LookKind::Subtitles).looks;
        self.titles = vidcut_core::looks::all(&env, LookKind::Titles).looks;
        self.read = Some(Instant::now());
    }
}

pub(super) fn same_look(look: &Look, styled: &Styled) -> bool {
    match (&look.styled, styled) {
        (Styled::Subtitles(a), Styled::Subtitles(b)) => a.style == b.style,
        (a, b) => a == b,
    }
}

pub(super) fn matching<'a>(looks: &'a [Look], styled: &Styled) -> Option<&'a Look> {
    looks.iter().find(|l| same_look(l, styled))
}

#[derive(Debug, Clone)]
enum Step {
    Choosing,
    Naming(String),
    Deleting(String),
}

pub(super) struct Picker {
    kind: LookKind,
    step: Step,
    chars: u32,
    lines: u32,
    error: String,
    asking: Option<(Ask, String)>,
    focus: bool,
}

impl std::fmt::Debug for Picker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picker")
            .field("kind", &self.kind)
            .field("step", &self.step)
            .finish_non_exhaustive()
    }
}

impl Picker {
    pub(super) fn new(kind: LookKind, looks: &[Look], styled: &Styled) -> Self {
        let mut p = Self {
            kind,
            step: Step::Choosing,
            chars: SubtitleLook::CHARS,
            lines: SubtitleLook::LINES,
            error: String::new(),
            asking: None,
            focus: false,
        };
        if let Some(l) = matching(looks, styled) {
            p.laid(&l.styled);
        }
        p
    }

    fn laid(&mut self, styled: &Styled) {
        if let Styled::Subtitles(l) = styled {
            self.chars = l.chars();
            self.lines = l.lines();
        }
    }
}

fn mine(looks: &[Look], name: &str) -> bool {
    looks
        .iter()
        .any(|l| !l.built_in && l.name.to_lowercase() == name.trim().to_lowercase())
}

fn built_in(looks: &[Look], name: &str) -> bool {
    looks
        .iter()
        .any(|l| l.built_in && l.name.to_lowercase() == name.trim().to_lowercase())
}

impl Workspace {
    pub(super) fn look_picker(
        &mut self,
        ui: &mut egui::Ui,
        picker: &mut Picker,
        current: Result<Styled, String>,
    ) -> Option<Styled> {
        if let Some((ask, what)) = &picker.asking
            && let Some(answer) = ask.take()
        {
            match answer {
                Ok(_) => {
                    self.say(what.clone());
                    self.looks.reread();
                    picker.step = Step::Choosing;
                    picker.error.clear();
                }
                Err(e) => picker.error = e.message,
            }
            picker.asking = None;
        }
        let looks = self.looks.of(picker.kind).to_vec();
        let shown = current.as_ref().ok().and_then(|c| matching(&looks, c));
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.label("Look");
            egui::ComboBox::from_id_salt(("look-picker", picker.kind))
                .selected_text(shown.map_or("Custom", |l| l.name.as_str()))
                .width(160.0)
                .show_ui(ui, |ui| {
                    for (i, l) in looks.iter().enumerate() {
                        if i > 0 && looks[i - 1].built_in && !l.built_in {
                            ui.separator();
                        }
                        let on = shown.is_some_and(|s| s.name == l.name);
                        let r = ui.selectable_label(on, &l.name);
                        if r.clicked() {
                            picked = Some(l.styled.clone());
                        }
                    }
                });
            let idle = matches!(picker.step, Step::Choosing) && picker.asking.is_none();
            if ui
                .add_enabled(idle, egui::Button::new("Save as Look…"))
                .clicked()
            {
                let name = shown.filter(|l| !l.built_in).map(|l| l.name.clone());
                picker.step = Step::Naming(name.unwrap_or_default());
                picker.focus = true;
                picker.error.clear();
            }
            let yours = shown.filter(|l| !l.built_in);
            if shown.is_some_and(|l| l.built_in) {
                super::dialog::why(ui, "built-in looks stay");
            }
            if ui
                .add_enabled(idle && yours.is_some(), egui::Button::new("Delete"))
                .clicked()
                && let Some(l) = yours
            {
                picker.step = Step::Deleting(l.name.clone());
                picker.error.clear();
            }
        });
        if let Some(s) = &picked {
            picker.laid(s);
        }
        let mut step = picker.step.clone();
        match &mut step {
            Step::Choosing => {}
            Step::Naming(name) => {
                let mut save = false;
                ui.horizontal(|ui| {
                    ui.label("Name");
                    let r = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("My look")
                            .desired_width(160.0),
                    );
                    if std::mem::take(&mut picker.focus) {
                        r.request_focus();
                    }
                    save |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let label = if mine(&looks, name) {
                        "Replace"
                    } else {
                        "Save"
                    };
                    save |= ui
                        .add_enabled(picker.asking.is_none(), egui::Button::new(label))
                        .clicked();
                    if ui.button("Cancel").clicked() {
                        picker.step = Step::Choosing;
                        picker.error.clear();
                    }
                });
                if picker.kind == LookKind::Subtitles {
                    ui.horizontal(|ui| {
                        ui.label(dim("Create Subtitles from Audio:"));
                        ui.add(egui::DragValue::new(&mut picker.chars).range(1..=60));
                        ui.label(dim("characters per line,"));
                        ui.radio_value(&mut picker.lines, 1, "single");
                        ui.radio_value(&mut picker.lines, 2, "double");
                    });
                }
                if save && matches!(picker.step, Step::Naming(_)) {
                    match (&current, built_in(&looks, name)) {
                        (Err(e), _) => picker.error = e.clone(),
                        (_, true) => {
                            picker.error =
                                format!("{} is a built-in look; choose another name", name.trim())
                        }
                        (Ok(c), false) => {
                            let mut params = json!({
                                "name": name.trim(),
                                "overwrite": mine(&looks, name),
                            });
                            match c {
                                Styled::Subtitles(l) => {
                                    params["subtitles"] = json!(SubtitleLook {
                                        chars: Some(picker.chars),
                                        lines: Some(picker.lines),
                                        style: l.style.clone(),
                                    })
                                }
                                Styled::Titles(l) => params["titles"] = json!(l),
                            }
                            let ask = Ask::send(&self.live, ui.ctx(), "looks.save", params);
                            picker.asking = Some((ask, format!("Saved look {}", name.trim())));
                        }
                    }
                }
            }
            Step::Deleting(name) => {
                ui.horizontal(|ui| {
                    ui.label(format!("Delete look {name}? This can't be undone."));
                    if ui
                        .add_enabled(picker.asking.is_none(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        let params = json!({ "kind": picker.kind, "name": name });
                        let ask = Ask::send(&self.live, ui.ctx(), "looks.delete", params);
                        picker.asking = Some((ask, format!("Deleted look {name}")));
                    }
                    if ui.button("Cancel").clicked() {
                        picker.step = Step::Choosing;
                    }
                });
            }
        }
        if let (Step::Naming(n), Step::Naming(_)) = (step, &picker.step) {
            picker.step = Step::Naming(n);
        }
        if !picker.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &picker.error);
        }
        picked
    }
}

pub(super) const SHADOW_INK: &str = "#000000";

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Parts {
    pub font: Option<String>,
    pub size: Option<f64>,
    pub color: Option<String>,
    pub outline: Option<SubtitleOutline>,
    pub background: Option<SubtitleBackground>,
    pub shadow: Option<TitleShadow>,
}

impl Parts {
    pub(super) fn of_subtitles(s: &SubtitleStyle) -> Self {
        Self {
            font: s.font.clone(),
            size: s.size,
            color: s.color.clone(),
            outline: s.outline.clone(),
            background: s.background.clone(),
            shadow: s.shadow.clone(),
        }
    }

    pub(super) fn of_title(s: &TitleStyle) -> Self {
        Self {
            font: s.font.clone(),
            size: s.size,
            color: s.color.clone(),
            outline: s.outline.clone(),
            background: s.background.clone(),
            shadow: s.shadow.clone(),
        }
    }

    pub(super) fn title(self) -> TitleStyle {
        TitleStyle {
            font: self.font,
            size: self.size,
            color: self.color,
            outline: self.outline,
            background: self.background,
            shadow: self.shadow,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct TextFields {
    font: String,
    size: String,
    color: String,
    outline: String,
    outline_size: String,
    background: String,
    opacity: String,
    shadow: String,
    shadow_opacity: String,
    shadow_across: String,
    shadow_down: String,
    shadow_blur: String,
}

pub(super) fn plain(v: Option<f64>) -> String {
    v.map_or_else(String::new, |n| {
        let s = format!("{n:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    })
}

pub(super) fn kept_plain(
    shown: &str,
    stored: Option<f64>,
    what: &str,
) -> Result<Option<f64>, String> {
    if shown.trim() == plain(stored) {
        return Ok(stored);
    }
    let t = shown.trim().trim_end_matches('°').trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>()
        .map(Some)
        .map_err(|_| format!("{what} is a number, such as 20"))
}

impl TextFields {
    pub(super) fn load(s: &Parts) -> Self {
        let shade = s.shadow.as_ref();
        Self {
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
            shadow: shade.map_or_else(String::new, |h| {
                h.color.clone().unwrap_or_else(|| SHADOW_INK.to_string())
            }),
            shadow_opacity: percent(shade.and_then(|h| h.opacity)),
            shadow_across: percent(shade.and_then(|h| h.offset).map(|o| o[0])),
            shadow_down: percent(shade.and_then(|h| h.offset).map(|o| o[1])),
            shadow_blur: plain(shade.and_then(|h| h.blur)),
        }
    }

    pub(super) fn rows(
        &mut self,
        ui: &mut egui::Ui,
        salt: &str,
        families: &[String],
        size_hint: &str,
    ) -> (bool, bool) {
        let mut enter = false;
        let mut changed = false;
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
        let shadow_opacity = format!("{}%", percent(Some(TitleShadow::OPACITY)));
        let shadow_across = format!("{}%", percent(Some(TitleShadow::OFFSET[0])));
        let shadow_down = format!("{}%", percent(Some(TitleShadow::OFFSET[1])));
        let shadow_blur = plain(Some(TitleShadow::BLUR));
        changed |= row(ui, "Font", &mut self.font, "Open Sans");
        changed |= font_list(ui, &format!("{salt}-font"), &mut self.font, families);
        ui.end_row();
        changed |= row(ui, "Size", &mut self.size, size_hint);
        ui.end_row();
        changed |= row(ui, "Color", &mut self.color, "#ffffff");
        changed |= swatch(ui, &mut self.color, [255, 255, 255]);
        ui.end_row();
        changed |= row(ui, "Outline", &mut self.outline, "#000000");
        changed |= swatch(ui, &mut self.outline, [0, 0, 0]);
        ui.end_row();
        changed |= row(ui, "Outline size", &mut self.outline_size, "5% of the text");
        ui.end_row();
        changed |= row(ui, "Background", &mut self.background, "none");
        changed |= swatch(ui, &mut self.background, [0, 0, 0]);
        ui.end_row();
        changed |= row(ui, "Opacity", &mut self.opacity, "100%");
        ui.end_row();
        changed |= row(ui, "Shadow", &mut self.shadow, "none");
        changed |= swatch(ui, &mut self.shadow, [0, 0, 0]);
        ui.end_row();
        changed |= row(
            ui,
            "Shadow opacity",
            &mut self.shadow_opacity,
            &shadow_opacity,
        );
        ui.end_row();
        changed |= row(ui, "Shadow across", &mut self.shadow_across, &shadow_across);
        ui.end_row();
        changed |= row(ui, "Shadow down", &mut self.shadow_down, &shadow_down);
        ui.end_row();
        changed |= row(ui, "Shadow blur", &mut self.shadow_blur, &shadow_blur);
        ui.end_row();
        (changed, enter)
    }

    fn shadow(&self, stored: Option<&TitleShadow>) -> Result<Option<TitleShadow>, String> {
        let rest = [
            &self.shadow_opacity,
            &self.shadow_across,
            &self.shadow_down,
            &self.shadow_blur,
        ];
        let Some(color) = field(&self.shadow) else {
            return if rest.iter().all(|f| f.trim().is_empty()) {
                Ok(None)
            } else {
                Err("a shadow needs a color".into())
            };
        };
        let color = match stored {
            Some(h) if h.color.is_none() && color.eq_ignore_ascii_case(SHADOW_INK) => None,
            None if color.eq_ignore_ascii_case(SHADOW_INK) => None,
            _ => Some(color),
        };
        let offset = stored.and_then(|h| h.offset);
        let across = kept_number(&self.shadow_across, offset.map(|o| o[0]), "shadow offset")?;
        let down = kept_number(&self.shadow_down, offset.map(|o| o[1]), "shadow offset")?;
        let offset = match (across, down) {
            (Some(x), Some(y)) => Some([x, y]),
            (None, None) => None,
            _ => {
                return Err(
                    "a shadow's offset takes both numbers, or neither for the usual one".into(),
                );
            }
        };
        Ok(Some(TitleShadow {
            color,
            opacity: kept_number(
                &self.shadow_opacity,
                stored.and_then(|h| h.opacity),
                "shadow opacity",
            )?,
            offset,
            blur: kept_plain(&self.shadow_blur, stored.and_then(|h| h.blur), "blur")?,
        }))
    }

    pub(super) fn parts(&self, stored: &Parts) -> Result<Parts, String> {
        let background = match field(&self.background) {
            Some(color) => Some(SubtitleBackground {
                color,
                opacity: kept_number(
                    &self.opacity,
                    stored.background.as_ref().and_then(|b| b.opacity),
                    "opacity",
                )?,
            }),
            None if self.opacity.trim().is_empty() => None,
            None => return Err("a background needs a color".into()),
        };
        let outline_color = field(&self.outline);
        let outline_size = kept_number(
            &self.outline_size,
            stored.outline.as_ref().and_then(|o| o.size),
            "outline size",
        )?;
        Ok(Parts {
            font: field(&self.font),
            size: kept_number(&self.size, stored.size, "size")?,
            color: field(&self.color),
            outline: (outline_color.is_some() || outline_size.is_some()).then_some(
                SubtitleOutline {
                    color: outline_color,
                    size: outline_size,
                },
            ),
            background,
            shadow: self.shadow(stored.shadow.as_ref())?,
        })
    }
}
