use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use vidcut_core::keyframes::{PictureValue, ease_of, eases};
use vidcut_core::looks::{LookKind, Styled, TitleLook};
use vidcut_core::model::{
    Clip, Ease, Follow, LowerThirdLine, MotionPath, Project, Title, TitleAlign, TitleAnchor,
    TitleCase, TitleKind, TitleLine, TitlePart,
};

use super::export::Ask;
use super::inspector::{KeyClick, key_buttons, mark, next_key};
use super::looks::{Parts, Picker, ShapeFields, TextFields, Usual, kept_plain, plain};
use super::subtitles::{filled, font_list, kept_number, percent, swatch};
use super::title_browser::LeftTab;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

#[derive(Debug, Default)]
struct LineFields {
    anchor: Option<TitleAnchor>,
    across: String,
    down: String,
    zoom: String,
    rotation: String,
}

impl LineFields {
    fn of(t: &Title) -> Self {
        Self {
            anchor: t.anchor,
            across: percent(t.position.map(|p| p[0])),
            down: percent(t.position.map(|p| p[1])),
            zoom: percent(t.zoom),
            rotation: plain(t.rotation),
        }
    }

    fn of_upper(t: &Title) -> Self {
        let f = Self::of(&t.upper_view());
        match (&t.upper, t.position) {
            (None, None) => Self {
                across: String::new(),
                down: String::new(),
                ..f
            },
            _ => f,
        }
    }
}

#[derive(Debug)]
pub struct TitleDialog {
    clip: String,
    at: i64,
    kind: TitleKind,
    align: TitleAlign,
    text: String,
    fields: TextFields,
    shape: ShapeFields,
    line_spacing: String,
    tracking: String,
    case: TitleCase,
    line: LineFields,
    upper: LineFields,
    title_opacity: String,
    error: String,
    families: Vec<String>,
    shown: Option<Title>,
    loaded: bool,
    focus: bool,
    dirty: bool,
    from: Option<Title>,
    selection: (usize, usize),
    word_color: String,
    word_size: String,
    word_font: String,
    base: Option<Title>,
    picker: Option<Picker>,
}

const CASES: [(TitleCase, &str); 5] = [
    (TitleCase::Mixed, "Mixed"),
    (TitleCase::Upper, "All Caps"),
    (TitleCase::Lower, "All Lowercase"),
    (TitleCase::Small, "Small Caps"),
    (TitleCase::Title, "Title Caps"),
];

const ALIGNS: [(TitleAlign, &str); 3] = [
    (TitleAlign::Left, "Left"),
    (TitleAlign::Middle, "Middle"),
    (TitleAlign::Right, "Right"),
];

impl TitleDialog {
    fn new(clip: &str, t: Option<&Title>) -> Self {
        let mut d = Self {
            clip: clip.to_string(),
            at: 0,
            kind: TitleKind::Text,
            align: TitleAlign::Left,
            text: String::new(),
            fields: TextFields::default(),
            shape: ShapeFields::default(),
            line_spacing: String::new(),
            tracking: String::new(),
            case: TitleCase::Mixed,
            line: LineFields::default(),
            upper: LineFields::default(),
            title_opacity: String::new(),
            error: String::new(),
            families: vidcut_av::burn::families(),
            selection: (0, 0),
            word_color: "#ffd700".to_string(),
            word_size: String::new(),
            word_font: String::new(),
            shown: None,
            loaded: false,
            focus: true,
            dirty: false,
            from: None,
            base: None,
            picker: None,
        };
        if let Some(t) = t {
            d.load(t);
        }
        d
    }

    fn load(&mut self, t: &Title) {
        self.kind = t.kind;
        self.align = t.align();
        self.text = t.text.clone();
        self.fields = TextFields::load(&Parts::of_title(&t.style));
        self.shape = ShapeFields::load(t.style.shape.as_ref());
        self.line_spacing = plain(t.style.line_spacing);
        self.tracking = plain(t.style.tracking);
        self.case = t.style.case.unwrap_or_default();
        self.base = Some(t.clone());
        self.line = LineFields::of(t);
        self.upper = LineFields::of_upper(t);
        self.title_opacity = percent(t.opacity);
        self.loaded = true;
    }
}

