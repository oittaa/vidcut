//! The single viewer (spec §4): mode buttons, the picture, the source scroll
//! bar with In/Out handles, the transport and the timecode and duration
//! fields.

use std::collections::HashSet;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, RichText, Sense, Stroke,
    pos2, vec2,
};
use serde_json::json;
use vidcut_core::model::{Media, Project, TrackKind};
use vidcut_core::request::ViewerMode;
use vidcut_core::time::{TimeContext, TimeSpec};

use super::pool::elide;
use super::theme;
use super::workspace::{Drag, TimecodeEntry, Workspace, dim, fit};
use crate::live::PlayerId;

/// What the viewer's two-up shows: the Trim Editor's or Dynamic Trim's
/// two frames.
#[derive(Clone)]
pub struct TwoUp {
    pub mode: TwoUpMode,
    /// The left cell and the right; `None` where no clip is.
    pub cells: [Option<TwoUpCell>; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwoUpMode {
    TrimEditor,
    DynamicTrim,
}

impl std::fmt::Display for TwoUpMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TrimEditor => "trim_editor",
            Self::DynamicTrim => "dynamic_trim",
        })
    }
}

#[derive(Clone)]
pub struct TwoUpCell {
    /// The timeline frame it shows, and the source frame of its clip there.
    pub frame: i64,
    pub source_frame: i64,
    /// The picture is that frame's: `false` while it decodes, the cell
    /// showing the one before.
    pub exact: bool,
    /// The picture shown, as decoded.
    pub picture: Option<std::sync::Arc<vidcut_av::player::Picture>>,
}

/// In/Out handles on the source scroll bar.
enum MarkHandles {
    Clip {
        media: String,
        base: i64,
        len: i64,
        in_frame: i64,
        out_frame: i64,
        xa: f32,
        xb: f32,
    },
    Tape {
        in_frame: i64,
        out_frame: i64,
        xa: f32,
        xb: f32,
    },
}

impl MarkHandles {
    fn near(&self, px: f32) -> bool {
        (px - self.xa()).abs() < 6.0 || (px - self.xb()).abs() < 6.0
    }

    fn grabs_in(&self, px: f32) -> bool {
        (px - self.xa()).abs() <= (px - self.xb()).abs()
    }

    fn xa(&self) -> f32 {
        match self {
            Self::Clip { xa, .. } | Self::Tape { xa, .. } => *xa,
        }
    }

    fn xb(&self) -> f32 {
        match self {
            Self::Clip { xb, .. } | Self::Tape { xb, .. } => *xb,
        }
    }
}

/// The media the viewer shows at `position` if its file is missing: the
/// source's, or the picture on top at the timeline's playhead.
fn offline_media<'a>(
    p: &'a Project,
    missing: &HashSet<String>,
    mode: ViewerMode,
    position: i64,
    source: Option<&str>,
) -> Option<&'a Media> {
    let media = match mode {
        ViewerMode::Timeline => p
            .tracks
            .iter()
            .rev()
            .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
            .find_map(|t| {
                t.clip_at(position)
                    .map(|i| &t.clips[i])
                    .filter(|c| c.enabled && c.video)
            })?
            .media
            .as_str(),
        ViewerMode::SourceClip | ViewerMode::SourceTape => source?,
    };
    p.media(media).filter(|_| missing.contains(media))
}

