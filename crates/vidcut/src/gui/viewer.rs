//! The single viewer (spec §4): mode buttons, the picture, the source scroll
//! bar with In/Out handles, the transport and the timecode and duration
//! fields.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, RichText, Sense, Stroke, vec2,
};
use serde_json::json;
use vidcut_core::request::ViewerMode;
use vidcut_core::time::{TimeContext, TimeSpec};

use super::app::{App, Drag, TimecodeEntry, dim, fit};
use super::pool::elide;
use super::theme;
use crate::live::PlayerId;

impl App {
    pub(super) fn viewer_panel(&mut self, ui: &mut egui::Ui) {
        let mode = self.snap.viewer;
        let id = PlayerId::of(mode);
        ui.horizontal(|ui| {
            for (m, label, tip) in [
                (
                    ViewerMode::SourceClip,
                    "Source Clip",
                    "The clip opened from the pool",
                ),
                (
                    ViewerMode::SourceTape,
                    "Source Tape",
                    "Every pool clip end to end",
                ),
                (ViewerMode::Timeline, "Timeline", "The program (Q toggles)"),
            ] {
                if ui
                    .selectable_label(mode == m, label)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.send("viewer.set", json!({ "mode": m }));
                }
            }
            ui.separator();
            ui.label(RichText::new(self.viewer_title()).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.duration_field(ui);
            });
        });

        // The picture, in the timeline's aspect: sources are fitted inside it
        // as export will fit them.
        let below = 34.0 + 36.0;
        let avail = ui.available_rect_before_wrap();
        let area = Rect::from_min_max(
            avail.min,
            Pos2::new(avail.max.x, (avail.max.y - below).max(avail.min.y + 40.0)),
        );
        let aspect = self.project().map_or(16.0 / 9.0, |p| {
            p.settings.width as f32 / p.settings.height.max(1) as f32
        });
        let picture = fit(area.shrink(2.0), aspect);
        let ppp = ui.ctx().pixels_per_point();
        let want = [
            (picture.width() * ppp).round() as i32,
            (picture.height() * ppp).round() as i32,
        ];
        if self.videos[id.index()].want != want {
            // The next logic() pass resizes the target and redraws into it.
            self.videos[id.index()].want = want;
            ui.ctx().request_repaint();
        }
        ui.allocate_rect(area, Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(12, 12, 13));
        let player = self.snap.player(id).clone();
        let video = &self.videos[id.index()];
        match (video.texture_id, player.loaded && video.has_frame) {
            (Some(tex), true) => {
                painter.image(
                    tex,
                    picture,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            _ => {
                painter.rect_filled(picture, CornerRadius::ZERO, Color32::BLACK);
                let text = match (&self.snap.preview_error, mode) {
                    (Some(e), _) => format!("No preview: {e}"),
                    (None, ViewerMode::Timeline) => "The timeline is empty".to_string(),
                    (None, ViewerMode::SourceClip) => {
                        "Double-click a clip in the Media Pool to open it".to_string()
                    }
                    (None, ViewerMode::SourceTape) => "The Media Pool is empty".to_string(),
                };
                if !player.loaded {
                    painter.text(
                        picture.center(),
                        Align2::CENTER_CENTER,
                        text,
                        FontId::proportional(14.0),
                        theme::TEXT_DIM,
                    );
                }
            }
        }
        if let Some(e) = &player.error {
            painter.text(
                picture.left_top() + vec2(8.0, 8.0),
                Align2::LEFT_TOP,
                e,
                FontId::proportional(12.0),
                theme::ERROR,
            );
        }

        ui.add_space(4.0);
        if mode.is_source() {
            self.scroll_bar(ui);
        } else {
            self.clip_edge_hint(ui);
        }
        self.transport(ui, id);
    }

    fn viewer_title(&self) -> String {
        let name = |m: &str| {
            self.project()
                .and_then(|p| p.media(m))
                .map_or(m.to_string(), |x| x.name.clone())
        };
        match self.snap.viewer {
            ViewerMode::Timeline => "Timeline".into(),
            _ => match self.snap.source_at() {
                Some((m, _)) => elide(&name(m), 60),
                None => String::new(),
            },
        }
    }

    /// Source Clip and Source Tape: the whole clip or tape, with the playhead
    /// and draggable In/Out handles (p. 680, 692).
    fn scroll_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(2), theme::TRACK);
        let frames = self.snap.source.frames.max(1);
        let x = |f: i64| rect.left() + rect.width() * (f as f32 / frames as f32);
        let frame_at = |px: f32| {
            (((px - rect.left()) / rect.width()).clamp(0.0, 1.0) * frames as f32).round() as i64
        };
        let tape = self.snap.viewer == ViewerMode::SourceTape;
        if tape {
            for e in self.snap.tape.iter() {
                let x0 = x(e.start);
                painter.line_segment(
                    [Pos2::new(x0, rect.top()), Pos2::new(x0, rect.bottom())],
                    Stroke::new(1.0, theme::TEXT_DIM),
                );
                let name = self
                    .project()
                    .and_then(|p| p.media(&e.media))
                    .map_or(e.media.clone(), |m| m.name.clone());
                let room = x(e.start + e.frames) - x0;
                if room > 40.0 {
                    painter.text(
                        Pos2::new(x0 + 3.0, rect.top() + 2.0),
                        Align2::LEFT_TOP,
                        elide(&name, (room / 7.0) as usize),
                        FontId::proportional(10.0),
                        theme::TEXT_DIM,
                    );
                }
            }
        }
        // The shown media's marks, offset to its place on the tape.
        let shown = self.snap.source_at().map(|(m, _)| m.to_string());
        let mut handles = None;
        if let Some(media) = &shown
            && let Some(m) = self.project().and_then(|p| p.media(media))
        {
            let base = if tape {
                self.snap
                    .tape
                    .iter()
                    .find(|e| &e.media == media)
                    .map_or(0, |e| e.start)
            } else {
                0
            };
            let len = self.media_frames(media);
            let (mut a, mut b) = (
                m.marks.mark_in.unwrap_or(0),
                m.marks.mark_out.unwrap_or(len),
            );
            if let Some(Drag::SourceMark {
                media: dm,
                is_in,
                frame,
            }) = &self.drag
                && dm == media
            {
                if *is_in {
                    a = *frame;
                } else {
                    b = *frame;
                }
            }
            if !m.marks.is_empty() || matches!(self.drag, Some(Drag::SourceMark { .. })) {
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(x(base + a), rect.top()),
                        Pos2::new(x(base + b), rect.bottom()),
                    ),
                    CornerRadius::ZERO,
                    theme::MARKED,
                );
            }
            for (f, is_in) in [(a, true), (b, false)] {
                let px = x(base + f);
                let tri = if is_in {
                    vec![
                        Pos2::new(px, rect.bottom() - 9.0),
                        Pos2::new(px + 7.0, rect.bottom()),
                        Pos2::new(px, rect.bottom()),
                    ]
                } else {
                    vec![
                        Pos2::new(px, rect.bottom() - 9.0),
                        Pos2::new(px, rect.bottom()),
                        Pos2::new(px - 7.0, rect.bottom()),
                    ]
                };
                painter.add(egui::Shape::convex_polygon(
                    tri,
                    Color32::WHITE,
                    Stroke::NONE,
                ));
            }
            handles = Some((media.clone(), base, len, x(base + a), x(base + b)));
        }
        let px = x(self.snap.source.position);
        painter.line_segment(
            [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
            Stroke::new(2.0, theme::PLAYHEAD),
        );

        // Press near a handle drags the mark, elsewhere scrubs.
        if resp.drag_started()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            self.drag = match &handles {
                Some((media, _, _, xa, xb))
                    if (pos.x - xa).abs() < 6.0 || (pos.x - xb).abs() < 6.0 =>
                {
                    let is_in = (pos.x - xa).abs() <= (pos.x - xb).abs();
                    Some(Drag::SourceMark {
                        media: media.clone(),
                        is_in,
                        frame: 0,
                    })
                }
                _ => Some(Drag::Scrub(PlayerId::Source)),
            };
        }
        if let Some(pos) = resp.interact_pointer_pos() {
            let f = frame_at(pos.x);
            match &mut self.drag {
                Some(Drag::Scrub(PlayerId::Source)) if resp.dragged() => {
                    if self.scrub != Some((PlayerId::Source, f)) {
                        self.scrub = Some((PlayerId::Source, f));
                        self.seek(PlayerId::Source, f, true);
                    }
                }
                Some(Drag::SourceMark { frame, .. }) => {
                    if let Some((_, base, len, _, _)) = &handles {
                        *frame = (f - base).clamp(0, *len);
                    }
                }
                _ => {}
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Scrub(id)) => {
                    if let Some((_, f)) = self.scrub.take() {
                        self.seek(id, f, false);
                    }
                }
                Some(Drag::SourceMark {
                    media,
                    is_in,
                    frame,
                }) => {
                    let key = if is_in { "in" } else { "out" };
                    self.send("media.marks.set", json!({ "media": media, key: frame }));
                }
                None => {}
            }
        } else if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            self.seek(PlayerId::Source, frame_at(pos.x), false);
        }
    }

    /// Timeline mode has no scroll bar; it shows when the playhead sits on a
    /// clip's first or last frame (p. 681).
    fn clip_edge_hint(&self, ui: &mut egui::Ui) {
        let at = self.snap.timeline.position;
        let text = self
            .project()
            .and_then(|p| p.track("V1"))
            .and_then(|t| t.clip_at(at).map(|i| &t.clips[i]))
            .map(|c| {
                let first = c.start == at;
                let last = c.end() - 1 == at;
                match (first, last) {
                    (true, true) => "◧ first and last frame ◨",
                    (true, false) => "◧ first frame",
                    (false, true) => "last frame ◨",
                    _ => "",
                }
            })
            .unwrap_or_default();
        ui.allocate_ui(vec2(ui.available_width(), 28.0), |ui| {
            ui.centered_and_justified(|ui| ui.label(dim(text)));
        });
    }

    fn transport(&mut self, ui: &mut egui::Ui, id: PlayerId) {
        let player = self.snap.player(id).clone();
        ui.horizontal(|ui| {
            let big = |s: &str| RichText::new(s).size(16.0);
            if ui
                .button(big("⏮"))
                .on_hover_text("Previous edit (Up)")
                .clicked()
            {
                self.jump_edit(id, false);
            }
            if ui.button(big("⏹")).on_hover_text("Stop (K)").clicked() {
                self.send("player.pause", json!({}));
            }
            let play = if player.playing { "⏸" } else { "▶" };
            if ui
                .button(big(play))
                .on_hover_text("Play/stop (Space); J/K/L shuttle")
                .clicked()
            {
                self.send("player.toggle", json!({}));
            }
            if ui
                .button(big("⏭"))
                .on_hover_text("Next edit (Down)")
                .clicked()
            {
                self.jump_edit(id, true);
            }
            ui.separator();
            if ui.button("Mark In").on_hover_text("I").clicked() {
                self.mark(true);
            }
            if ui.button("Mark Out").on_hover_text("O").clicked() {
                self.mark(false);
            }
            if ui.button("Clear").on_hover_text("Alt+X").clicked() {
                self.clear_marks();
            }
            if player.speed != 0.0 && player.speed != 1.0 {
                ui.label(RichText::new(format!("{:+}×", player.speed)).color(theme::ACCENT));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.timecode_field(ui, id);
            });
        });
    }

    fn jump_edit(&self, id: PlayerId, next: bool) {
        let pos = self.snap.player(id).position;
        let points: Vec<i64> = match id {
            PlayerId::Timeline => self
                .project()
                .and_then(|p| p.track("V1"))
                .map(|t| t.edit_points())
                .unwrap_or_default(),
            PlayerId::Source => self.snap.tape.iter().map(|e| e.start).collect(),
        };
        let target = if next {
            points.into_iter().find(|&f| f > pos)
        } else {
            points.into_iter().rev().find(|&f| f < pos)
        };
        let target = target.unwrap_or(if next { self.snap.player(id).frames } else { 0 });
        self.seek(id, target, false);
    }

    /// The playhead timecode; click, `=`, `+` or `-` to type one (p. 688).
    fn timecode_field(&mut self, ui: &mut egui::Ui, id: PlayerId) {
        let position = self.snap.player(id).position;
        let current = match self.snap.viewer {
            ViewerMode::Timeline => self.tc(position),
            _ => self.src_tc(self.snap.source_at().map_or(position, |(_, f)| f)),
        };
        let font = FontId::monospace(18.0);
        if let Some(entry) = &mut self.timecode {
            let r = ui.add(
                egui::TextEdit::singleline(&mut entry.text)
                    .font(font)
                    .desired_width(150.0)
                    .hint_text(&current),
            );
            ui.label(match entry.sign {
                Some(1) => "+",
                Some(_) => "−",
                None => "=",
            });
            if entry.focus {
                r.request_focus();
                entry.focus = false;
            }
            if r.lost_focus() {
                let submit = ui.input(|i| i.key_pressed(Key::Enter));
                let entry = self.timecode.take().expect("editing");
                if submit && let Some(spec) = timecode_input(&entry.text) {
                    let at = match entry.sign {
                        Some(1) => format!("+{spec}"),
                        Some(_) => format!("-{spec}"),
                        None => spec,
                    };
                    self.send("player.seek", json!({ "at": at }));
                }
            }
        } else {
            let r = ui
                .add(egui::Label::new(RichText::new(&current).font(font)).sense(Sense::click()))
                .on_hover_text(
                    "Click (or press =) to go to a timecode; + or - for a relative jump",
                );
            if r.clicked() {
                self.timecode = Some(TimecodeEntry {
                    sign: None,
                    text: String::new(),
                    focus: true,
                });
            }
        }
    }

    /// The marked duration; editing it moves the Out mark (p. 693).
    fn duration_field(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project() else { return };
        let (start, end, media) = match self.snap.source_at() {
            Some((media, _)) => {
                let len = self.media_frames(media);
                let marks = p.media(media).map(|m| m.marks.clone()).unwrap_or_default();
                (
                    marks.mark_in.unwrap_or(0),
                    marks.mark_out.unwrap_or(len),
                    Some(media.to_string()),
                )
            }
            None => match (p.marks.mark_in, p.marks.mark_out) {
                (Some(a), Some(b)) => (a, b, None),
                _ => (0, p.end(), None),
            },
        };
        let current = self.src_tc(end - start);
        if let Some(text) = &mut self.duration_text {
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .font(FontId::monospace(13.0))
                    .desired_width(110.0)
                    .hint_text(&current),
            );
            r.request_focus();
            if r.lost_focus() {
                let submit = ui.input(|i| i.key_pressed(Key::Enter));
                let text = self.duration_text.take().unwrap_or_default();
                let length = timecode_input(&text)
                    .and_then(|spec| TimeSpec::parse(&spec).ok())
                    .and_then(|t| {
                        TimeContext {
                            fps: self.fps(),
                            start_tc: 0,
                            end: 0,
                            mark_in: None,
                            mark_out: None,
                            playhead: None,
                        }
                        .duration(&t)
                        .ok()
                    })
                    .filter(|&d| d > 0);
                if submit && let Some(d) = length {
                    // Out = In + duration.
                    match media {
                        Some(m) => self.send(
                            "media.marks.set",
                            json!({ "media": m, "in": start, "out": start + d }),
                        ),
                        None => self.send("marks.set", json!({ "in": start, "out": start + d })),
                    }
                }
            }
        } else {
            let r = ui
                .add(
                    egui::Label::new(RichText::new(&current).font(FontId::monospace(13.0)))
                        .sense(Sense::click()),
                )
                .on_hover_text("Marked duration; click to change it (moves the Out mark)");
            ui.label(dim("Duration"));
            if r.clicked() {
                self.duration_text = Some(String::new());
            }
        }
    }
}

/// What was typed into a timecode field, as a TimeSpec: digits fill a
/// timecode from the right as in Resolve ("100" is 00:00:01:00, p. 688);
/// anything else (`5s`, `1m30s`, `450f`, `00:01:00:00`) is used as is.
pub(super) fn timecode_input(text: &str) -> Option<String> {
    let t = text.trim().trim_start_matches(['+', '-', '=']);
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        let padded = format!("{t:0>8}");
        let d = &padded[padded.len() - 8..];
        return Some(format!(
            "{}:{}:{}:{}",
            &d[0..2],
            &d[2..4],
            &d[4..6],
            &d[6..8]
        ));
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::timecode_input;

    #[test]
    fn digits_fill_timecodes_from_the_right() {
        assert_eq!(timecode_input("100").as_deref(), Some("00:00:01:00"));
        assert_eq!(timecode_input("+12").as_deref(), Some("00:00:00:12"));
        assert_eq!(timecode_input("1000000").as_deref(), Some("01:00:00:00"));
        assert_eq!(timecode_input("5s").as_deref(), Some("5s"));
        assert_eq!(timecode_input("  ").as_deref(), None);
    }
}