impl TitleDialog {
    pub(super) fn clip(&self) -> &str {
        &self.clip
    }

    pub(super) fn previewing(&self) -> Option<(String, Title)> {
        self.shown.clone().map(|t| (self.clip.clone(), t))
    }

    pub(super) fn placed(&mut self, t: &Title) {
        self.line = LineFields::of(t);
        self.upper = LineFields::of_upper(t);
        self.shown = self.shown.is_some().then(|| t.clone());
    }
}

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

fn sampled_at(c: &Clip, t: &Title, at: i64) -> Title {
    t.sampled(c.content_at(at) as f64)
}

fn leads(p: &Project, c: &Clip, now: Option<&Follow>) -> Vec<(Follow, String)> {
    let meets = |x: &Clip| x.start < c.end() && c.start < x.end();
    let led = |x: &Clip| now.is_some_and(|f| f.clip == x.id);
    let has = |x: &&Clip| {
        vidcut_core::ops::motion::takes(x) && !x.motion.is_empty() && (meets(x) || led(x))
    };
    let clips: Vec<&Clip> = p.tracks.iter().flat_map(|t| &t.clips).filter(has).collect();
    let several = clips.len() > 1;
    let entry = |x: &Clip, m: &MotionPath| {
        let follow = Follow {
            clip: x.id.clone(),
            path: m.id.clone(),
        };
        let name = m.name.as_deref().unwrap_or(&m.id);
        if several {
            (follow, format!("{name} ({})", x.id))
        } else {
            (follow, name.to_string())
        }
    };
    let paths = clips.iter().flat_map(|x| x.motion.iter().map(|m| (*x, m)));
    paths.map(|(x, m)| entry(x, m)).collect()
}

fn follow_row(
    ui: &mut egui::Ui,
    leads: &[(Follow, String)],
    now: Option<&Follow>,
) -> Option<Option<Follow>> {
    let mut picked = None;
    let shown = leads.iter().find(|(f, _)| Some(f) == now);
    ui.label("Follow");
    ui.add_enabled_ui(!leads.is_empty() || now.is_some(), |ui| {
        egui::ComboBox::from_id_salt("title-follow")
            .selected_text(shown.map_or("None", |(_, label)| label.as_str()))
            .width(240.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(now.is_none(), "None").clicked() && now.is_some() {
                    picked = Some(None);
                }
                for (f, label) in leads {
                    let on = Some(f) == now;
                    if ui.selectable_label(on, label.as_str()).clicked() && !on {
                        picked = Some(Some(f.clone()));
                    }
                }
            });
    });
    ui.end_row();
    picked
}

fn key_eases(c: &Clip, t: &Title, v: PictureValue, at: i64) -> (Vec<Ease>, Option<Ease>) {
    let keys = t.keys_of(v);
    let found = keys
        .iter()
        .position(|k| c.key_time(k.frame).round() as i64 == at);
    let path = v == PictureValue::Position;
    match found.filter(|_| !(path && t.moves())) {
        Some(i) if path => (eases(keys, i), Some(t.position_ease(keys[i].frame))),
        Some(i) => (eases(keys, i), ease_of(keys, i)),
        None => (Vec::new(), None),
    }
}

fn kept(shown: &str, value: Option<f64>, what: &str) -> Result<Option<f64>, String> {
    kept_number(&filled(shown, value), value, what)
}

fn line_of(d: &TitleDialog, f: &LineFields, stored: &Title) -> Result<TitleLine, String> {
    let across = kept(&f.across, stored.position.map(|p| p[0]), "position")?;
    let down = kept(&f.down, stored.position.map(|p| p[1]), "position")?;
    let scroll = d.kind == TitleKind::Scroll;
    let position = match (across, down) {
        (Some(x), _) if scroll => Some([x, stored.position.map_or(0.5, |p| p[1])]),
        (None, _) if scroll => None,
        (None, None) => None,
        (x, y) => {
            let usual = Title::usual_position(d.kind, d.align);
            Some([x.unwrap_or(usual[0]), y.unwrap_or(usual[1])])
        }
    };
    Ok(TitleLine {
        anchor: f.anchor,
        position,
        zoom: if scroll {
            None
        } else {
            kept(&f.zoom, stored.zoom, "zoom")?
        },
        rotation: if scroll {
            None
        } else if f.rotation.trim().is_empty() {
            stored.rotation
        } else {
            kept_plain(&f.rotation, stored.rotation, "rotation")?
        },
        ..stored.line()
    })
}

