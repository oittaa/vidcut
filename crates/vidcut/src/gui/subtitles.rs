use std::path::PathBuf;

use eframe::egui::{
    self, Align, Color32, FontId, Key, Painter, Rect, Stroke, pos2, text::LayoutJob, vec2,
};
use serde_json::json;
use vidcut_core::model::{Cue, Place, Project, SubtitleTrack};
use vidcut_core::subtitles::{Tag, Token};

use super::export::{Ask, Picked};
use super::folders::Kind;
use super::keys;
use super::workspace::{Workspace, dim};

#[derive(Debug, Clone)]
pub struct CueDialog {
    pub cue: String,
    pub text: String,
    pub place: Place,
    loaded: bool,
    focus: bool,
}

#[derive(Debug, Clone)]
pub struct TrackDialog {
    track: String,
    name: String,
    language: String,
    hint: String,
}

pub(super) enum Files {
    PickImport(Picked),
    Importing(Ask, PathBuf),
    PickExport(Picked, String),
    Exporting(Ask),
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PickImport(_) => "PickImport",
            Self::Importing(..) => "Importing",
            Self::PickExport(..) => "PickExport",
            Self::Exporting(_) => "Exporting",
        })
    }
}

const PLACES: [(Place, &str); 3] = [
    (Place::Bottom, "Bottom"),
    (Place::Middle, "Middle"),
    (Place::Top, "Top"),
];

fn jobs(text: &str, size: f32, width: f32) -> (LayoutJob, LayoutJob) {
    let mut plain = LayoutJob::default();
    let mut bold = LayoutJob::default();
    let mut colors: Vec<Color32> = Vec::new();
    let mut depth = [0usize; 3];
    for token in vidcut_core::subtitles::tokens(text) {
        match token {
            Token::Text(t) => {
                let color = colors.last().copied().unwrap_or(Color32::WHITE);
                let format = |color: Color32| egui::TextFormat {
                    font_id: FontId::proportional(size),
                    line_height: Some(size * 1.31),
                    color,
                    italics: depth[0] > 0,
                    underline: if depth[2] > 0 {
                        Stroke::new((size / 16.0).max(1.0), color)
                    } else {
                        Stroke::NONE
                    },
                    ..Default::default()
                };
                plain.append(&t, 0.0, format(color));
                let shown = if depth[1] > 0 {
                    color
                } else {
                    Color32::TRANSPARENT
                };
                bold.append(&t, 0.0, format(shown));
            }
            Token::Open(Tag::Color(c)) => colors.push(hex(&c).unwrap_or(Color32::WHITE)),
            Token::Close(Tag::Color(_)) => {
                colors.pop();
            }
            Token::Open(tag) => depth[slot(&tag)] += 1,
            Token::Close(tag) => {
                let i = slot(&tag);
                depth[i] = depth[i].saturating_sub(1);
            }
        }
    }
    for job in [&mut plain, &mut bold] {
        job.wrap.max_width = width;
        job.halign = Align::Center;
    }
    (plain, bold)
}

fn slot(tag: &Tag) -> usize {
    match tag {
        Tag::I => 0,
        Tag::B => 1,
        _ => 2,
    }
}

