use std::sync::OnceLock;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::looks::{Look, LookKind, Styled, SubtitleLook};
use vidcut_core::model::{
    Picture, SubtitleBackground, SubtitleOutline, SubtitleStyle, Title, TitleShadow, TitleShape,
    TitleStyle,
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

pub(super) struct Usual<'a> {
    size: &'a str,
    outline: &'a str,
    ink: [u8; 3],
    outline_size: &'a str,
    background: &'a str,
    opacity: &'a str,
    shadow: &'a str,
    switches: bool,
}

impl<'a> Usual<'a> {
    pub(super) fn subtitles(size: &'a str, outline_size: &'a str) -> Self {
        Self {
            size,
            outline: "#ffffff",
            ink: [255, 255, 255],
            outline_size,
            background: "#000000",
            opacity: "50%",
            shadow: SHADOW_INK,
            switches: true,
        }
    }

    pub(super) fn cue() -> Self {
        Self {
            size: "the track",
            outline: "the track",
            ink: [255, 255, 255],
            outline_size: "the track",
            background: "the track",
            opacity: "the track",
            shadow: "none",
            switches: false,
        }
    }

    pub(super) fn title(size: &'a str) -> Self {
        Self {
            size,
            outline: "#000000",
            ink: [0, 0, 0],
            outline_size: "none",
            background: "none",
            opacity: "100%",
            shadow: "none",
            switches: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Parts {
    pub font: Option<String>,
    pub size: Option<f64>,
    pub color: Option<String>,
    pub outline: Option<SubtitleOutline>,
    pub background: Option<SubtitleBackground>,
    pub shadow: Option<TitleShadow>,
    pub frame: Option<[f64; 2]>,
}

impl Parts {
    pub(super) fn of_subtitles(s: &SubtitleStyle, frame: [f64; 2]) -> Self {
        Self {
            font: s.font.clone(),
            size: s.size,
            color: s.color.clone(),
            outline: s.outline.clone(),
            background: s.background.clone(),
            shadow: s.shadow.clone(),
            frame: Some(frame),
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
            frame: None,
        }
    }

    pub(super) fn title(self) -> TitleStyle {
        TitleStyle {
            font: self.font,
            size: self.size,
            line_spacing: None,
            tracking: None,
            case: None,
            color: self.color,
            outline: self.outline,
            background: self.background,
            shape: None,
            shadow: self.shadow,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct TextFields {
    font: String,
    size: String,
    color: String,
    outline: String,
    outline_size: String,
    outside: bool,
    box_off: bool,
    background: String,
    opacity: String,
    shadow_off: bool,
    shadow: String,
    shadow_opacity: String,
    shadow_across: String,
    shadow_down: String,
    shadow_blur: String,
    frame: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ShapeFields {
    width: String,
    height: String,
    across: String,
    down: String,
    corner: String,
    outline: String,
    outline_width: String,
}

impl ShapeFields {
    pub(super) fn load(s: Option<&TitleShape>) -> Self {
        let center = s.and_then(|s| s.center);
        Self {
            width: percent(s.map(|s| s.size[0])),
            height: percent(s.map(|s| s.size[1])),
            across: percent(center.map(|c| c[0])),
            down: percent(center.map(|c| c[1])),
            corner: percent(s.and_then(|s| s.corner)),
            outline: s.and_then(|s| s.outline_color.clone()).unwrap_or_default(),
            outline_width: plain(s.and_then(|s| s.outline_width)),
        }
    }

    pub(super) fn shape(&self, stored: Option<&TitleShape>) -> Result<Option<TitleShape>, String> {
        let bare = [
            &self.width,
            &self.height,
            &self.across,
            &self.down,
            &self.corner,
            &self.outline,
            &self.outline_width,
        ]
        .iter()
        .all(|f| f.trim().is_empty());
        if bare {
            return Ok(None);
        }
        let size = stored.map(|s| s.size);
        let center = stored.and_then(|s| s.center);
        let width = kept_number(&self.width, size.map(|s| s[0]), "box width")?;
        let height = kept_number(&self.height, size.map(|s| s[1]), "box height")?;
        let across = kept_number(&self.across, center.map(|c| c[0]), "box across")?;
        let down = kept_number(&self.down, center.map(|c| c[1]), "box down")?;
        let Some(height) = height else {
            return Err("a box needs a height".into());
        };
        Ok(Some(TitleShape {
            size: [width.unwrap_or(Title::BOX_WIDTH), height],
            center: (across.is_some() || down.is_some())
                .then(|| [across.unwrap_or(0.0), down.unwrap_or(0.0)]),
            corner: kept_number(&self.corner, stored.and_then(|s| s.corner), "box corner")?,
            outline_color: field(&self.outline),
            outline_width: kept_plain(
                &self.outline_width,
                stored.and_then(|s| s.outline_width),
                "box outline width",
            )?,
        }))
    }
}

fn usual_font() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| vidcut_av::burn::usual_family().unwrap_or_else(|| "Sans Serif".to_string()))
}

fn switched(ui: &mut egui::Ui, label: &str, off: &mut bool) -> bool {
    ui.label(label);
    let mut on = !*off;
    let changed = ui.checkbox(&mut on, "").changed();
    *off = !on;
    ui.end_row();
    changed
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

fn offset_text(size: Option<f64>, v: Option<f64>) -> String {
    match size {
        Some(size) => plain(v.map(|v| v * size)),
        None => percent(v),
    }
}

fn kept_offset(shown: &str, stored: Option<f64>, size: Option<f64>) -> Result<Option<f64>, String> {
    let Some(size) = size else {
        return kept_number(shown, stored, "shadow offset");
    };
    if shown.trim() == offset_text(Some(size), stored) {
        return Ok(stored);
    }
    let pixels = kept_plain(shown, None, "shadow offset")?;
    Ok(pixels.map(|v| (v / size).clamp(-Picture::REACH, Picture::REACH)))
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
            outline_size: plain(s.outline.as_ref().and_then(|o| o.size)),
            outside: s.outline.as_ref().is_some_and(|o| o.outside == Some(true)),
            box_off: s
                .background
                .as_ref()
                .is_some_and(SubtitleBackground::is_off),
            background: s
                .background
                .as_ref()
                .map(|b| b.color.clone())
                .unwrap_or_default(),
            opacity: percent(s.background.as_ref().and_then(|b| b.opacity)),
            shadow_off: shade.is_some_and(TitleShadow::is_off),
            shadow: shade.map_or_else(String::new, |h| {
                h.color.clone().unwrap_or_else(|| SHADOW_INK.to_string())
            }),
            shadow_opacity: percent(shade.and_then(|h| h.opacity)),
            shadow_across: offset_text(
                s.frame.map(|f| f[0]),
                shade.and_then(|h| h.offset).map(|o| o[0]),
            ),
            shadow_down: offset_text(
                s.frame.map(|f| f[1]),
                shade.and_then(|h| h.offset).map(|o| o[1]),
            ),
            shadow_blur: plain(shade.and_then(|h| h.blur)),
            frame: s.frame,
        }
    }

    pub(super) fn colors_kept(&self, stored: &Parts) -> Self {
        let was = Self::load(stored);
        let mut out = self.clone();
        for (shown, kept) in [
            (&mut out.outline, was.outline),
            (&mut out.background, was.background),
            (&mut out.shadow, was.shadow),
        ] {
            if shown.trim().is_empty() {
                *shown = kept;
            }
        }
        out
    }

    pub(super) fn refilled(&self, stored: &Parts) -> Self {
        let was = Self::load(stored);
        let kept = |shown: &str, was: &str| {
            if shown.trim().is_empty() {
                was.to_string()
            } else {
                shown.to_string()
            }
        };
        let mut out = self.clone();
        out.size = kept(&self.size, &was.size);
        if !self.outline.trim().is_empty() || was.outline.is_empty() {
            out.outline_size = kept(&self.outline_size, &was.outline_size);
        }
        if !self.background.trim().is_empty() {
            out.opacity = kept(&self.opacity, &was.opacity);
        }
        if !self.shadow.trim().is_empty() {
            out.shadow_opacity = kept(&self.shadow_opacity, &was.shadow_opacity);
            out.shadow_across = kept(&self.shadow_across, &was.shadow_across);
            out.shadow_down = kept(&self.shadow_down, &was.shadow_down);
            out.shadow_blur = kept(&self.shadow_blur, &was.shadow_blur);
        }
        out
    }

    pub(super) fn rows(
        &mut self,
        ui: &mut egui::Ui,
        salt: &str,
        families: &[String],
        usual: &Usual,
        spaced: Option<(&mut String, &mut String)>,
        shaped: Option<&mut ShapeFields>,
    ) -> (bool, bool) {
        let mut enter = false;
        let mut changed = false;
        let mut row = |ui: &mut egui::Ui, on: bool, label: &str, text: &mut String, hint: &str| {
            ui.add_enabled(on, egui::Label::new(label));
            let r = ui.add_enabled(
                on,
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .min_size(egui::vec2(240.0, 0.0)),
            );
            enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            r.changed()
        };
        let shadow_opacity = format!("{}%", percent(Some(TitleShadow::OPACITY)));
        let usual_offset = |size: Option<f64>, v: f64| match size {
            Some(_) => offset_text(size, Some(v)),
            None => format!("{}%", percent(Some(v))),
        };
        let shadow_across = usual_offset(self.frame.map(|f| f[0]), TitleShadow::OFFSET[0]);
        let shadow_down = usual_offset(self.frame.map(|f| f[1]), TitleShadow::OFFSET[1]);
        let shadow_blur = plain(Some(TitleShadow::BLUR));
        changed |= row(ui, true, "Font", &mut self.font, usual_font());
        changed |= font_list(ui, &format!("{salt}-font"), &mut self.font, families);
        ui.end_row();
        changed |= row(ui, true, "Size", &mut self.size, usual.size);
        ui.end_row();
        if let Some((tracking, spacing)) = spaced {
            changed |= row(ui, true, "Tracking", tracking, "0");
            ui.end_row();
            changed |= row(ui, true, "Line Spacing", spacing, "0");
            ui.end_row();
        }
        changed |= row(ui, true, "Color", &mut self.color, "#ffffff");
        changed |= swatch(ui, &mut self.color, [255, 255, 255]);
        ui.end_row();
        changed |= row(ui, true, "Outline", &mut self.outline, usual.outline);
        changed |= swatch(ui, &mut self.outline, usual.ink);
        ui.end_row();
        changed |= row(
            ui,
            true,
            "Outline size",
            &mut self.outline_size,
            usual.outline_size,
        );
        ui.end_row();
        ui.label("Outside Only");
        changed |= ui.checkbox(&mut self.outside, "").changed();
        ui.end_row();
        if usual.switches {
            changed |= switched(ui, "Show Background", &mut self.box_off);
        }
        let on = !usual.switches || !self.box_off;
        changed |= row(ui, on, "Background", &mut self.background, usual.background);
        changed |= ui
            .add_enabled_ui(on, |ui| swatch(ui, &mut self.background, [0, 0, 0]))
            .inner;
        ui.end_row();
        changed |= row(ui, on, "Opacity", &mut self.opacity, usual.opacity);
        ui.end_row();
        if let Some(s) = shaped {
            let corner = format!("{}%", percent(Some(Title::BOX_CORNER)));
            changed |= row(ui, on, "Box Width", &mut s.width, "Fits Text");
            ui.end_row();
            changed |= row(ui, on, "Box Height", &mut s.height, "Fits Text");
            ui.end_row();
            changed |= row(ui, on, "Box Across", &mut s.across, "0%");
            ui.end_row();
            changed |= row(ui, on, "Box Down", &mut s.down, "0%");
            ui.end_row();
            changed |= row(ui, on, "Box Corner", &mut s.corner, &corner);
            ui.end_row();
            changed |= row(ui, on, "Box Outline", &mut s.outline, "#000000");
            changed |= ui
                .add_enabled_ui(on, |ui| swatch(ui, &mut s.outline, [0, 0, 0]))
                .inner;
            ui.end_row();
            changed |= row(ui, on, "Box Outline Width", &mut s.outline_width, "0");
            ui.end_row();
        }
        if usual.switches {
            changed |= switched(ui, "Show Shadow", &mut self.shadow_off);
        }
        let on = !usual.switches || !self.shadow_off;
        changed |= row(ui, on, "Shadow", &mut self.shadow, usual.shadow);
        changed |= ui
            .add_enabled_ui(on, |ui| swatch(ui, &mut self.shadow, [0, 0, 0]))
            .inner;
        ui.end_row();
        changed |= row(
            ui,
            on,
            "Shadow opacity",
            &mut self.shadow_opacity,
            &shadow_opacity,
        );
        ui.end_row();
        changed |= row(
            ui,
            on,
            "Shadow across",
            &mut self.shadow_across,
            &shadow_across,
        );
        ui.end_row();
        changed |= row(ui, on, "Shadow down", &mut self.shadow_down, &shadow_down);
        ui.end_row();
        changed |= row(ui, on, "Shadow blur", &mut self.shadow_blur, &shadow_blur);
        ui.end_row();
        (changed, enter)
    }

    fn shadow(
        &self,
        stored: Option<&TitleShadow>,
        black_is_usual: bool,
    ) -> Result<Option<TitleShadow>, String> {
        let bare = [
            &self.shadow_opacity,
            &self.shadow_across,
            &self.shadow_down,
            &self.shadow_blur,
        ]
        .iter()
        .all(|f| f.trim().is_empty());
        let color = match field(&self.shadow) {
            Some(color) => match stored {
                Some(h) if h.color.is_none() && color.eq_ignore_ascii_case(SHADOW_INK) => None,
                None if color.eq_ignore_ascii_case(SHADOW_INK) => None,
                _ => Some(color),
            },
            None if black_is_usual && (self.shadow_off || !bare) => None,
            None if bare => return Ok(None),
            None => return Err("a shadow needs a color".into()),
        };
        let offset = stored.and_then(|h| h.offset);
        let size = |i: usize| self.frame.map(|f| f[i]);
        let across = kept_offset(&self.shadow_across, offset.map(|o| o[0]), size(0))?;
        let down = kept_offset(&self.shadow_down, offset.map(|o| o[1]), size(1))?;
        let offset = match (across, down) {
            (None, None) => None,
            (x, y) => Some([
                x.unwrap_or(TitleShadow::OFFSET[0]),
                y.unwrap_or(TitleShadow::OFFSET[1]),
            ]),
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
            off: self.shadow_off.then_some(true),
        }))
    }

    pub(super) fn parts(&self, stored: &Parts, usual_box: Option<&str>) -> Result<Parts, String> {
        let color = match (field(&self.background), usual_box) {
            (Some(color), _) => Some(color),
            (None, Some(usual)) if self.box_off || !self.opacity.trim().is_empty() => {
                Some(usual.to_string())
            }
            (None, _) if self.opacity.trim().is_empty() => None,
            (None, _) => return Err("a background needs a color".into()),
        };
        let background = match color {
            Some(color) => Some(SubtitleBackground {
                color,
                opacity: kept_number(
                    &self.opacity,
                    stored.background.as_ref().and_then(|b| b.opacity),
                    "opacity",
                )?,
                off: self.box_off.then_some(true),
            }),
            None => None,
        };
        let outline_color = field(&self.outline);
        let outline_size = kept_plain(
            &self.outline_size,
            stored.outline.as_ref().and_then(|o| o.size),
            "outline size",
        )?;
        let kept_outside = stored.outline.as_ref().and_then(|o| o.outside);
        let outside = if kept_outside == Some(self.outside) {
            kept_outside
        } else {
            self.outside.then_some(true)
        };
        Ok(Parts {
            font: field(&self.font),
            size: kept_number(&self.size, stored.size, "size")?,
            color: field(&self.color),
            outline: (outline_color.is_some() || outline_size.is_some() || outside.is_some())
                .then_some(SubtitleOutline {
                    color: outline_color,
                    size: outline_size,
                    outside,
                }),
            background,
            shadow: self.shadow(stored.shadow.as_ref(), usual_box.is_some())?,
            frame: stored.frame,
        })
    }
}