fn title_of(d: &TitleDialog, stored: &Title) -> Result<Title, String> {
    let stored = d.base.as_ref().unwrap_or(stored);
    let was = Parts::of_title(&stored.style);
    let mut style = d.fields.refilled(&was).parts(&was, None)?.title();
    style.line_spacing = kept_plain(&d.line_spacing, stored.style.line_spacing, "line spacing")?;
    style.tracking = kept_plain(&d.tracking, stored.style.tracking, "tracking")?;
    style.case = (d.case != TitleCase::Mixed).then_some(d.case);
    style.shape = d.shape.shape(stored.style.shape.as_ref())?;
    let lower = line_of(d, &d.line, stored)?;
    let high = stored.upper_view();
    let upper = line_of(d, &d.upper, &high)?;
    let apart = stored.upper.is_some() || upper != high.line() || lower != stored.line();
    let align = match d.kind {
        k if stored.kind == k && stored.align() == d.align => stored.align,
        _ => Some(d.align),
    };
    let lines = d.kind == TitleKind::LowerThird && stored.kind == TitleKind::LowerThird;
    let title = Title {
        kind: d.kind,
        align,
        anchor: lower.anchor,
        text: d.text.clone(),
        style,
        position: lower.position,
        position_keys: stored.position_keys.clone(),
        position_y: stored.position_y.clone(),
        position_handles: stored.position_handles.clone(),
        zoom: lower.zoom,
        zoom_keys: stored.zoom_keys.clone(),
        rotation: lower.rotation,
        rotation_keys: stored.rotation_keys.clone(),
        opacity: kept(&d.title_opacity, stored.opacity, "title opacity")?,
        opacity_keys: stored.opacity_keys.clone(),
        upper: (lines && apart).then_some(upper),
        follows: stored.follows.clone(),
    };
    vidcut_core::ops::titles::checked(&title).map_err(|e| e.message)
}

fn anchor_row(ui: &mut egui::Ui, f: &mut LineFields, usual: TitleAnchor) -> bool {
    ui.label("Anchor");
    let changed = ui
        .horizontal_wrapped(|ui| anchor_buttons(ui, &mut f.anchor, usual))
        .inner;
    ui.end_row();
    changed
}

pub(super) fn anchor_buttons(
    ui: &mut egui::Ui,
    anchor: &mut Option<TitleAnchor>,
    usual: TitleAnchor,
) -> bool {
    let [across, down] = anchor.unwrap_or(usual).fractions();
    let at = |i: usize| i as f64 / 2.0;
    let mut changed = false;
    for (i, label) in ["Left", "Middle", "Right"].into_iter().enumerate() {
        if ui.radio(across == at(i), label).clicked() && across != at(i) {
            *anchor = Some(TitleAnchor::at([at(i), down]));
            changed = true;
        }
    }
    ui.separator();
    for (i, label) in ["Top", "Middle", "Bottom"].into_iter().enumerate() {
        if ui.radio(down == at(i), label).clicked() && down != at(i) {
            *anchor = Some(TitleAnchor::at([across, at(i)]));
            changed = true;
        }
    }
    changed
}