impl Workspace {
    fn mark_handles(&self, painter: &egui::Painter, rect: Rect, xa: f32, xb: f32) {
        for (px, is_in) in [(xa, true), (xb, false)] {
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
    }

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
        // The audio meter and monitoring mute on the right: one
        // bar per project channel.
        let meter_w = self.viewer_meter_width();
        let area = Rect::from_min_max(
            avail.min,
            Pos2::new(
                avail.max.x - meter_w - 6.0,
                (avail.max.y - below).max(avail.min.y + 40.0),
            ),
        );
        let meter = Rect::from_min_max(
            Pos2::new(avail.max.x - meter_w, area.top()),
            Pos2::new(avail.max.x, area.bottom()),
        );
        self.viewer_meter(ui, meter);
        let aspect = self.project().map_or(16.0 / 9.0, |p| {
            p.settings.width as f32 / p.settings.height.max(1) as f32
        });
        let ppp = ui.ctx().pixels_per_point();
        // On whole physical pixels: the player's picture is drawn at this
        // size and shown one to one.
        let picture = {
            use eframe::emath::GuiRounding;
            fit(area.shrink(2.0), aspect).round_to_pixels(ppp)
        };
        self.want = [
            (picture.width() * ppp).round() as i32,
            (picture.height() * ppp).round() as i32,
        ];
        self.two_up = None;
        let dynamic = self.timeline.dynamic.is_some() && self.dynamic_target().is_some();
        if !dynamic
            && self.trim_editor.is_none()
            && let Some(cells) = &mut self.cells
        {
            cells.close();
        }
        if dynamic {
            self.dynamic_view(ui, area);
            return;
        }
        if self.trim_editor.is_some() {
            self.trim_editor_view(ui, area);
            return;
        }
        let area_resp = ui.allocate_rect(area, Sense::click_and_drag());
        // Dragging the source picture onto the timeline edits it in (spec §7.3).
        if mode.is_source()
            && area_resp.drag_started()
            && let Some((media, _)) = self.snap.source_at()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            let media = media.to_string();
            let frames = self.media_frames(&media);
            let marks = self
                .project()
                .and_then(|p| p.media(&media).map(|m| m.marks.clone()))
                .unwrap_or_default();
            let range = (marks.mark_in.unwrap_or(0), marks.mark_out.unwrap_or(frames));
            self.start_media_drag(&media, range, press);
        }
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(12, 12, 13));
        let player = self.snap.player(id).clone();
        let video = self.picture;
        let played = video.texture.filter(|_| player.loaded && video.has_frame);
        match played {
            Some(tex) => {
                painter.image(
                    tex,
                    picture,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
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
                        theme::colors().text_dim,
                    );
                }
            }
        }
        let offline = self.project().and_then(|p| {
            let source = self.snap.source_at().map(|(m, _)| m);
            offline_media(p, &self.snap.missing, mode, player.position, source)
                .map(|m| m.name.clone())
        });
        if let Some(name) = offline {
            painter.rect_filled(picture, CornerRadius::ZERO, theme::colors().offline);
            let text = RichText::new(format!("Media Offline\n{name}"))
                .size(18.0)
                .color(Color32::WHITE);
            ui.put(
                Rect::from_center_size(picture.center(), vec2(picture.width(), 60.0)),
                egui::Label::new(text)
                    .halign(Align::Center)
                    .selectable(false),
            );
        } else if let Some(e) = &player.error {
            painter.text(
                picture.left_top() + vec2(8.0, 8.0),
                Align2::LEFT_TOP,
                e,
                FontId::proportional(12.0),
                theme::colors().error,
            );
        }
        area_resp.context_menu(|ui| {
            let mut shown = self.snap.stats;
            if ui.checkbox(&mut shown, "Stats for Nerds").clicked() {
                self.send("player.stats", json!({ "show": shown }));
                ui.close();
            }
        });
        if let Some(stats) = self.stats.as_ref().filter(|_| self.snap.stats) {
            stats_panel(&painter, picture, stats);
        }

        ui.add_space(4.0);
        if mode.is_source() {
            self.scroll_bar(ui);
        } else {
            self.clip_edge_hint(ui);
        }
        self.transport(ui, id);
    }

    /// The Trim Editor: the outgoing clip's last frame beside the
    /// incoming clip's first frame, with one-frame trims of the Out, the In
    /// and both (a roll). Esc or Close leaves it.
    fn trim_editor_view(&mut self, ui: &mut egui::Ui, area: Rect) {
        use crate::gui::timeline::Side;
        let (Some(e), Some(p)) = (self.timeline.edit, self.shown_project()) else {
            self.trim_editor = None;
            return;
        };
        // A trim on its way moves the selection before the cut moves, so a
        // missing cut doesn't close the editor; a cut that is gone clears the
        // selection, which does.
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.trim_editor = None;
            return;
        }
        self.draw_two_up(
            ui,
            area,
            &p,
            &e.track.id(),
            TwoUpMode::TrimEditor,
            [(e.at - 1, "Out (outgoing)"), (e.at, "In (incoming)")],
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (side, label) in [(Side::Out, "Out"), (Side::Both, "Roll"), (Side::In, "In")] {
                ui.label(dim(label));
                for by in [-1, 1] {
                    if ui
                        .button(if by < 0 { "−1" } else { "+1" })
                        .on_hover_text(format!("{label} {by:+} frame"))
                        .clicked()
                    {
                        self.timeline.edit = Some(crate::gui::timeline::EditSel {
                            at: e.at,
                            side,
                            track: e.track,
                        });
                        self.nudge_edit(side, by);
                    }
                }
                ui.separator();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").on_hover_text("Esc").clicked() {
                    self.trim_editor = None;
                }
                ui.label(RichText::new(format!("edit at {}", self.tc(e.at))).monospace());
            });
        });
    }

    /// Dynamic Trim's two-up: the frames on both sides of the
    /// edit (or of the slipped or sliding clip) as the trim so far leaves them.
    fn dynamic_view(&mut self, ui: &mut egui::Ui, area: Rect) {
        use crate::gui::timeline::tools::{DynamicTarget, dynamic_preview};
        let (Some(target), Some(p)) = (self.dynamic_target(), self.shown_project()) else {
            return;
        };
        let mut p = Project::clone(&p);
        let pending = self.timeline.dynamic.as_ref().map_or(0, |d| d.pending);
        let mode = self.timeline.options.dynamic_mode;
        let Ok((left, right)) = dynamic_preview(&mut p, &target, mode, pending) else {
            return;
        };
        let track = match &target {
            DynamicTarget::Edit(e) => e.track.id(),
            DynamicTarget::Clips(ids) => ids
                .first()
                .and_then(|id| crate::gui::timeline::TrackRef::of_clip(&p, id))
                .unwrap_or(crate::gui::timeline::TrackRef::MAIN)
                .id(),
        };
        let labels = match &target {
            DynamicTarget::Edit(_) => ["Out (outgoing)", "In (incoming)"],
            DynamicTarget::Clips(_) => match mode {
                crate::gui::timeline::tools::DynamicMode::Slip => ["First frame", "Last frame"],
                crate::gui::timeline::tools::DynamicMode::Slide => ["Before", "After"],
            },
        };
        self.draw_two_up(
            ui,
            area,
            &p,
            &track,
            TwoUpMode::DynamicTrim,
            [(left, labels[0]), (right, labels[1])],
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Dynamic Trim {pending:+} frames"))
                    .monospace()
                    .color(Color32::from_rgb(240, 200, 40)),
            );
            match self.timeline.dynamic.as_ref().and_then(|d| d.stopped.as_ref()) {
                Some(why) => {
                    let back = if pending > 0 { "J" } else { "L" };
                    ui.label(
                        RichText::new(format!("Stopped here: {why}. K applies, {back} goes back"))
                            .color(theme::colors().error),
                    )
                }
                None => ui.label(dim("L/J trim (repeat for speed), K stops and applies, K+L/J one frame, Space plays around it, W leaves")),
            };
        });
    }

    /// Two exact frames of `p` side by side, labeled: timeline frames that
    /// show nothing read "no clip".
    fn draw_two_up(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        p: &vidcut_core::model::Project,
        track: &str,
        mode: TwoUpMode,
        cells: [(i64, &str); 2],
    ) {
        let aspect = p.settings.width as f32 / p.settings.height.max(1) as f32;
        ui.allocate_rect(area, Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(12, 12, 13));
        let half = area.width() / 2.0;
        let ppp = ui.ctx().pixels_per_point();
        // The trim's run, × real time.
        let speed = match mode {
            TwoUpMode::DynamicTrim => self.timeline.dynamic.as_ref().map_or(0.0, |d| d.speed()),
            TwoUpMode::TrimEditor => 0.0,
        };
        let gpu_pictures = self.screens.as_ref().map(|s| s.gpu_pictures());
        let ctx = ui.ctx().clone();
        let players = self
            .cells
            .get_or_insert_with(|| super::cells::Cells::new(ctx, gpu_pictures.unwrap_or_default()));
        let track = p.track(track);
        let mut shown = [None, None];
        for (i, (frame, label)) in cells.into_iter().enumerate() {
            let cell = Rect::from_min_size(
                pos2(area.left() + half * i as f32, area.top()),
                vec2(half, area.height()),
            );
            let picture = fit(cell.shrink(6.0), aspect);
            painter.rect_filled(picture, CornerRadius::ZERO, Color32::BLACK);
            let clip =
                track.and_then(|t| t.clips.iter().find(|c| c.start <= frame && frame < c.end()));
            let Some(c) = clip else {
                painter.text(
                    picture.center(),
                    Align2::CENTER_CENTER,
                    "no clip",
                    FontId::proportional(12.0),
                    theme::colors().text_dim,
                );
                continue;
            };
            // The clip it names, whatever covers it on the tracks above.
            let piece = vidcut_core::render::clip_frame(p, &c.id, frame).ok();
            if let Some(piece) = piece {
                let size = [picture.width(), picture.height()].map(|x| (x * ppp).round() as u32);
                let got = players.show(i, p, &piece, size, speed);
                let texture = got
                    .as_ref()
                    .zip(self.screens.as_mut())
                    .and_then(|(g, s)| s.draw(i, &g.picture, size));
                if let Some(texture) = texture {
                    painter.image(
                        texture,
                        picture,
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                shown[i] = Some(TwoUpCell {
                    frame,
                    source_frame: c.source_at(frame),
                    exact: got.as_ref().is_some_and(|g| g.exact),
                    picture: got.map(|g| g.picture),
                });
            }
            let name = c
                .name
                .clone()
                .or_else(|| p.media(&c.media).map(|m| m.name.clone()))
                .unwrap_or_default();
            painter.text(
                picture.left_top() + vec2(6.0, 6.0),
                Align2::LEFT_TOP,
                format!("{label}: {}  src {}", elide(&name, 28), c.source_at(frame)),
                FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
        self.two_up = Some(TwoUp { mode, cells: shown });
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
    /// and draggable In/Out handles.
    fn scroll_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(2), theme::colors().track);
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
                    Stroke::new(1.0, theme::colors().text_dim),
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
                        theme::colors().text_dim,
                    );
                }
            }
        }
        // A span across clips is drawn in tape frames (decision D17). One
        // clip keeps that clip's own marks, offset onto the tape.
        let span = tape.then(|| self.crossing_span()).flatten();
        let shown = self.snap.source_at().map(|(m, _)| m.to_string());
        let mut handles = None;
        if let Some((mut a, mut b)) = span {
            if let Some(Drag::TapeMark { is_in, frame }) = &self.drag {
                if *is_in {
                    a = *frame;
                } else {
                    b = *frame;
                }
            }
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x(a), rect.top()), Pos2::new(x(b), rect.bottom())),
                CornerRadius::ZERO,
                theme::colors().marked,
            );
            self.mark_handles(&painter, rect, x(a), x(b));
            handles = Some(MarkHandles::Tape {
                in_frame: a,
                out_frame: b,
                xa: x(a),
                xb: x(b),
            });
        } else if let Some(media) = &shown
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
                    theme::colors().marked,
                );
            }
            self.mark_handles(&painter, rect, x(base + a), x(base + b));
            handles = Some(MarkHandles::Clip {
                media: media.clone(),
                base,
                len,
                in_frame: a,
                out_frame: b,
                xa: x(base + a),
                xb: x(base + b),
            });
        }
        let px = x(self.snap.source.position);
        painter.line_segment(
            [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
            Stroke::new(2.0, theme::colors().playhead),
        );

        // Press near a handle drags the mark, elsewhere scrubs.
        if resp.drag_started()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            self.drag = match &handles {
                Some(h) if h.near(pos.x) => {
                    let is_in = h.grabs_in(pos.x);
                    match h {
                        MarkHandles::Clip {
                            media,
                            in_frame,
                            out_frame,
                            ..
                        } => Some(Drag::SourceMark {
                            media: media.clone(),
                            is_in,
                            frame: if is_in { *in_frame } else { *out_frame },
                        }),
                        MarkHandles::Tape {
                            in_frame,
                            out_frame,
                            ..
                        } => Some(Drag::TapeMark {
                            is_in,
                            frame: if is_in { *in_frame } else { *out_frame },
                        }),
                    }
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
                    if let Some(MarkHandles::Clip { base, len, .. }) = &handles {
                        *frame = (f - base).clamp(0, *len);
                    }
                }
                Some(Drag::TapeMark { frame, is_in, .. }) if resp.dragged() => {
                    let end = self.snap.source.frames;
                    *frame = if *is_in {
                        f.clamp(0, end - 1)
                    } else {
                        (f + 1).clamp(1, end)
                    };
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
                Some(Drag::TapeMark { is_in, frame }) => {
                    let order = self.tape_order();
                    let key = if is_in { "in" } else { "out" };
                    self.send("tape.marks.set", json!({ key: frame, "order": order }));
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
    /// clip's first or last frame.
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
            if ui
                .button("Clear")
                .on_hover_text(super::keys("Alt+X"))
                .clicked()
            {
                self.clear_marks();
            }
            if player.speed != 0.0 && player.speed != 1.0 {
                ui.label(
                    RichText::new(format!("{:+}×", player.speed)).color(theme::colors().accent),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Clear of the panel's corner.
                ui.add_space(12.0);
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

    /// The playhead timecode; click, `=`, `+` or `-` to type one.
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
                if submit && let Some(spec) = timecode_input(&entry.text, self.dropped()) {
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

    /// The marked duration; editing it moves the Out mark.
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
                let length = timecode_input(&text, self.dropped())
                    .and_then(|spec| TimeSpec::parse(&spec).ok())
                    .and_then(|t| {
                        TimeContext {
                            fps: self.fps(),
                            start_tc: 0,
                            drop_frame: self
                                .project()
                                .is_some_and(|p| p.settings.counts_drop_frame()),
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

/// Stats for nerds over the picture's top left: a group for each part of
/// the player, a fact on each row. When the picture is too short for all
/// of them, the groups go on in another column.
fn stats_panel(painter: &egui::Painter, picture: Rect, stats: &crate::live::stats::PlayerStats) {
    let groups = stats.groups();
    let width = groups
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|(label, _)| label.chars().count()))
        .max()
        .unwrap_or(0);
    let font = FontId::monospace(11.0);
    let text = |color| egui::TextFormat::simple(font.clone(), color);
    let galleys: Vec<_> = groups
        .into_iter()
        .map(|(heading, rows)| {
            let mut job = egui::text::LayoutJob::default();
            job.append(heading, 0.0, text(theme::colors().accent));
            for (label, fact) in rows {
                job.append(
                    &format!("\n  {label:<width$}  "),
                    0.0,
                    text(theme::colors().text_dim),
                );
                job.append(&fact, 0.0, text(Color32::WHITE));
            }
            job.wrap.max_width = (picture.width() - 32.0).max(120.0);
            painter.layout_job(job)
        })
        .collect();
    let (top, bottom) = (picture.top() + 14.0, picture.bottom() - 14.0);
    let mut at = pos2(picture.left() + 16.0, top);
    let mut column = 0.0_f32;
    let mut area = Rect::NOTHING;
    let mut placed = Vec::new();
    for galley in galleys {
        let size = galley.size();
        if at.y > top && at.y + size.y > bottom {
            at = pos2(at.x + column + 24.0, top);
            column = 0.0;
        }
        column = column.max(size.x);
        area = area.union(Rect::from_min_size(at, size));
        placed.push((at, galley));
        at.y += size.y + 8.0;
    }
    painter.rect_filled(
        area.expand2(vec2(8.0, 6.0)),
        CornerRadius::same(4),
        Color32::from_black_alpha(200),
    );
    for (at, galley) in placed {
        painter.galley(at, galley, Color32::WHITE);
    }
}

/// What was typed into a timecode field, as a TimeSpec: digits fill a
/// timecode from the right ("100" is 00:00:01:00), and where the timecode
/// counts drop-frame, skipping `dropped` frame numbers at each minute but
/// every tenth, a label it skips lands on the minute's first frame
/// (`10000`, a minute, is 00:01:00;02 at 29.97: decision D47); anything
/// else (`5s`, `1m30s`, `450f`, `00:01:00:00`) is used as is.
pub(super) fn timecode_input(text: &str, dropped: i64) -> Option<String> {
    let t = text.trim().trim_start_matches(['+', '-', '=']);
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        let padded = format!("{t:0>8}");
        let d = &padded[padded.len() - 8..];
        let field = |i: usize| d[i..i + 2].parse::<i64>().unwrap_or(0);
        let (m, s, mut f) = (field(2), field(4), field(6));
        if s == 0 && f < dropped && m % 10 != 0 {
            f = dropped;
        }
        return Some(format!("{}:{m:02}:{s:02}:{f:02}", &d[0..2]));
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::timecode_input;

    #[test]
    fn digits_fill_timecodes_from_the_right() {
        assert_eq!(timecode_input("100", 0).as_deref(), Some("00:00:01:00"));
        assert_eq!(timecode_input("+12", 0).as_deref(), Some("00:00:00:12"));
        assert_eq!(timecode_input("1000000", 0).as_deref(), Some("01:00:00:00"));
        assert_eq!(timecode_input("5s", 0).as_deref(), Some("5s"));
        assert_eq!(timecode_input("  ", 0).as_deref(), None);
    }

    #[test]
    fn a_minute_typed_in_drop_frame_lands_on_its_first_frame() {
        assert_eq!(timecode_input("10000", 2).as_deref(), Some("00:01:00:02"));
    }
}
