use eframe::egui::{self, Key};
use serde_json::{Value, json};
use vidcut_core::model::{
    SubtitleBackground, SubtitleOutline, Title, TitleAlign, TitleKind, TitleShadow, TitleStyle,
};

use super::export::Ask;
use super::keys;
use super::subtitles::{field, font_list, kept_number, percent, swatch};
use super::workspace::{Workspace, dim};

#[derive(Debug, Clone)]
pub struct TitleDialog {
    clip: String,
    kind: TitleKind,
    align: TitleAlign,
    text: String,
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
    across: String,
    down: String,
    zoom: String,
    rotation: String,
    error: String,
    families: Vec<String>,
    shown: Option<Title>,
    loaded: bool,
    focus: bool,
    selection: (usize, usize),
    word_color: String,
    word_size: String,
    word_font: String,
}

const ALIGNS: [(TitleAlign, &str); 3] = [
    (TitleAlign::Left, "Left"),
    (TitleAlign::Middle, "Middle"),
    (TitleAlign::Right, "Right"),
];

impl TitleDialog {
    fn new(clip: &str, t: Option<&Title>) -> Self {
        let mut d = Self {
            clip: clip.to_string(),
            kind: TitleKind::Text,
            align: TitleAlign::Left,
            text: String::new(),
            font: String::new(),
            size: String::new(),
            color: String::new(),
            outline: String::new(),
            outline_size: String::new(),
            background: String::new(),
            opacity: String::new(),
            shadow: String::new(),
            shadow_opacity: String::new(),
            shadow_across: String::new(),
            shadow_down: String::new(),
            shadow_blur: String::new(),
            across: String::new(),
            down: String::new(),
            zoom: String::new(),
            rotation: String::new(),
            error: String::new(),
            families: vidcut_av::burn::families(),
            selection: (0, 0),
            word_color: "#ffd700".to_string(),
            word_size: String::new(),
            word_font: String::new(),
            shown: None,
            loaded: false,
            focus: true,
        };
        if let Some(t) = t {
            d.load(t);
        }
        d
    }

    fn load(&mut self, t: &Title) {
        let s = &t.style;
        self.kind = t.kind;
        self.align = t.align();
        self.text = t.text.clone();
        self.font = s.font.clone().unwrap_or_default();
        self.size = percent(s.size);
        self.color = s.color.clone().unwrap_or_default();
        self.outline = s
            .outline
            .as_ref()
            .and_then(|o| o.color.clone())
            .unwrap_or_default();
        self.outline_size = percent(s.outline.as_ref().and_then(|o| o.size));
        self.background = s
            .background
            .as_ref()
            .map(|b| b.color.clone())
            .unwrap_or_default();
        self.opacity = percent(s.background.as_ref().and_then(|b| b.opacity));
        let shade = s.shadow.as_ref();
        self.shadow = shade.map_or_else(String::new, |h| {
            h.color.clone().unwrap_or_else(|| SHADOW_INK.to_string())
        });
        self.shadow_opacity = percent(shade.and_then(|h| h.opacity));
        self.shadow_across = percent(shade.and_then(|h| h.offset).map(|o| o[0]));
        self.shadow_down = percent(shade.and_then(|h| h.offset).map(|o| o[1]));
        self.shadow_blur = plain(shade.and_then(|h| h.blur));
        self.across = percent(t.position.map(|p| p[0]));
        self.down = percent(t.position.map(|p| p[1]));
        self.zoom = percent(t.zoom);
        self.rotation = plain(t.rotation);
        self.loaded = true;
    }
}

const SHADOW_INK: &str = "#000000";

fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

fn styled(text: &str, (a, b): (usize, usize), open: &str, close: &str) -> String {
    let (a, b) = if a == b {
        (0, text.len())
    } else {
        (byte_at(text, a), byte_at(text, b))
    };
    format!("{}{open}{}{close}{}", &text[..a], &text[a..b], &text[b..])
}

fn unstyled(text: &str, (a, b): (usize, usize)) -> String {
    let (a, b) = if a == b {
        (0, text.len())
    } else {
        (byte_at(text, a), byte_at(text, b))
    };
    let middle = vidcut_core::rich::escape(&vidcut_core::rich::plain(&text[a..b]));
    format!("{}{middle}{}", &text[..a], &text[b..])
}