fn line_changes(params: &mut Value, t: &Title, stored: &Title, part: TitlePart) -> bool {
    if t.anchor != stored.anchor {
        let usual = Title::usual_anchor(t.kind, t.align(), part);
        params["anchor"] = json!(t.anchor.unwrap_or(usual));
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
    t.anchor != stored.anchor
        || t.position != stored.position
        || t.zoom != stored.zoom
        || t.rotation != stored.rotation
}

fn changes(clip: &str, t: &Title, stored: &Title, at: i64) -> Vec<Value> {
    let mut params = json!({ "clip": clip });
    if t.text != stored.text {
        params["text"] = json!(t.text);
    }
    if t.kind != stored.kind {
        params["kind"] = json!(t.kind);
    }
    if t.kind != stored.kind || t.align() != stored.align() {
        params["align"] = json!(t.align());
    }
    if t.style != stored.style {
        params["style"] = serde_json::to_value(&t.style).unwrap_or(json!({}));
    }
    if line_changes(&mut params, t, stored, TitlePart::Lower) && t.upper.is_some() {
        params["line"] = json!(LowerThirdLine::Lower);
    }
    if t.opacity != stored.opacity {
        params["opacity"] = json!(t.opaque());
    }
    let mut out = Vec::new();
    if params.as_object().is_some_and(|o| o.len() > 1) {
        params["at"] = json!(at);
        out.push(params);
    }
    let mut upper = json!({ "clip": clip, "line": LowerThirdLine::Upper, "at": at });
    if t.upper.is_some()
        && line_changes(
            &mut upper,
            &t.upper_view(),
            &stored.upper_view(),
            TitlePart::Upper,
        )
    {
        out.push(upper);
    }
    out
}

impl Workspace {
    fn title_changes(&self, d: &TitleDialog) -> Option<(String, Vec<Value>, Title)> {
        let c = self.project()?.clip(&d.clip)?;
        let stored = sampled_at(c, c.title.as_ref()?, d.at);
        let t = title_of(d, &stored).ok()?;
        let sends = changes(&d.clip, &t, &stored, d.at);
        (!sends.is_empty()).then(|| (d.clip.clone(), sends, t))
    }

    fn title_key(
        &mut self,
        c: &Clip,
        t: &Title,
        (v, line): (PictureValue, Option<LowerThirdLine>),
        click: KeyClick,
        at: i64,
    ) {
        let high = t.upper_view();
        let view = if line == Some(LowerThirdLine::Upper) {
            &high
        } else {
            t
        };
        let times = view
            .keys_of(v)
            .iter()
            .map(|k| c.key_time(k.frame).round() as i64);
        let param = match line {
            Some(LowerThirdLine::Upper) => format!("upper_{}", v.name()),
            Some(LowerThirdLine::Lower) => format!("lower_{}", v.name()),
            None => v.name().to_string(),
        };
        let mut body = json!({ "clips": [c.id], "param": param, "at": at });
        match click {
            KeyClick::Step(dir) => {
                if let Some(to) = next_key(times, at, dir) {
                    self.seek(PlayerId::Timeline, to, false);
                }
            }
            KeyClick::Toggle => {
                let on = times.into_iter().any(|k| k == at);
                self.send(
                    if on {
                        "keyframes.remove"
                    } else {
                        "keyframes.add"
                    },
                    body,
                );
            }
            KeyClick::Ease(e) => {
                body["ease"] = json!(e);
                self.send("keyframes.set", body);
            }
        }
    }

    pub(crate) fn add_title(&mut self, ctx: &egui::Context) {
        let params = json!({ "text": "Basic Title", "at": self.snap.timeline.position });
        self.timeline.adding_title = Some(Ask::send(&self.live, ctx, "titles.add", params));
    }

    pub(crate) fn open_title(&mut self, id: &str) {
        self.left = LeftTab::Inspector;
        self.inspect_title(id, true);
    }

    pub(super) fn inspect_title(&mut self, id: &str, focus: bool) {
        self.close_cue_form();
        self.close_track_form();
        self.inspector_saw_selection();
        if let Some(d) = &mut self.timeline.title_dialog
            && d.clip == id
        {
            d.focus |= focus;
            return;
        }
        self.close_title_form();
        let found = self.stored_title(id);
        self.looks.reread();
        let mut d = TitleDialog::new(id, found.as_ref());
        d.focus = focus;
        self.timeline.title_dialog = Some(d);
    }

    pub(super) fn close_title_form(&mut self) {
        if let Some((clip, sends, t)) = self
            .timeline
            .title_dialog
            .take()
            .filter(|d| d.dirty)
            .and_then(|d| self.title_changes(&d))
        {
            for params in sends {
                self.send("titles.set", params);
            }
            self.title_landing(&clip, &t);
        }
    }

    pub(super) fn title_tick(&mut self) {
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
        self.sync_title_preview();
    }

    pub(super) fn title_form(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(mut d) = self.timeline.title_dialog.take() else {
            return false;
        };
        let Some(p) = self.project_shared() else {
            return false;
        };
        let found = p.find_clip(&d.clip).and_then(|(t, c)| {
            let clip = &p.tracks[t].clips[c];
            Some((&p.tracks[t], clip, clip.title.as_ref()?))
        });
        let position = self.snap.timeline.position;
        if !d.dirty {
            d.at = position;
        }
        let frame = d.at;
        let stored = found.map(|(_, clip, t)| sampled_at(clip, t, d.at));
        match stored.as_ref() {
            Some(t) if !d.loaded || (!d.dirty && d.from.as_ref() != Some(t)) => {
                d.load(t);
                d.from = Some(t.clone());
                d.shown = None;
            }
            None if d.loaded => return false,
            _ => {}
        }
        let mut changed = false;
        ui.label(RichText::new("Title").strong());
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
        if let Some(s) = stored.as_ref() {
            let current = title_of(&d, s).map(|t| Styled::Titles(TitleLook::of(&t)));
            let mut picker = d.picker.take().unwrap_or_else(|| {
                Picker::new(
                    LookKind::Titles,
                    self.looks.of(LookKind::Titles),
                    &Styled::Titles(TitleLook::of(s)),
                )
            });
            if let Some(Styled::Titles(l)) = self.look_picker(ui, &mut picker, current) {
                d.load(&l.on(&d.text));
                changed = true;
            }
            d.picker = Some(picker);
        }
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
            d.line.anchor = None;
            d.upper.anchor = None;
        }
        ui.horizontal(|ui| {
            ui.label("Align");
            for (align, label) in ALIGNS {
                changed |= ui.radio_value(&mut d.align, align, label).changed();
            }
        });
        if d.kind != TitleKind::Scroll {
            ui.horizontal_wrapped(|ui| {
                ui.label("Case");
                for (case, label) in CASES {
                    changed |= ui.radio_value(&mut d.case, case, label).changed();
                }
            });
        }
        let out = egui::TextEdit::multiline(&mut d.text)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
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
            if ui.button(RichText::new("B").strong()).clicked() {
                changed |= wrap(&mut d, "<b>".into(), "</b>");
            }
            if ui.button(RichText::new("I").italics()).clicked() {
                changed |= wrap(&mut d, "<i>".into(), "</i>");
            }
            if ui.button(RichText::new("U").underline()).clicked() {
                changed |= wrap(&mut d, "<u>".into(), "</u>");
            }
            for (label, open, close) in [
                ("Overline", "<o>", "</o>"),
                ("Strike", "<s>", "</s>"),
                ("Super", "<sup>", "</sup>"),
                ("Sub", "<sub>", "</sub>"),
            ] {
                if ui.button(label).clicked() {
                    changed |= wrap(&mut d, open.into(), close);
                }
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
            if ui.button("Plain").clicked() {
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
        let (kind, align) = (d.kind, d.align);
        let usual = Title::usual_position(d.kind, d.align);
        let size_hint = format!(
            "{}% of the picture",
            percent(Some(Title::usual_size(d.kind)))
        );
        let across_hint = format!("{}%", percent(Some(usual[0])));
        let down_hint = format!("{}%", percent(Some(usual[1])));
        let upper_hint = format!("{}%", percent(Some(usual[1] + Title::UPPER_DROP)));
        let following = found.and_then(|(_, _, t)| t.follows.as_ref());
        let paths = found.map_or_else(Vec::new, |(_, clip, _)| leads(&p, clip, following));
        let mut follow = None;
        let mut clicks = Vec::new();
        egui::Grid::new("title-style-fields").show(ui, |ui| {
            changed |= d
                .fields
                .rows(
                    ui,
                    "title",
                    &d.families,
                    &Usual::title(&size_hint),
                    (!scroll).then_some((&mut d.tracking, &mut d.line_spacing)),
                    Some(&mut d.shape),
                )
                .0;
            let row = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str| {
                ui.label(label);
                ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .min_size(egui::vec2(240.0, 0.0)),
                )
                .changed()
            };
            let mut keys = |ui: &mut egui::Ui, v: PictureValue, line: Option<LowerThirdLine>| {
                let Some((_, clip, t)) = found else {
                    return;
                };
                let high = t.upper_view();
                let view = if line == Some(LowerThirdLine::Upper) {
                    &high
                } else {
                    t
                };
                let marks = mark(clip, view.keys_of(v), frame);
                let (eases, now) = key_eases(clip, view, v, frame);
                ui.horizontal(|ui| {
                    if let Some(click) = key_buttons(ui, &marks, &eases, now) {
                        clicks.push(((v, line), click));
                    }
                });
            };
            let mut line_rows =
                |ui: &mut egui::Ui, f: &mut LineFields, line: Option<LowerThirdLine>| {
                    let part = if line == Some(LowerThirdLine::Upper) {
                        TitlePart::Upper
                    } else {
                        TitlePart::Lower
                    };
                    let mut moved = anchor_row(ui, f, Title::usual_anchor(kind, align, part));
                    moved |= row(ui, "Across", &mut f.across, &across_hint);
                    keys(ui, PictureValue::Position, line);
                    ui.end_row();
                    if !scroll {
                        let hint = if line == Some(LowerThirdLine::Upper) {
                            &upper_hint
                        } else {
                            &down_hint
                        };
                        moved |= row(ui, "Down", &mut f.down, hint);
                        ui.end_row();
                        moved |= row(ui, "Zoom", &mut f.zoom, "100%");
                        keys(ui, PictureValue::Zoom, line);
                        ui.end_row();
                        moved |= row(ui, "Rotation", &mut f.rotation, "0°");
                        keys(ui, PictureValue::Rotation, line);
                        ui.end_row();
                    }
                    moved
                };
            if d.kind == TitleKind::LowerThird {
                ui.label(RichText::new("Upper Line").strong());
                ui.end_row();
                changed |= line_rows(ui, &mut d.upper, Some(LowerThirdLine::Upper));
                ui.label(RichText::new("Lower Line").strong());
                ui.end_row();
                changed |= line_rows(ui, &mut d.line, Some(LowerThirdLine::Lower));
            } else {
                changed |= line_rows(ui, &mut d.line, None);
            }
            if !scroll {
                follow = follow_row(ui, &paths, following);
            }
            changed |= row(ui, "Title opacity", &mut d.title_opacity, "100%");
            keys(ui, PictureValue::Opacity, None);
            ui.end_row();
        });
        ui.label(dim(match (d.kind, following.is_some()) {
            (TitleKind::LowerThird, false) => "Across and Down: each line's anchor, in percents.",
            (TitleKind::LowerThird, true) => {
                "Across and Down: each line's anchor from the followed thing, in percents."
            }
            (TitleKind::Scroll, _) => "Across: the anchor's column, in percents of the picture.",
            (TitleKind::Text, false) => {
                "Across and Down: the anchor's place, in percents of the picture."
            }
            (TitleKind::Text, true) => {
                "Across and Down: the anchor's place from the followed thing, in percents."
            }
        }));
        if !d.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &d.error);
        }
        d.dirty |= changed;
        if d.dirty
            && (self.inspector_commits() || follow.is_some())
            && let Some(s) = stored.as_ref()
        {
            match title_of(&d, s) {
                Ok(t) => {
                    let sends = changes(&d.clip, &t, s, d.at);
                    if sends.is_empty() {
                        d.loaded = false;
                    } else {
                        for params in sends {
                            self.send("titles.set", params);
                        }
                        self.title_landing(&d.clip, &t);
                    }
                    d.dirty = false;
                    d.error.clear();
                }
                Err(e) => d.error = e,
            }
        }
        if let Some(to) = follow.filter(|_| !d.dirty) {
            let mut params = json!({ "clip": d.clip });
            if let Some(f) = to {
                params["of"] = json!(f.clip);
                params["path"] = json!(f.path);
            }
            self.send("motion.follow", params);
        }
        ui.separator();
        if changed && let Some(s) = &stored {
            d.shown = title_of(&d, s).ok().filter(|t| t != s);
        }
        if let Some((_, clip, t)) = found {
            for (v, click) in clicks {
                self.title_key(clip, t, v, click, frame);
            }
        }
        self.timeline.title_dialog = Some(d);
        self.sync_title_preview();
        true
    }
}