fn hex(c: &str) -> Option<Color32> {
    let c = c.strip_prefix('#')?;
    let v = u32::from_str_radix(c, 16).ok()?;
    (c.len() == 6).then(|| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

pub(super) fn paint_cues(
    painter: &Painter,
    picture: Rect,
    p: &Project,
    frame: i64,
    typing: Option<&CueDialog>,
) {
    let Some(t) = p.subtitles.iter().find(|t| t.shown) else {
        return;
    };
    let size = (picture.height() * 0.055).max(8.0);
    let width = picture.width() * 0.9;
    let on: Vec<&Cue> = t
        .cues
        .iter()
        .filter(|c| c.start <= frame && frame < c.end())
        .collect();
    for (place, _) in PLACES {
        let galleys: Vec<_> = on
            .iter()
            .map(|c| match typing {
                Some(d) if d.cue == c.id => (d.text.as_str(), d.place),
                _ => (c.text.as_str(), c.place),
            })
            .filter(|&(_, at)| at == place)
            .map(|(text, _)| {
                let (plain, bold) = jobs(text, size, width);
                (painter.layout_job(plain), painter.layout_job(bold))
            })
            .collect();
        let total: f32 = galleys.iter().map(|(g, _)| g.rect.height()).sum();
        let middle = match place {
            Place::Top => 0.2,
            Place::Middle => 0.5,
            Place::Bottom => 0.8,
        };
        let mut top = picture.top() + picture.height() * middle - total / 2.0;
        for (plain, bold) in galleys {
            let r = plain.rect;
            let at = pos2(picture.center().x - r.center().x, top - r.top());
            let edge = (size / 20.0).max(1.0);
            for (dx, dy) in [
                (-1.0, -1.0),
                (0.0, -1.0),
                (1.0, -1.0),
                (-1.0, 0.0),
                (1.0, 0.0),
                (-1.0, 1.0),
                (0.0, 1.0),
                (1.0, 1.0),
            ] {
                painter.galley_with_override_text_color(
                    at + vec2(dx, dy) * edge,
                    plain.clone(),
                    Color32::from_black_alpha(220),
                );
            }
            painter.galley(at, plain, Color32::WHITE);
            painter.galley(at + vec2(size / 28.0, 0.0), bold, Color32::WHITE);
            top += r.height();
        }
    }
}

impl Workspace {
    fn typed(&self, d: &CueDialog) -> Option<serde_json::Value> {
        let cue = self.project()?.cue(&d.cue)?;
        let mut params = json!({ "cue": d.cue });
        if vidcut_core::subtitles::normalize(&d.text) != cue.text {
            params["text"] = json!(d.text);
        }
        if d.place != cue.place {
            params["place"] = json!(d.place);
        }
        params
            .as_object()
            .is_some_and(|o| o.len() > 1)
            .then_some(params)
    }

    pub(crate) fn open_cue(&mut self, id: &str) {
        if let Some(d) = &mut self.timeline.cue_dialog
            && d.cue == id
        {
            d.focus = true;
            return;
        }
        if let Some(params) = self
            .timeline
            .cue_dialog
            .as_ref()
            .and_then(|d| self.typed(d))
        {
            self.send("subtitles.set", params);
        }
        let found = self.project().and_then(|p| p.cue(id)).cloned();
        self.timeline.cue_dialog = Some(CueDialog {
            cue: id.to_string(),
            text: found.as_ref().map(|c| c.text.clone()).unwrap_or_default(),
            place: found.as_ref().map(|c| c.place).unwrap_or_default(),
            loaded: found.is_some(),
            focus: true,
        });
    }

    pub(crate) fn open_track_dialog(&mut self, t: &SubtitleTrack) {
        self.timeline.track_dialog = Some(TrackDialog {
            track: t.id.clone(),
            name: t.name.clone().unwrap_or_default(),
            language: t.language.clone().unwrap_or_default(),
            hint: t.display_name(),
        });
    }

    pub(super) fn subtitle_windows(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.timeline.adding.as_ref().and_then(Ask::take) {
            self.timeline.adding = None;
            match result {
                Ok(r) => {
                    if let Some(id) = r.result["cue"].as_str() {
                        let id = id.to_string();
                        self.open_cue(&id);
                    }
                }
                Err(e) => self.say(format!("Add Subtitle: {}", e.message)),
            }
        }
        self.cue_window(ctx);
        self.track_window(ctx);
        self.subtitle_files(ctx);
    }

    fn cue_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.cue_dialog.take() else {
            return;
        };
        let Some(p) = self.project_shared() else {
            return;
        };
        let found = p
            .find_cue(&d.cue)
            .map(|(t, c)| (&p.subtitles[t], &p.subtitles[t].cues[c]));
        match found {
            Some((_, cue)) if !d.loaded => {
                d.text = cue.text.clone();
                d.place = cue.place;
                d.loaded = true;
            }
            None if d.loaded => return,
            _ => {}
        }
        let mut open = true;
        let mut done = false;
        egui::Window::new("Subtitle")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                match found {
                    Some((track, cue)) => ui.label(dim(format!(
                        "{}   {} – {}   ({})",
                        track.display_name(),
                        self.tc(cue.start),
                        self.tc(cue.end()),
                        self.src_tc(cue.duration)
                    ))),
                    None => ui.label(dim("Adding…")),
                };
                let save = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter));
                let r = ui.add(
                    egui::TextEdit::multiline(&mut d.text)
                        .desired_rows(3)
                        .desired_width(380.0)
                        .hint_text("Its text; Enter starts a new line"),
                );
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                ui.label(dim(
                    "<i>, <b>, <u> and <font color=\"#ffcc00\"> around words style them",
                ));
                ui.horizontal(|ui| {
                    ui.label("Position");
                    for (place, label) in PLACES {
                        ui.radio_value(&mut d.place, place, label);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(keys("Done  (Ctrl+Enter)")).clicked() || save {
                        if let Some(params) = self.typed(&d) {
                            self.send("subtitles.set", params);
                        }
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                    if ui
                        .add_enabled(found.is_some(), egui::Button::new("Delete Subtitle"))
                        .clicked()
                    {
                        self.send("subtitles.remove", json!({ "cues": [d.cue] }));
                        done = true;
                    }
                });
            });
        if open && !done {
            self.timeline.cue_dialog = Some(d);
        }
    }

    fn track_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.track_dialog.take() else {
            return;
        };
        let Some(t) = self
            .project()
            .and_then(|p| p.subtitle_track(&d.track))
            .cloned()
        else {
            return;
        };
        let mut open = true;
        let mut done = false;
        egui::Window::new("Subtitle Track")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let mut enter = false;
                egui::Grid::new("subtitle-track-fields").show(ui, |ui| {
                    ui.label("Name");
                    let r = ui.add(egui::TextEdit::singleline(&mut d.name).hint_text(&d.hint));
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.end_row();
                    ui.label("Language");
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut d.language).hint_text("en, pt-BR, …"));
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.end_row();
                });
                ui.label(dim("Exported subtitle files are named with these"));
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() || enter {
                        let mut params = json!({ "track": d.track });
                        if d.name.trim() != t.name.as_deref().unwrap_or_default() {
                            params["name"] = json!(d.name.trim());
                        }
                        if d.language.trim() != t.language.as_deref().unwrap_or_default() {
                            params["language"] = json!(d.language.trim());
                        }
                        if params.as_object().is_some_and(|o| o.len() > 1) {
                            self.send("track.set", params);
                        }
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            });
        if open && !done {
            self.timeline.track_dialog = Some(d);
        }
    }

    pub(super) fn import_subtitle(&mut self, ctx: &egui::Context) {
        let dialogs = self.dialogs.clone();
        let folder = self
            .folders
            .get(Kind::Media)
            .or_else(|| self.project_folder())
            .unwrap_or_else(|| PathBuf::from("."));
        self.timeline.files = Some(Files::PickImport(Picked::ask(ctx, move || {
            dialogs.pick_file(
                "Import Subtitle",
                &folder,
                &["srt", "vtt", "ttml", "dfxp", "xml"],
            )
        })));
    }

    pub(crate) fn export_subtitle(&mut self, ctx: &egui::Context, track: Option<&str>) {
        let Some(p) = self.project() else {
            return;
        };
        let ti = match track {
            Some(t) => p.subtitle_index(t),
            None => vidcut_core::ops::subtitles::shown(p).ok(),
        };
        let Some(t) = ti.map(|i| &p.subtitles[i]) else {
            self.say(
                "Show a subtitle track to export, or use Export Subtitle in its header's menu",
            );
            return;
        };
        let stem = self
            .snap
            .project
            .as_ref()
            .and_then(|s| s.path.file_stem())
            .map_or("subtitles".into(), |s| s.to_string_lossy().into_owned());
        let file = format!("{stem}_{}.srt", t.display_name());
        let id = t.id.clone();
        let dialogs = self.dialogs.clone();
        let folder = self
            .folders
            .get(Kind::Exports)
            .or_else(|| self.project_folder())
            .unwrap_or_else(|| PathBuf::from("."));
        self.timeline.files = Some(Files::PickExport(
            Picked::ask(ctx, move || {
                dialogs.save_file("Export Subtitle", &folder, &file, &["srt", "vtt"])
            }),
            id,
        ));
    }

    fn subtitle_files(&mut self, ctx: &egui::Context) {
        let Some(step) = self.timeline.files.take() else {
            return;
        };
        self.timeline.files = match step {
            Files::PickImport(picked) => match picked.take() {
                None => Some(Files::PickImport(picked)),
                Some(None) => None,
                Some(Some(path)) => {
                    self.folders.remember(Kind::Media, &path);
                    Some(Files::Importing(
                        Ask::send(&self.live, ctx, "subtitles.import", json!({ "path": path })),
                        path,
                    ))
                }
            },
            Files::Importing(ask, path) => match ask.take() {
                None => Some(Files::Importing(ask, path)),
                Some(Ok(r)) => {
                    let name = path
                        .file_name()
                        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    self.say(format!(
                        "Imported {} subtitle(s) from {name} onto {}",
                        r.result["cues"],
                        r.result["track"].as_str().unwrap_or_default()
                    ));
                    let warnings: Vec<String> = r.result["warnings"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|w| w.as_str().map(String::from))
                        .collect();
                    if !warnings.is_empty() {
                        self.show_import_warnings(name, warnings);
                    }
                    None
                }
                Some(Err(e)) => {
                    self.say(format!("Import Subtitle: {}", e.message));
                    None
                }
            },
            Files::PickExport(picked, track) => match picked.take() {
                None => Some(Files::PickExport(picked, track)),
                Some(None) => None,
                Some(Some(mut path)) => {
                    if path.extension().is_none() {
                        path.set_extension("srt");
                    }
                    self.folders.remember(Kind::Exports, &path);
                    Some(Files::Exporting(Ask::send(
                        &self.live,
                        ctx,
                        "subtitles.export",
                        json!({ "out": path, "track": track, "overwrite": true }),
                    )))
                }
            },
            Files::Exporting(ask) => match ask.take() {
                None => Some(Files::Exporting(ask)),
                Some(Ok(r)) => {
                    self.say(format!(
                        "Wrote {} ({} subtitles)",
                        r.result["path"].as_str().unwrap_or_default(),
                        r.result["cues"]
                    ));
                    None
                }
                Some(Err(e)) => {
                    self.say(format!("Export Subtitle: {}", e.message));
                    None
                }
            },
        };
    }
}