fn plain(v: Option<f64>) -> String {
    v.map_or_else(String::new, |n| {
        let s = format!("{n:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    })
}

fn kept_plain(shown: &str, stored: Option<f64>, what: &str) -> Result<Option<f64>, String> {
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

fn shadow_of(d: &TitleDialog, stored: Option<&TitleShadow>) -> Result<Option<TitleShadow>, String> {
    let rest = [
        &d.shadow_opacity,
        &d.shadow_across,
        &d.shadow_down,
        &d.shadow_blur,
    ];
    let Some(color) = field(&d.shadow) else {
        return if rest.iter().all(|f| f.trim().is_empty()) {
            Ok(None)
        } else {
            Err("a shadow needs a color".into())
        };
    };
    let color = match stored {
        Some(h) if h.color.is_none() && color.eq_ignore_ascii_case(SHADOW_INK) => None,
        _ => Some(color),
    };
    let offset = stored.and_then(|h| h.offset);
    let across = kept_number(&d.shadow_across, offset.map(|o| o[0]), "shadow offset")?;
    let down = kept_number(&d.shadow_down, offset.map(|o| o[1]), "shadow offset")?;
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
            &d.shadow_opacity,
            stored.and_then(|h| h.opacity),
            "shadow opacity",
        )?,
        offset,
        blur: kept_plain(&d.shadow_blur, stored.and_then(|h| h.blur), "blur")?,
    }))
}

fn title_of(d: &TitleDialog, stored: &Title) -> Result<Title, String> {
    let s = &stored.style;
    let background = match field(&d.background) {
        Some(color) => Some(SubtitleBackground {
            color,
            opacity: kept_number(
                &d.opacity,
                s.background.as_ref().and_then(|b| b.opacity),
                "opacity",
            )?,
        }),
        None if d.opacity.trim().is_empty() => None,
        None => return Err("a background needs a color".into()),
    };
    let outline_color = field(&d.outline);
    let outline_size = kept_number(
        &d.outline_size,
        s.outline.as_ref().and_then(|o| o.size),
        "outline size",
    )?;
    let across = kept_number(&d.across, stored.position.map(|p| p[0]), "position")?;
    let down = kept_number(&d.down, stored.position.map(|p| p[1]), "position")?;
    let scroll = d.kind == TitleKind::Scroll;
    let position = match (across, down) {
        (Some(x), _) if scroll => Some([x, stored.position.map_or(0.5, |p| p[1])]),
        (None, _) if scroll => None,
        (Some(x), Some(y)) => Some([x, y]),
        (None, None) => None,
        _ => return Err("a position takes both numbers, or neither for the usual place".into()),
    };
    let align = match d.kind {
        TitleKind::Text => None,
        k if stored.kind == k && stored.align() == d.align => stored.align,
        _ => Some(d.align),
    };
    let title = Title {
        kind: d.kind,
        align,
        text: d.text.clone(),
        style: TitleStyle {
            font: field(&d.font),
            size: kept_number(&d.size, s.size, "size")?,
            color: field(&d.color),
            outline: (outline_color.is_some() || outline_size.is_some()).then_some(
                SubtitleOutline {
                    color: outline_color,
                    size: outline_size,
                },
            ),
            background,
            shadow: shadow_of(d, s.shadow.as_ref())?,
        },
        position,
        zoom: if scroll {
            None
        } else {
            kept_number(&d.zoom, stored.zoom, "zoom")?
        },
        rotation: if scroll {
            None
        } else {
            kept_plain(&d.rotation, stored.rotation, "rotation")?
        },
    };
    vidcut_core::ops::titles::checked(&title).map_err(|e| e.message)
}

fn changes(clip: &str, t: &Title, stored: &Title) -> Option<Value> {
    let mut params = json!({ "clip": clip });
    if t.text != stored.text {
        params["text"] = json!(t.text);
    }
    if t.kind != stored.kind {
        params["kind"] = json!(t.kind);
    }
    if t.kind != TitleKind::Text && (t.kind != stored.kind || t.align() != stored.align()) {
        params["align"] = json!(t.align());
    }
    if t.style != stored.style {
        params["style"] = serde_json::to_value(&t.style).unwrap_or(json!({}));
    }
    if t.position != stored.position {
        params["position"] = json!(t.position.map_or_else(Vec::new, |p| p.to_vec()));
    }
    if t.zoom != stored.zoom {
        params["zoom"] = json!(t.scale());
    }
    if t.rotation != stored.rotation {
        params["rotation"] = json!(t.turn());
    }
    params
        .as_object()
        .is_some_and(|o| o.len() > 1)
        .then_some(params)
}

impl Workspace {
    fn title_changes(&self, d: &TitleDialog) -> Option<Value> {
        let stored = self.project()?.clip(&d.clip)?.title.as_ref()?;
        let t = title_of(d, stored).ok()?;
        changes(&d.clip, &t, stored)
    }

    pub(crate) fn add_title(&mut self, ctx: &egui::Context) {
        let params = json!({ "text": "Title", "at": self.snap.timeline.position });
        self.timeline.adding_title = Some(Ask::send(&self.live, ctx, "titles.add", params));
    }

