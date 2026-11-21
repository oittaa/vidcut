use eframe::egui::{self, Key, RichText};
use serde_json::json;
use vidcut_core::model::Settings;
use vidcut_core::time::{Rational, TimeSpec, format_timecode};

use super::workspace::Workspace;

#[derive(Debug)]
enum Making {
    Clips(Vec<String>),
    Marks,
    Pool,
}

#[derive(Debug)]
pub(super) struct NewCompound {
    making: Making,
    name: String,
    start_tc: String,
    fps: Rational,
    drop: bool,
    focus: bool,
}

impl NewCompound {
    fn of(making: Making, fps: Rational, drop: bool) -> Self {
        Self {
            making,
            name: String::new(),
            start_tc: format_timecode(0, fps, 0, drop),
            fps,
            drop,
            focus: true,
        }
    }

    pub(super) fn new(clips: Vec<String>, fps: Rational, drop: bool) -> Self {
        Self::of(Making::Clips(clips), fps, drop)
    }

    pub(super) fn of_marks(fps: Rational, drop: bool) -> Self {
        Self::of(Making::Marks, fps, drop)
    }

    pub(super) fn in_pool(fps: Rational, drop: bool) -> Self {
        Self::of(Making::Pool, fps, drop)
    }
}

#[derive(Debug)]
pub(super) struct CompoundStartTc {
    id: String,
    name: String,
    start_tc: String,
    fps: Rational,
    drop: bool,
    focus: bool,
}

impl CompoundStartTc {
    pub(super) fn new(id: &str, name: &str, start_tc: String, fps: Rational, drop: bool) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            start_tc,
            fps,
            drop,
            focus: true,
        }
    }
}

pub(super) fn parse_start_tc(text: &str, fps: Rational, drop_frame: bool) -> Option<i64> {
    let spec = TimeSpec::parse(text.trim()).ok()?;
    matches!(spec, TimeSpec::Timecode(..)).then_some(())?;
    let s = Settings {
        fps,
        drop_frame,
        ..Settings::default()
    };
    s.start_frames(&spec).ok()
}

fn enter_pressed(r: &egui::Response, ui: &egui::Ui) -> bool {
    r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

impl Workspace {
    pub(super) fn new_compound_rate(&self) -> (Rational, bool) {
        self.project().map_or_else(
            || {
                let s = Settings::default();
                (s.fps, s.counts_drop_frame())
            },
            |p| (p.fps(), p.counts_drop_frame()),
        )
    }

    pub(super) fn compound_window_tick(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.new_compound.take() else {
            return;
        };
        let mut open = true;
        let (mut create, mut cancel) = (false, false);
        let valid = parse_start_tc(&d.start_tc, d.fps, d.drop).is_some();
        super::dialog::window(ctx, "New Compound Clip", &mut open, |ui| {
            let mut enter = false;
            super::dialog::form(ui, "compound-form", |ui| {
                super::dialog::row(ui, "Start Timecode", |ui| {
                    let field = ui.text_edit_singleline(&mut d.start_tc);
                    enter |= enter_pressed(&field, ui);
                });
                super::dialog::row(ui, "Name", |ui| {
                    let field = ui.text_edit_singleline(&mut d.name);
                    if d.focus {
                        field.request_focus();
                        d.focus = false;
                    }
                    enter |= enter_pressed(&field, ui);
                });
            });
            super::dialog::footer(ui, |ui| {
                create = super::dialog::main_button(ui, "Create", valid) || (enter && valid);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if create && valid {
            let name = d.name.trim();
            let bin = self.snap.pool.bin.clone();
            let start_tc = d.start_tc.trim();
            let (method, params) = match &d.making {
                Making::Clips(clips) => (
                    "edit.compound",
                    json!({ "clips": clips, "name": name, "bin": bin, "start_tc": start_tc }),
                ),
                Making::Marks => (
                    "edit.compound_range",
                    json!({ "name": name, "bin": bin, "start_tc": start_tc }),
                ),
                Making::Pool => (
                    "media.compound",
                    json!({ "name": name, "bin": bin, "start_tc": start_tc }),
                ),
            };
            self.send(method, params);
        } else if open && !cancel {
            self.new_compound = Some(d);
        }
    }

    pub(super) fn compound_tc_window_tick(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.compound_tc.take() else {
            return;
        };
        let mut open = true;
        let (mut apply, mut cancel) = (false, false);
        let valid = parse_start_tc(&d.start_tc, d.fps, d.drop).is_some();
        super::dialog::window(ctx, "Set New Start Timecode", &mut open, |ui| {
            ui.label(RichText::new(&d.name).strong());
            let mut enter = false;
            super::dialog::form(ui, "compound-tc-form", |ui| {
                super::dialog::row(ui, "Start Timecode", |ui| {
                    let field = ui.text_edit_singleline(&mut d.start_tc);
                    if d.focus {
                        field.request_focus();
                        d.focus = false;
                    }
                    enter |= enter_pressed(&field, ui);
                });
            });
            super::dialog::footer(ui, |ui| {
                apply = super::dialog::main_button(ui, "OK", valid) || (enter && valid);
                cancel = ui.button("Cancel").clicked();
            });
        });
        if apply && valid {
            self.send(
                "compound.set",
                json!({ "compound": d.id, "start_tc": d.start_tc.trim() }),
            );
        } else if open && !cancel {
            self.compound_tc = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use vidcut_core::time::Rational;

    use super::parse_start_tc;

    #[test]
    fn parse_start_tc_reads_non_drop_timecode() {
        let fps_24 = Rational::new(24, 1).unwrap();
        assert_eq!(parse_start_tc("01:02:03:04", fps_24, false), Some(89356));
    }

    #[test]
    fn parse_start_tc_rejects_negative_timecode() {
        let fps_24 = Rational::new(24, 1).unwrap();
        assert_eq!(parse_start_tc("-00:00:01:00", fps_24, false), None);
    }

    #[test]
    fn parse_start_tc_reads_drop_frame_timecode() {
        let ntsc = Rational::new(30000, 1001).unwrap();
        assert_eq!(parse_start_tc("01:01:00;02", ntsc, true), Some(109_692));
    }

    #[test]
    fn parse_start_tc_rejects_skipped_drop_frame_label() {
        let ntsc = Rational::new(30000, 1001).unwrap();
        assert_eq!(parse_start_tc("01:01:00;00", ntsc, true), None);
    }
}
