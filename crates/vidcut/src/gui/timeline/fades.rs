use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::model::{Project, Transition};
use vidcut_core::time::{Rational, format_timecode, parse_media_timecode};

use super::{EditSel, TrackRef, geometry};
use crate::gui::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct FadeDialog {
    pub track: TrackRef,
    pub at: i64,
    pub text: String,
}

pub fn alignment(x: &Transition) -> &'static str {
    match (x.before, x.after) {
        (0, _) => "start",
        (_, 0) => "end",
        _ => "center",
    }
}

pub fn shown(frames: i64, fps: Rational, in_frames: bool) -> String {
    if in_frames {
        frames.to_string()
    } else {
        format_timecode(frames, fps, 0, false)
    }
}

pub fn typed(text: &str, fps: Rational, in_frames: bool) -> Option<i64> {
    let text = text.trim();
    let frames = if in_frames {
        text.parse().ok()?
    } else {
        parse_media_timecode(text, fps)?
    };
    (frames > 0).then_some(frames)
}

fn found(p: &Project, track: TrackRef, at: i64) -> Option<Transition> {
    geometry::picture_transition(p, p.track(&track.id())?, at).cloned()
}

impl Workspace {
    pub(crate) fn selected_transition(&self) -> Option<(EditSel, Transition)> {
        let e = self.timeline.edit?;
        let x = self.trim_transition(self.project()?, e)?;
        Some((e, x))
    }

    fn reshape_transition(
        &self,
        track: TrackRef,
        at: i64,
        length: Option<i64>,
        align: Option<&str>,
    ) {
        let Some(x) = self.project().and_then(|p| found(p, track, at)) else {
            return;
        };
        let sound = self
            .project()
            .and_then(|p| p.track(&track.id()))
            .is_some_and(|t| {
                t.transitions
                    .iter()
                    .any(|y| y.sound && y.from == x.from && y.to == x.to)
            });
        let length = length.unwrap_or(x.before + x.after);
        self.send(
            "edit.dissolve",
            json!({
                "at": at,
                "track": track.id(),
                "length": format!("{length}f"),
                "align": align.unwrap_or(alignment(&x)),
                "sound": sound,
            }),
        );
    }

    pub(crate) fn delete_transition(&self) -> bool {
        let Some((e, _)) = self.selected_transition() else {
            return false;
        };
        self.send(
            "edit.remove_transition",
            json!({ "at": e.at, "track": e.track.id() }),
        );
        true
    }

    pub(crate) fn open_fade_dialog(&mut self) {
        let Some((e, x)) = self.selected_transition() else {
            return;
        };
        let Some(fps) = self.project().map(|p| p.settings.fps) else {
            return;
        };
        self.timeline.fade_dialog = Some(FadeDialog {
            track: e.track,
            at: e.at,
            text: shown(x.before + x.after, fps, self.timeline.fade_in_frames),
        });
    }

    pub(crate) fn transition_menu(&mut self, ui: &mut egui::Ui) {
        let Some((e, x)) = self.selected_transition() else {
            ui.close();
            return;
        };
        if ui.button("Change Transition Duration…").clicked() {
            self.open_fade_dialog();
            ui.close();
        }
        ui.separator();
        let now = alignment(&x);
        for (align, label) in [
            ("start", "Start on Edit"),
            ("center", "Center on Edit"),
            ("end", "End on Edit"),
        ] {
            if ui
                .add_enabled(now != align, egui::Button::new(label))
                .clicked()
            {
                self.reshape_transition(e.track, e.at, None, Some(align));
                ui.close();
            }
        }
    }

    pub(crate) fn fade_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.fade_dialog.take() else {
            return;
        };
        let Some(fps) = self.project().map(|p| p.settings.fps) else {
            return;
        };
        let mut in_frames = self.timeline.fade_in_frames;
        let mut open = true;
        let mut apply = None;
        let mut done = false;
        egui::Window::new("Change Transition Duration")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("fade-duration").show(ui, |ui| {
                    ui.label("Format");
                    ui.horizontal(|ui| {
                        for (frames, label) in [(false, "Time"), (true, "Frames")] {
                            if ui.selectable_label(in_frames == frames, label).clicked()
                                && in_frames != frames
                            {
                                if let Some(f) = typed(&d.text, fps, in_frames) {
                                    d.text = shown(f, fps, frames);
                                }
                                in_frames = frames;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Duration");
                    let r = ui.add(egui::TextEdit::singleline(&mut d.text).desired_width(110.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        apply = typed(&d.text, fps, in_frames);
                        done = apply.is_some();
                    }
                    ui.end_row();
                    ui.label("Preset");
                    ui.horizontal(|ui| {
                        for secs in [1, 5, 15] {
                            let frames = fps.secs_to_frames(f64::from(secs));
                            let label = if in_frames {
                                frames.to_string()
                            } else {
                                format!("{secs}s")
                            };
                            if ui.button(label).clicked() {
                                apply = Some(frames);
                                done = true;
                            }
                        }
                        ui.add_enabled(false, egui::Button::new("End"));
                    });
                    ui.end_row();
                });
                let frames = typed(&d.text, fps, in_frames);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                    if ui
                        .add_enabled(frames.is_some(), egui::Button::new("Change"))
                        .clicked()
                    {
                        apply = frames;
                        done = true;
                    }
                });
            });
        self.timeline.fade_in_frames = in_frames;
        if let Some(f) = apply {
            self.reshape_transition(d.track, d.at, Some(f), None);
        }
        if open && !done {
            self.timeline.fade_dialog = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use vidcut_core::time::Rational;

    #[test]
    fn a_one_second_dissolve_reads_as_a_timecode_in_time_format() {
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(super::shown(30, fps, false), "00:00:01:00");
    }

    #[test]
    fn a_typed_timecode_of_two_seconds_at_25_fps_is_50_frames() {
        let fps = Rational::new(25, 1).unwrap();
        assert_eq!(super::typed("00:00:02:00", fps, false), Some(50));
    }
}