    pub(crate) fn open_title(&mut self, id: &str) {
        if let Some(d) = &mut self.timeline.title_dialog
            && d.clip == id
        {
            d.focus = true;
            return;
        }
        if let Some(params) = self
            .timeline
            .title_dialog
            .as_ref()
            .and_then(|d| self.title_changes(d))
        {
            self.send("titles.set", params);
        }
        let found = self
            .project()
            .and_then(|p| p.clip(id))
            .and_then(|c| c.title.clone());
        self.timeline.title_dialog = Some(TitleDialog::new(id, found.as_ref()));
        self.live.preview_title(None);
    }

    pub(super) fn title_windows(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.timeline.adding_title.as_ref().and_then(Ask::take) {
            self.timeline.adding_title = None;
            match result {
                Ok(r) => {
                    if let Some(id) = r.result["clip"].as_str() {
                        let id = id.to_string();
                        self.open_title(&id);
                    }
                }
                Err(e) => self.say(format!("Add Title: {}", e.message)),
            }
        }
        self.title_window(ctx);
    }

    fn title_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.title_dialog.take() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let found = p.find_clip(&d.clip).and_then(|(t, c)| {
            let clip = &p.tracks[t].clips[c];
            Some((&p.tracks[t], clip, clip.title.as_ref()?))
        });
        match found {
            Some((_, _, t)) if !d.loaded => d.load(t),
            None if d.loaded => {
                self.live.preview_title(None);
                return;
            }
            _ => {}
        }
        let stored = found.map(|(_, _, t)| t.clone());
        let mut open = true;
        let mut done = false;
        let mut changed = false;
        egui::Window::new("Title")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                match found {
                    Some((track, clip, _)) => ui.label(dim(format!(
                        "{}   {} – {}   ({})",
                        track.id,
                        self.tc(clip.start),
                        self.tc(clip.end()),
                        self.src_tc(clip.duration)
                    ))),
                    None => ui.label(dim("Adding…")),
                };
                let save = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
                let before = d.kind;
                ui.horizontal(|ui| {
                    ui.label("Kind");
                    changed |= ui
                        .radio_value(&mut d.kind, TitleKind::Text, "Text")
                        .changed();
                    changed |= ui
                        .radio_value(&mut d.kind, TitleKind::LowerThird, "Lower Third")
                        .changed();
                    changed |= ui
                        .radio_value(&mut d.kind, TitleKind::Scroll, "Scroll")
                        .changed();
                });
                if d.kind != before {
                    d.align = Title {
                        kind: d.kind,
                        ..Title::new("")
                    }
                    .align();
                }
                ui.add_enabled_ui(d.kind != TitleKind::Text, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Align");
                        for (align, label) in ALIGNS {
                            changed |= ui.radio_value(&mut d.align, align, label).changed();
                        }
                    });
                });
                let out = egui::TextEdit::multiline(&mut d.text)
                    .desired_rows(3)
                    .desired_width(380.0)
                    .hint_text("Its text; Enter starts a new line")
                    .show(ui);
                if let Some(range) = out.cursor_range {
                    let r = range.as_sorted_char_range();
                    d.selection = (r.start.0, r.end.0);
                }
                let r = out.response.response;
                changed |= r.changed();
                ui.horizontal_wrapped(|ui| {
                    let wrap = |d: &mut TitleDialog, open: String, close: &str| {
                        d.text = styled(&d.text, d.selection, &open, close);
                        d.selection = (0, 0);
                        true
                    };
                    if ui.button("B").on_hover_text("Bold").clicked() {
                        changed |= wrap(&mut d, "<b>".into(), "</b>");
                    }
                    if ui.button("I").on_hover_text("Italic").clicked() {
                        changed |= wrap(&mut d, "<i>".into(), "</i>");
                    }
                    if ui.button("U").on_hover_text("Underline").clicked() {
                        changed |= wrap(&mut d, "<u>".into(), "</u>");
                    }
                    swatch(ui, &mut d.word_color, [255, 215, 0]);
                    if ui.button("Color").clicked()
                        && let Some(c) = vidcut_core::subtitles::color(&d.word_color)
                    {
                        changed |= wrap(&mut d, format!("<font color=\"{c}\">"), "</font>");
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut d.word_size)
                            .hint_text("size %")
                            .desired_width(56.0),
                    );
                    if ui.button("Size").clicked()
                        && let Ok(Some(s)) = kept_number(&d.word_size, None, "size")
                    {
                        changed |= wrap(&mut d, format!("<font size=\"{s}\">"), "</font>");
                    }
                    let mut face = d.word_font.clone();
                    font_list(ui, "title-word-font", &mut face, &d.families);
                    d.word_font = face;
                    if ui.button("Font").clicked() && !d.word_font.trim().is_empty() {
                        let f = vidcut_core::rich::escape(d.word_font.trim()).replace('"', "");
                        changed |= wrap(&mut d, format!("<font face=\"{f}\">"), "</font>");
                    }
                    if ui
                        .button("Plain")
                        .on_hover_text("Takes the styles off the words")
                        .clicked()
                    {
                        d.text = unstyled(&d.text, d.selection);
                        d.selection = (0, 0);
                        changed = true;
                    }
                });
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                let scroll = d.kind == TitleKind::Scroll;
                let usual = Title::usual_position(d.kind, d.align);
                let size_hint = format!(
                    "{}% of the picture",
                    percent(Some(Title::usual_size(d.kind)))
                );
                let across_hint = format!("{}%", percent(Some(usual[0])));
                let down_hint = format!("{}%", percent(Some(usual[1])));
                let shadow_opacity = format!("{}%", percent(Some(TitleShadow::OPACITY)));
                let shadow_across = format!("{}%", percent(Some(TitleShadow::OFFSET[0])));
                let shadow_down = format!("{}%", percent(Some(TitleShadow::OFFSET[1])));
                let shadow_blur = plain(Some(TitleShadow::BLUR));
                egui::Grid::new("title-style-fields").show(ui, |ui| {
                    let row = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str| {
                        ui.label(label);
                        ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text(hint)
                                .min_size(egui::vec2(240.0, 0.0)),
                        )
                        .changed()
                    };
                    changed |= row(ui, "Font", &mut d.font, "Open Sans");
                    changed |= font_list(ui, "title-font", &mut d.font, &d.families);
                    ui.end_row();
                    changed |= row(ui, "Size", &mut d.size, &size_hint);
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
                    changed |= row(ui, "Shadow", &mut d.shadow, "none");
                    changed |= swatch(ui, &mut d.shadow, [0, 0, 0]);
                    ui.end_row();
                    changed |= row(ui, "Shadow opacity", &mut d.shadow_opacity, &shadow_opacity);
                    ui.end_row();
                    changed |= row(ui, "Shadow across", &mut d.shadow_across, &shadow_across);
                    ui.end_row();
                    changed |= row(ui, "Shadow down", &mut d.shadow_down, &shadow_down);
                    ui.end_row();
                    changed |= row(ui, "Shadow blur", &mut d.shadow_blur, &shadow_blur);
                    ui.end_row();
                    changed |= row(ui, "Across", &mut d.across, &across_hint);
                    ui.end_row();
                    if !scroll {
                        changed |= row(ui, "Down", &mut d.down, &down_hint);
                        ui.end_row();
                        changed |= row(ui, "Zoom", &mut d.zoom, "100%");
                        ui.end_row();
                        changed |= row(ui, "Rotation", &mut d.rotation, "0°");
                        ui.end_row();
                    }
                });
                ui.label(dim(match d.kind {
                    TitleKind::LowerThird => {
                        "Across and Down place the lower third's corner, as percents of the \
                         picture; it grows and turns about the point where its lines meet. \
                         A shadow's Across and Down are percents of the picture too, and its \
                         blur from 0 to 100. An empty field keeps the usual look"
                    }
                    TitleKind::Scroll => {
                        "Across places the lines' left edge, middle or right edge by Align, \
                         as a percent of the picture; the lines rise from below the picture \
                         to above it over the clip. A shadow's Across and Down are percents \
                         of the picture, and its blur from 0 to 100. An empty field keeps the \
                         usual look"
                    }
                    TitleKind::Text => {
                        "Across and Down place the middle of the text, as percents of the \
                         picture; it grows and turns about that point. A shadow's Across and \
                         Down are percents of the picture too, and its blur from 0 to 100. An \
                         empty field keeps the usual look"
                    }
                }));
                if !d.error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, &d.error);
                }
                ui.horizontal(|ui| {
                    if ui.button(keys("Done  (Ctrl+Enter)")).clicked() || save {
                        match stored.as_ref().map(|s| (title_of(&d, s), s)) {
                            Some((Ok(t), s)) => {
                                if let Some(params) = changes(&d.clip, &t, s) {
                                    self.send("titles.set", params);
                                }
                                done = true;
                            }
                            Some((Err(e), _)) => d.error = e,
                            None => done = true,
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            });
        if changed && let Some(s) = &stored {
            let preview = title_of(&d, s).ok().filter(|t| t != s);
            if preview != d.shown {
                self.live
                    .preview_title(preview.clone().map(|t| (d.clip.clone(), t)));
                d.shown = preview;
            }
        }
        if open && !done {
            self.timeline.title_dialog = Some(d);
        } else {
            self.live.preview_title(None);
        }
    }
}
