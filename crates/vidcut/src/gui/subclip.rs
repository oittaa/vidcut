use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::request::ViewerMode;
use vidcut_core::shortcuts::Action;
use vidcut_core::time::{format_timecode, parse_counter};

use super::viewer::timecode_input;
use super::workspace::Workspace;

const FULL_EXTENTS: &str = "Use Full Clip Extents";
const FIELD: f32 = 96.0;

#[derive(Debug)]
pub(super) struct NewSubclip {
    media: String,
    range: (i64, i64),
    name: String,
    full: bool,
    focus: bool,
}

impl NewSubclip {
    pub(super) fn new(media: &str, range: (i64, i64)) -> Self {
        Self {
            media: media.to_string(),
            range,
            name: String::new(),
            full: false,
            focus: true,
        }
    }
}

#[derive(Debug)]
pub(super) struct EditSubclip {
    media: String,
    title: String,
    origin: i64,
    current: (i64, i64),
    whole: i64,
    start: String,
    end: String,
    full: bool,
}

fn timecode_field(ui: &mut egui::Ui, text: &mut String, enabled: bool) {
    ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(text).desired_width(FIELD),
    );
}

impl Workspace {
    fn source_clip(&self) -> Option<&str> {
        if self.snap.viewer != ViewerMode::SourceClip {
            return None;
        }
        self.snap.source_at().map(|(media, _)| media)
    }

    pub(super) fn marked_range(&self, media: &str) -> (i64, i64) {
        let marks = self
            .project()
            .and_then(|p| p.media(media).map(|m| m.marks.clone()))
            .unwrap_or_default();
        (
            marks.mark_in.unwrap_or(0),
            marks.mark_out.unwrap_or(self.media_frames(media)),
        )
    }

    pub(super) fn open_new_subclip(&mut self) {
        if let Some(media) = self.source_clip().map(str::to_string) {
            self.new_subclip = Some(NewSubclip::new(&media, self.marked_range(&media)));
        }
    }

    pub(super) fn create_subclip_button(&mut self, ui: &mut egui::Ui) {
        let key = self
            .snap
            .bindings
            .first(Action::CreateSubclip)
            .unwrap_or_default();
        let button = egui::Button::new("Create Subclip").shortcut_text(key);
        if ui
            .add_enabled(self.source_clip().is_some(), button)
            .clicked()
        {
            self.open_new_subclip();
            ui.close();
        }
    }

    pub(super) fn subclip_windows_tick(&mut self, ctx: &egui::Context) {
        let mut state = self.new_subclip.take();
        self.subclip_window(ctx, "New Subclip", true, &mut state);
        self.new_subclip = state;
        self.edit_subclip_window(ctx);
    }

    pub(super) fn subclip_window(
        &mut self,
        ctx: &egui::Context,
        title: &str,
        extents: bool,
        state: &mut Option<NewSubclip>,
    ) {
        let Some(mut d) = state.take() else {
            return;
        };
        let mut open = true;
        let (mut create, mut cancel) = (false, false);
        super::dialog::window(ctx, title, &mut open, |ui| {
            let mut enter = false;
            super::dialog::form(ui, "subclip-form", |ui| {
                super::dialog::row(ui, "Name", |ui| {
                    let field = ui.text_edit_singleline(&mut d.name);
                    if d.focus {
                        field.request_focus();
                        d.focus = false;
                    }
                    enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                });
                if extents {
                    super::dialog::row(ui, "", |ui| ui.checkbox(&mut d.full, FULL_EXTENTS));
                }
            });
            let ready = !d.name.trim().is_empty();
            super::dialog::footer(ui, |ui| {
                create = super::dialog::main_button(ui, "Create", ready) || (enter && ready);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if create {
            let (a, b) = d.range;
            let mut params = json!({
                "media": d.media,
                "name": d.name.trim(),
                "bin": self.snap.pool.bin,
            });
            if d.full {
                params["full_extents"] = json!(true);
            } else {
                params["range"] = json!(format!("{a}f..{b}f"));
            }
            self.send("media.subclip", params);
        } else if open && !cancel {
            *state = Some(d);
        }
    }

    pub(super) fn open_edit_subclip(&mut self, media: &str) {
        let Some(p) = self.project() else {
            return;
        };
        let Some((m, current)) = p.media(media).and_then(|m| Some((m, p.subclip_span(m)?))) else {
            return;
        };
        let origin = m
            .info
            .timecode
            .map_or(0.0, |tc| tc.frames_at(p.settings.fps))
            .round() as i64;
        let label =
            |f: i64| format_timecode(f, p.settings.fps, origin, p.settings.counts_drop_frame());
        let edit = EditSubclip {
            media: media.to_string(),
            title: format!("Edit {}", m.name),
            origin,
            current,
            whole: p.file_frames(m),
            start: label(current.0),
            end: label(current.1),
            full: false,
        };
        self.edit_subclip = Some(edit);
    }

    fn typed_frame(&self, text: &str, origin: i64) -> Option<i64> {
        let p = self.project()?;
        let drop = p.settings.counts_drop_frame();
        let spec = timecode_input(text, self.dropped(), p.settings.fps.frame_digits())?;
        parse_counter(&spec, p.settings.fps, drop, origin)
    }

    fn edit_subclip_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.edit_subclip.take() else {
            return;
        };
        let (fps, drop) = self.project().map_or((self.fps(), false), |p| {
            (p.settings.fps, p.settings.counts_drop_frame())
        });
        let label = |f: i64| format_timecode(f, fps, d.origin, drop);
        let (mut now_start, mut now_end) = (label(d.current.0), label(d.current.1));
        let (mut all_start, mut all_end) = (label(0), label(d.whole));
        let typed = match (
            self.typed_frame(&d.start, d.origin),
            self.typed_frame(&d.end, d.origin),
        ) {
            (Some(a), Some(b)) if a < b && a >= 0 && b <= d.whole => Some((a, b)),
            _ => None,
        };
        let changed = match d.full {
            true => d.current != (0, d.whole),
            false => typed.is_some_and(|t| t != d.current),
        };
        let title = d.title.clone();
        let mut open = true;
        let (mut update, mut cancel) = (false, false);
        super::dialog::window(ctx, &title, &mut open, |ui| {
            super::dialog::form(ui, "edit-subclip-form", |ui| {
                super::dialog::row(ui, "Current Subclip", |ui| {
                    ui.label("Starts");
                    timecode_field(ui, &mut now_start, false);
                    ui.label("Ends");
                    timecode_field(ui, &mut now_end, false);
                });
                super::dialog::row(ui, "New Subclip", |ui| {
                    ui.label("Starts");
                    if d.full {
                        timecode_field(ui, &mut all_start, false);
                    } else {
                        timecode_field(ui, &mut d.start, true);
                    }
                    ui.label("Ends");
                    if d.full {
                        timecode_field(ui, &mut all_end, false);
                    } else {
                        timecode_field(ui, &mut d.end, true);
                    }
                });
                super::dialog::row(ui, "", |ui| ui.checkbox(&mut d.full, FULL_EXTENTS));
            });
            super::dialog::footer(ui, |ui| {
                update = super::dialog::main_button(ui, "Update", changed);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if update {
            let mut params = json!({ "media": d.media });
            if d.full {
                params["full_extents"] = json!(true);
            } else if let Some((a, b)) = typed {
                params["range"] = json!(format!("{a}f..{b}f"));
            }
            self.send("media.subclip_set", params);
        } else if open && !cancel {
            self.edit_subclip = Some(d);
        }
    }
}
