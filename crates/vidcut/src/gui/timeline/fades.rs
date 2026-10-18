use eframe::egui::{self, Key};
use serde_json::json;
use vidcut_core::model::{Project, Transition};

use super::{EditSel, TrackRef, geometry};
use crate::gui::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct FadeDialog {
    pub track: TrackRef,
    pub at: i64,
    pub seconds: String,
    pub frames: String,
}

pub fn alignment(x: &Transition) -> &'static str {
    match (x.before, x.after) {
        (0, _) => "start",
        (_, 0) => "end",
        _ => "center",
    }
}

pub fn seconds(frames: i64, fps: f64) -> String {
    let s = format!("{:.2}", frames as f64 / fps.max(0.001));
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn frames_of(seconds: &str, fps: f64) -> Option<i64> {
    let s: f64 = seconds.trim().replace(',', ".").parse().ok()?;
    (s > 0.0).then(|| ((s * fps).round() as i64).max(1))
}

fn found(p: &Project, track: TrackRef, at: i64) -> Option<(Transition, bool)> {
    let t = p.track(&track.id())?;
    let x = geometry::picture_transition(p, t, at)?.clone();
    let sound = t
        .transitions
        .iter()
        .any(|y| y.sound && y.from == x.from && y.to == x.to);
    Some((x, sound))
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
        let Some((x, sound)) = self.project().and_then(|p| found(p, track, at)) else {
            return;
        };
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
            json!({ "at": e.at, "track": e.track.id(), "sound": false }),
        );
        true
    }

    pub(crate) fn open_fade_dialog(&mut self) {
        let Some((e, x)) = self.selected_transition() else {
            return;
        };
        let fps = self.project().map_or(30.0, |p| p.settings.fps.as_f64());
        let frames = x.before + x.after;
        self.timeline.fade_dialog = Some(FadeDialog {
            track: e.track,
            at: e.at,
            seconds: seconds(frames, fps),
            frames: frames.to_string(),
        });
    }

    pub(crate) fn transition_menu(&mut self, ui: &mut egui::Ui) {
        let Some((e, x)) = self.selected_transition() else {
            ui.close();
            return;
        };
        if ui.button("Change Duration…").clicked() {
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
            if ui.radio(now == align, label).clicked() {
                self.reshape_transition(e.track, e.at, None, Some(align));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Delete").clicked() {
            self.delete_transition();
            ui.close();
        }
    }

    pub(crate) fn fade_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.timeline.fade_dialog.take() else {
            return;
        };
        let fps = self.project().map_or(30.0, |p| p.settings.fps.as_f64());
        let mut open = true;
        let mut done = false;
        egui::Window::new("Change Transition Duration")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let mut enter = false;
                egui::Grid::new("fade-duration").show(ui, |ui| {
                    ui.label("Seconds");
                    let r = ui.add(egui::TextEdit::singleline(&mut d.seconds).desired_width(90.0));
                    if r.changed()
                        && let Some(f) = frames_of(&d.seconds, fps)
                    {
                        d.frames = f.to_string();
                    }
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.end_row();
                    ui.label("Frames");
                    let r = ui.add(egui::TextEdit::singleline(&mut d.frames).desired_width(90.0));
                    if r.changed()
                        && let Ok(f) = d.frames.trim().parse::<i64>()
                    {
                        d.seconds = seconds(f, fps);
                    }
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.end_row();
                });
                let frames = d.frames.trim().parse::<i64>().ok().filter(|&f| f > 0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                    let change = ui.add_enabled(frames.is_some(), egui::Button::new("Change"));
                    if let Some(f) = frames
                        && (change.clicked() || enter)
                    {
                        self.reshape_transition(d.track, d.at, Some(f), None);
                        done = true;
                    }
                });
            });
        if open && !done {
            self.timeline.fade_dialog = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dissolve_of_45_frames_at_30_fps_reads_1_5_seconds() {
        assert_eq!(super::seconds(45, 30.0), "1.5");
    }

    #[test]
    fn two_seconds_at_25_fps_are_50_frames() {
        assert_eq!(super::frames_of("2", 25.0), Some(50));
    }
}
