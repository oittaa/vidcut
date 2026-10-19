use eframe::egui::{self, Key};
use serde_json::{Value, json};
use vidcut_core::model::{
    SubtitleBackground, SubtitleOutline, Title, TitleAlign, TitleKind, TitleStyle,
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
    across: String,
    down: String,
    error: String,
    families: Vec<String>,
    shown: Option<Title>,
    loaded: bool,
    focus: bool,
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
            across: String::new(),
            down: String::new(),
            error: String::new(),
            families: vidcut_av::burn::families(),
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
        self.align = t.align.unwrap_or_default();
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
        self.across = percent(t.position.map(|p| p[0]));
        self.down = percent(t.position.map(|p| p[1]));
        self.loaded = true;
    }
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
    let position = match (across, down) {
        (Some(x), Some(y)) => Some([x, y]),
        (None, None) => None,
        _ => return Err("a position takes both numbers, or neither for the usual place".into()),
    };
    let align = match d.kind {
        TitleKind::Text => None,
        TitleKind::LowerThird
            if stored.kind == TitleKind::LowerThird && stored.align() == d.align =>
        {
            stored.align
        }
        TitleKind::LowerThird => Some(d.align),
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
        },
        position,
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
    if t.kind == TitleKind::LowerThird && t.align() != stored.align() {
        params["align"] = json!(t.align());
    }
    if t.style != stored.style {
        params["style"] = serde_json::to_value(&t.style).unwrap_or(json!({}));
    }
    if t.position != stored.position {
        params["position"] = json!(t.position.map_or_else(Vec::new, |p| p.to_vec()));
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
                ui.horizontal(|ui| {
                    ui.label("Kind");
                    changed |= ui
                        .radio_value(&mut d.kind, TitleKind::Text, "Text")
                        .changed();
                    changed |= ui
                        .radio_value(&mut d.kind, TitleKind::LowerThird, "Lower Third")
                        .changed();
                });
                ui.add_enabled_ui(d.kind == TitleKind::LowerThird, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Align");
                        for (align, label) in ALIGNS {
                            changed |= ui.radio_value(&mut d.align, align, label).changed();
                        }
                    });
                });
                let r = ui.add(
                    egui::TextEdit::multiline(&mut d.text)
                        .desired_rows(3)
                        .desired_width(380.0)
                        .hint_text("Its text; Enter starts a new line"),
                );
                changed |= r.changed();
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                let lower = d.kind == TitleKind::LowerThird;
                let usual = Title::usual_position(d.kind, d.align);
                let size_hint = if lower {
                    "6% of the picture"
                } else {
                    "8% of the picture"
                };
                let across_hint = format!("{}%", percent(Some(usual[0])));
                let down_hint = format!("{}%", percent(Some(usual[1])));
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
                    changed |= row(ui, "Size", &mut d.size, size_hint);
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
                    changed |= row(ui, "Across", &mut d.across, &across_hint);
                    ui.end_row();
                    changed |= row(ui, "Down", &mut d.down, &down_hint);
                    ui.end_row();
                });
                ui.label(dim(if lower {
                    "Across and Down place the lower third's corner, as percents of the \
                     picture. An empty field keeps the usual look"
                } else {
                    "Across and Down place the middle of the text, as percents of the \
                     picture. An empty field keeps the usual look"
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
