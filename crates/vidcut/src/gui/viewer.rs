use std::collections::HashSet;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Key, Pos2, Rect, RichText, Sense, Stroke,
    pos2, vec2,
};
use serde_json::json;
use vidcut_core::model::{Media, Project, TrackKind};
use vidcut_core::request::{
    TimecodeDisplay, ViewerFrame, ViewerFrameKey, ViewerMode, viewer_frame_after,
};
use vidcut_core::time::parse_frame_number;

use super::pool::elide;
use super::row::Row;
use super::theme;
use super::workspace::{Drag, PasteWatch, TimecodeEntry, Workspace, dim, fit};
use crate::live::{PlayerId, seek_frame};

#[derive(Clone)]
pub struct TwoUp {
    pub mode: TwoUpMode,
    pub cells: Vec<Option<TwoUpCell>>,
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
    pub frame: i64,
    pub source_frame: i64,
    pub exact: bool,
    pub picture: Option<std::sync::Arc<vidcut_av::player::Picture>>,
}

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
        self.viewer_keys(ui.ctx());
        let frame = self.snap.viewer_frame;
        super::window::show_fullscreen(ui.ctx(), frame == ViewerFrame::FullScreen);
        if frame == ViewerFrame::Page {
            self.viewer_body(ui);
            return;
        }
        let ctx = ui.ctx().clone();
        let screen = ctx.input(|i| i.viewport_rect());
        egui::Area::new(egui::Id::unique("viewer-frame"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(&ctx, |ui| {
                ui.set_min_size(screen.size());
                let rect = Rect::from_min_size(screen.min, screen.size());
                ui.painter()
                    .rect_filled(rect, CornerRadius::ZERO, theme::colors().panel_dark);
                let _cover = ui.interact(rect, ui.scope_id().with("cover"), Sense::click());
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)));
                self.viewer_body(&mut child);
            });
    }

    fn viewer_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.asking() {
            return;
        }
        let frame = self.snap.viewer_frame;
        if frame != ViewerFrame::Page
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.send_frame(ViewerFrame::Page);
            return;
        }
        let key = ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::SHIFT, Key::F) {
                Some(ViewerFrameKey::FullScreen)
            } else if i.consume_key(egui::Modifiers::ALT, Key::F) {
                Some(ViewerFrameKey::Enhanced)
            } else {
                None
            }
        });
        if let Some(key) = key {
            self.send_frame(viewer_frame_after(frame, key));
        }
    }

    fn send_frame(&self, mode: ViewerFrame) {
        self.send("viewer.frame", json!({ "mode": mode }));
    }

    fn viewer_body(&mut self, ui: &mut egui::Ui) {
        let mode = self.snap.viewer;
        let id = PlayerId::of(mode);
        let modes = [
            Header::Mode(ViewerMode::SourceClip),
            Header::Mode(ViewerMode::SourceTape),
            Header::Mode(ViewerMode::Timeline),
        ];
        let row = Row {
            left: &modes,
            fill: Some((Header::Title, Align::Min)),
            right: &[Header::FullScreen, Header::Duration],
            leaving: &[modes[1], modes[0], modes[2], Header::FullScreen],
            group: |h| match h {
                Header::Mode(_) => 0,
                Header::Title => 1,
                Header::FullScreen | Header::Duration => 2,
            },
        };
        ui.horizontal(|ui| {
            self.fit_row(ui, row, |ws, ui, h| match h {
                Header::Mode(m) => {
                    let label = match m {
                        ViewerMode::SourceClip => "Source Clip",
                        ViewerMode::SourceTape => "Source Tape",
                        ViewerMode::Timeline => "Timeline",
                    };
                    let r = ui.selectable_label(mode == m, label);
                    if r.clicked() {
                        ws.send("viewer.set", json!({ "mode": m }));
                    }
                    r
                }
                Header::Title => {
                    ui.add(egui::Label::new(RichText::new(ws.viewer_title()).strong()).truncate())
                }
                Header::FullScreen => {
                    let on = ws.snap.viewer_frame == ViewerFrame::FullScreen;
                    let r = ui.selectable_label(on, "Full Screen");
                    if r.clicked() {
                        ws.send_frame(viewer_frame_after(
                            ws.snap.viewer_frame,
                            ViewerFrameKey::FullScreen,
                        ));
                    }
                    r
                }
                Header::Duration => ui.scope(|ui| ws.duration_field(ui)).response,
            });
        });

        let dynamic = self
            .timeline
            .dynamic
            .as_ref()
            .is_some_and(crate::gui::timeline::tools::Dynamic::running)
            && self.dynamic_target().is_some();
        let trim = self.trim_editor_edit();
        let tools = mode == ViewerMode::Timeline
            && self.timeline.picture_grab.row
            && !dynamic
            && trim.is_none();
        let below = 34.0 + 36.0 + if tools { TOOLS_ROW + 4.0 } else { 0.0 };
        let avail = ui.available_rect_before_wrap();
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
        let picture = {
            use eframe::emath::GuiRounding;
            fit(area.shrink(2.0), aspect).round_to_pixels(ppp)
        };
        self.want = [
            (picture.width() * ppp).round() as i32,
            (picture.height() * ppp).round() as i32,
        ];
        self.two_up = None;
        if self.timeline.edit.is_none() {
            self.trim_anchor = None;
        }
        if dynamic || trim.is_none() {
            self.timeline.strip_drag = None;
        }
        if !dynamic
            && trim.is_none()
            && let Some(cells) = &mut self.cells
        {
            cells.close();
        }
        if dynamic {
            self.dynamic_view(ui, area);
            return;
        }
        if let Some(e) = trim {
            self.trim_editor_view(ui, area, e);
            return;
        }
        let area_resp = ui.allocate_rect(area, Sense::click_and_drag());
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
        paint_guides(&painter, picture, &self.snap.guides);
        let offline = self.project().and_then(|p| {
            let source = self.snap.source_at().map(|(m, _)| m);
            offline_media(p, &self.snap.missing, mode, player.position, source)
                .map(|m| m.name.clone())
        });
        if mode == ViewerMode::Timeline {
            self.sync_subtitles();
        }
        if mode == ViewerMode::Timeline
            && offline.is_none()
            && played.is_some()
            && !self.picture_controls(ui, &area_resp, picture)
        {
            self.title_controls(ui, &area_resp, picture);
        }
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
        self.skim(ui, id, picture, player.frames);

        ui.add_space(4.0);
        if tools {
            ui.allocate_ui(vec2(ui.available_width(), TOOLS_ROW), |ui| {
                ui.horizontal_centered(|ui| self.picture_tools_row(ui));
            });
        }
        if mode.is_source() {
            ui.horizontal(|ui| {
                self.jog_edge(ui, true);
                let width = (ui.available_width() - 88.0).max(40.0);
                ui.allocate_ui(vec2(width, 28.0), |ui| self.scroll_bar(ui));
                self.jog_edge(ui, false);
            });
        } else {
            self.clip_edge_hint(ui);
        }
        self.transport(ui, id, None);
    }

    fn trim_editor_view(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        e: crate::gui::timeline::EditSel,
    ) {
        use crate::gui::timeline::Side;
        let Some(p) = self.shown_project() else {
            return;
        };
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.timeline.set_edit(None);
            return;
        }
        let band = super::trim_strips::band_height(area);
        let pictures = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - band - 6.0));
        let dragged = self.strip_drawn(&p);
        let (drawn, at) = match &dragged {
            Some((q, at)) => (q, *at),
            None => (&*p, e.at),
        };
        self.draw_two_up(
            ui,
            pictures,
            drawn,
            &e.track.id(),
            TwoUpMode::TrimEditor,
            &[(at - 1, "Out (outgoing)"), (at, "In (incoming)")],
        );
        let strips = Rect::from_min_max(pos2(area.left(), area.bottom() - band), area.max);
        self.trim_strips(ui, strips, &p, e);
        ui.add_space(4.0);
        let side = match e.side {
            Side::Out => "the Out",
            Side::In => "the In",
            _ => "a roll",
        };
        let text = if self.timeline.dynamic.is_some() {
            format!(
                "Dynamic Trim: {side} at {}. L/J trim, K stops, W leaves",
                self.tc(e.at)
            )
        } else {
            format!("Trim Editor: {side} at {}", self.tc(e.at))
        };
        let row = Row {
            left: &[],
            fill: Some((TrimRow::Status, Align::Min)),
            right: &[TrimRow::Close],
            leaving: &[],
            group: |_| 0,
        };
        ui.allocate_ui(vec2(ui.available_width(), 28.0), |ui| {
            ui.horizontal_centered(|ui| {
                self.fit_row(ui, row, |ws, ui, t| match t {
                    TrimRow::Status => {
                        ui.horizontal(|ui| {
                            if let Some(why) = ws.dynamic_stopped() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("Stopped here: {why}."))
                                            .color(theme::colors().error),
                                    )
                                    .truncate(),
                                );
                            }
                            ui.add(egui::Label::new(dim(&text)).truncate());
                        })
                        .response
                    }
                    TrimRow::Close => {
                        let r = ui.button("Close");
                        if r.clicked() {
                            ws.timeline.set_edit(None);
                        }
                        r
                    }
                });
            });
        });
        self.transport(ui, PlayerId::Timeline, Some(e));
    }

    fn dynamic_view(&mut self, ui: &mut egui::Ui, area: Rect) {
        use crate::gui::timeline::tools::{DynamicTarget, dynamic_preview, first_clip};
        let (Some(target), Some(p)) = (self.dynamic_target(), self.shown_project()) else {
            return;
        };
        let mut p = Project::clone(&p);
        let pending = self.timeline.dynamic.as_ref().map_or(0, |d| d.pending);
        let mode = self.timeline.options.dynamic_mode;
        let Ok(at) = dynamic_preview(&mut p, &target, mode, pending) else {
            return;
        };
        let pictures = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - 28.0));
        match (&target, target.lead()) {
            (_, Some(e)) => self.draw_two_up(
                ui,
                pictures,
                &p,
                &e.track.id(),
                TwoUpMode::DynamicTrim,
                &[(at - 1, "Out (outgoing)"), (at, "In (incoming)")],
            ),
            (DynamicTarget::Edits(_), None) => {}
            (DynamicTarget::Clips(ids), None) => {
                let Some((track, start, end)) = first_clip(&p, ids).and_then(|c| {
                    let track = crate::gui::timeline::TrackRef::of_clip(&p, &c.id)?;
                    Some((track.id(), c.start, c.end()))
                }) else {
                    return;
                };
                self.draw_two_up(
                    ui,
                    pictures,
                    &p,
                    &track,
                    TwoUpMode::DynamicTrim,
                    &[
                        (start, "First frame"),
                        (end - 1, "Last frame"),
                        (start - 1, "Before"),
                        (end, "After"),
                    ],
                );
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Dynamic Trim {pending:+} frames"))
                    .monospace()
                    .color(Color32::from_rgb(240, 200, 40)),
            );
            ui.label(dim("K or Space stops, L/J again for speed, W leaves"));
        });
    }

    fn draw_two_up(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        p: &vidcut_core::model::Project,
        track: &str,
        mode: TwoUpMode,
        cells: &[(i64, &str)],
    ) {
        let aspect = p.settings.width as f32 / p.settings.height.max(1) as f32;
        ui.allocate_rect(area, Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(12, 12, 13));
        let cols = cells.len().clamp(1, 2);
        let size = vec2(
            area.width() / cols as f32,
            area.height() / cells.len().div_ceil(cols).max(1) as f32,
        );
        let ppp = ui.ctx().pixels_per_point();
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
        players.keep(cells.len());
        let mut shown = vec![None; cells.len()];
        for (i, &(frame, label)) in cells.iter().enumerate() {
            let cell = Rect::from_min_size(
                area.min + vec2(size.x * (i % cols) as f32, size.y * (i / cols) as f32),
                size,
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
            let piece = vidcut_core::render::clip_frame(p, &c.id, frame).ok();
            if let Some(piece) = piece {
                let size = [picture.width(), picture.height()].map(|x| (x * ppp).round() as u32);
                let got = players.show(i, p, &piece, speed);
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

    fn scroll_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(2), theme::colors().track);
        let frames = self.snap.source.frames.max(1);
        self.skim(ui, PlayerId::Source, rect, frames);
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
                    *frame = tape_mark(f, self.snap.source.frames, *is_in);
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
        let text = match self.dynamic_stopped() {
            Some(why) => {
                RichText::new(format!("Stopped here: {why}.")).color(theme::colors().error)
            }
            None => dim(text),
        };
        ui.allocate_ui(vec2(ui.available_width(), 28.0), |ui| {
            ui.centered_and_justified(|ui| ui.label(text));
        });
    }

    fn jog_control(&mut self, ui: &mut egui::Ui, id: PlayerId, position: i64) -> egui::Response {
        let r = ui.button("Jog");
        let key = r.id;
        if r.drag_started() {
            ui.data_mut(|d| d.insert_temp(key, (position, 0.0f32, position)));
        }
        if r.dragged()
            && let Some((origin, mut px, mut sent)) =
                ui.data(|d| d.get_temp::<(i64, f32, i64)>(key))
        {
            px += r.drag_delta().x;
            let frame = origin + jog_steps(px);
            if frame != sent {
                sent = frame;
                self.seek(id, frame, true);
            }
            ui.data_mut(|d| d.insert_temp(key, (origin, px, sent)));
        }
        if r.drag_stopped() {
            ui.data_mut(|d| d.remove_temp::<(i64, f32, i64)>(key));
        }
        r.on_hover_text("Drag to scrub")
    }

    fn jog_edge(&mut self, ui: &mut egui::Ui, is_in: bool) {
        let r = ui.button(if is_in { "Jog In" } else { "Jog Out" });
        let key = r.id;
        if r.drag_started() {
            ui.data_mut(|d| d.insert_temp(key, (0.0f32, 0i64)));
        }
        let mut dragged = false;
        if r.dragged()
            && let Some((mut px, mut sent)) = ui.data(|d| d.get_temp::<(f32, i64)>(key))
        {
            dragged = true;
            px += r.drag_delta().x;
            let steps = jog_steps(px);
            let by = steps - sent;
            if by != 0 {
                self.nudge_mark(is_in, by);
                sent = steps;
            }
            ui.data_mut(|d| d.insert_temp(key, (px, sent)));
        }
        if r.drag_stopped() {
            ui.data_mut(|d| d.remove_temp::<(f32, i64)>(key));
        }
        if r.clicked() && !dragged {
            self.nudge_mark(is_in, 1);
        }
    }

    fn nudge_mark(&mut self, is_in: bool, by: i64) {
        if self.snap.viewer == ViewerMode::SourceTape
            && let Some((inn, out)) = self.crossing_span()
        {
            let tape_end = self
                .snap
                .tape
                .last()
                .map(|e| e.start + e.frames)
                .unwrap_or(0);
            let len = tape_end.max(self.snap.source.frames).max(0);
            let (inn, out) = jogged_mark(is_in, inn, out, by, len);
            let order = self.tape_order();
            if is_in {
                self.send("tape.marks.set", json!({ "in": inn, "order": order }));
            } else {
                self.send("tape.marks.set", json!({ "out": out, "order": order }));
            }
            return;
        }
        let Some(media) = self.snap.source_at().map(|(m, _)| m.to_string()) else {
            return;
        };
        let len = self.media_frames(&media);
        let marks = self
            .project()
            .and_then(|p| p.media(&media).map(|m| m.marks.clone()))
            .unwrap_or_default();
        let (inn, out) = jogged_mark(
            is_in,
            marks.mark_in.unwrap_or(0),
            marks.mark_out.unwrap_or(len),
            by,
            len,
        );
        if is_in {
            self.send("media.marks.set", json!({ "media": media, "in": inn }));
        } else {
            self.send("media.marks.set", json!({ "media": media, "out": out }));
        }
    }

    fn skim(&mut self, ui: &egui::Ui, id: PlayerId, rect: Rect, frames: i64) {
        if ui.ctx().egui_wants_keyboard_input() || self.asking() {
            return;
        }
        let (held, pos) = ui.input(|i| {
            let held = i.key_down(Key::C) && !i.modifiers.command && !i.modifiers.alt;
            (held, i.pointer.hover_pos())
        });
        let key = egui::Id::unique("skim-frame");
        if !held {
            ui.data_mut(|d| d.remove_temp::<i64>(key));
            return;
        }
        let Some(pos) = pos.filter(|p| rect.contains(*p)) else {
            return;
        };
        if self.drag.is_some() {
            return;
        }
        let frame = skim_frame(pos.x, rect.left(), rect.width(), frames);
        let seen = ui.data(|d| d.get_temp::<i64>(key));
        if seen == Some(frame) {
            return;
        }
        ui.data_mut(|d| d.insert_temp(key, frame));
        if seen.is_none() && self.snap.player(id).playing {
            self.send("player.pause", json!({}));
        }
        self.seek(id, frame, true);
    }

    fn transport(
        &mut self,
        ui: &mut egui::Ui,
        id: PlayerId,
        trim: Option<crate::gui::timeline::EditSel>,
    ) {
        use Transport as T;
        let player = self.snap.player(id).clone();
        let shuttling = player.speed != 0.0 && player.speed != 1.0;
        let tools = id == PlayerId::Timeline && trim.is_none();
        let left: Vec<Transport> = [
            tools.then_some(T::Tools),
            trim.map(|_| T::TrimOut),
            Some(T::Jog),
            Some(T::Previous),
            Some(T::Stop),
            Some(T::Play),
            Some(T::Next),
            Some(T::Loop),
            Some(T::FastReview),
            trim.map(|_| T::TrimIn),
            Some(T::MarkIn),
            Some(T::MarkOut),
            Some(T::Clear),
            shuttling.then_some(T::Speed),
        ]
        .into_iter()
        .flatten()
        .collect();
        let row = Row {
            left: &left,
            fill: None,
            right: &[T::SafeArea, T::Timecode],
            leaving: &[
                T::Speed,
                T::Clear,
                T::MarkOut,
                T::MarkIn,
                T::Tools,
                T::FastReview,
                T::Jog,
                T::Loop,
                T::TrimIn,
                T::TrimOut,
                T::Next,
                T::Previous,
                T::Stop,
                T::Play,
                T::SafeArea,
            ],
            group: |t| match t {
                T::Tools => 5,
                T::TrimOut => 0,
                T::Jog | T::Previous | T::Stop | T::Play | T::Next | T::Loop | T::FastReview => 1,
                T::TrimIn => 2,
                T::MarkIn | T::MarkOut | T::Clear | T::Speed => 3,
                T::SafeArea => 6,
                T::Timecode => 4,
            },
        };
        ui.horizontal(|ui| {
            self.fit_row(ui, row, |ws, ui, t| {
                let big = |s: &str| RichText::new(s).size(16.0);
                match t {
                    T::Tools => {
                        let shown = ws.timeline.picture_grab.row;
                        let r = ui.selectable_label(shown, "Tools");
                        if r.clicked() {
                            ws.timeline.picture_grab.row = !shown;
                        }
                        r
                    }
                    T::TrimOut | T::TrimIn => {
                        let e = trim.expect("trimming");
                        ui.horizontal(|ui| ws.trim_pair(ui, e, t == T::TrimOut))
                            .response
                    }
                    T::Jog => ws.jog_control(ui, id, player.position),
                    T::Previous => {
                        let r = ui.button(big("⏮"));
                        if r.clicked() {
                            ws.jump_edit(id, false);
                        }
                        r
                    }
                    T::Stop => {
                        let r = ui.button(big("⏹"));
                        if r.clicked() {
                            ws.send("player.pause", json!({}));
                        }
                        r
                    }
                    T::Play => {
                        let play = if player.playing { "⏸" } else { "▶" };
                        let r = ui.button(big(play));
                        if r.clicked() {
                            ws.send("player.toggle", json!({}));
                        }
                        r
                    }
                    T::Next => {
                        let r = ui.button(big("⏭"));
                        if r.clicked() {
                            ws.jump_edit(id, true);
                        }
                        r
                    }
                    T::Loop => {
                        let looping = ws.snap.looping;
                        let r = ui.selectable_label(looping, big(egui_phosphor::regular::REPEAT));
                        if r.clicked() {
                            ws.send("player.loop", json!({ "on": !looping }));
                        }
                        r
                    }
                    T::FastReview => {
                        let on = ws.snap.fast_review;
                        let r = ui.selectable_label(on, "Fast Review");
                        if r.clicked() {
                            ws.send("player.fast_review", json!({ "on": !on }));
                        }
                        r
                    }
                    T::MarkIn => {
                        let r = ui.button("Mark In");
                        if r.clicked() {
                            ws.mark(true);
                        }
                        r
                    }
                    T::MarkOut => {
                        let r = ui.button("Mark Out");
                        if r.clicked() {
                            ws.mark(false);
                        }
                        r
                    }
                    T::Clear => {
                        let r = ui.button("Clear");
                        if r.clicked() {
                            ws.clear_marks();
                        }
                        r
                    }
                    T::Speed => ui.label(
                        RichText::new(format!("{:+}×", player.speed)).color(theme::colors().accent),
                    ),
                    T::SafeArea => ws.safe_area_control(ui),
                    T::Timecode => {
                        ui.scope(|ui| {
                            ui.add_space(12.0);
                            ws.timecode_field(ui);
                        })
                        .response
                    }
                }
            });
        });
    }

    fn trim_pair(&mut self, ui: &mut egui::Ui, e: crate::gui::timeline::EditSel, outgoing: bool) {
        use crate::gui::timeline::Side;
        let mine = if outgoing { Side::Out } else { Side::In };
        let (has_out, has_in) = self
            .project()
            .and_then(|p| {
                p.track(&e.track.id()).map(|t| {
                    (
                        t.clips.iter().any(|c| c.end() == e.at),
                        t.clips.iter().any(|c| c.start == e.at),
                    )
                })
            })
            .unwrap_or((false, false));
        let side = if e.side == Side::Both && has_out && has_in {
            Side::Both
        } else {
            mine
        };
        let here = if outgoing { has_out } else { has_in };
        let enabled = here && (e.side == mine || e.side == Side::Both);
        let what = match (side, outgoing) {
            (Side::Both, _) => "Roll the edit",
            (_, true) => "Trim the Out",
            _ => "Trim the In",
        };
        for (by, icon, way) in [(-1, "◀|", "a frame earlier"), (1, "|▶", "a frame later")] {
            let tip = format!("{what} {way}");
            let r = ui.add_enabled(enabled, egui::Button::new(RichText::new(icon).size(14.0)));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &tip));
            if r.clicked() {
                self.nudge_edit(side, by);
            }
        }
        if !enabled {
            let why = match here {
                false => "no clip on this side",
                true => "trimming the other side",
            };
            super::dialog::why(ui, why);
        }
    }

    pub(super) fn safe_area_control(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let on = self.snap.guides.enabled;
        let r = ui.selectable_label(on, "Safe Area");
        if r.clicked() {
            self.send("viewer.guides", json!({}));
        }
        let tip = r.on_hover_text("Framing Guides");
        tip.context_menu(|ui| self.safe_area_menu(ui));
        let menu = ui.menu_button("▾", |ui| self.safe_area_menu(ui));
        tip.union(menu.response)
    }

    pub(super) fn safe_area_menu(&mut self, ui: &mut egui::Ui) {
        use vidcut_core::preferences::Framing;
        let g = &self.snap.guides;
        ui.label(RichText::new("Social Media").weak());
        for (label, framing) in [
            ("1:1", Framing::Social1_1),
            ("4:5", Framing::Social4_5),
            ("9:16", Framing::Social9_16),
            ("1.91:1", Framing::Social1_91),
            ("16:9", Framing::Social16_9),
        ] {
            if ui.selectable_label(g.framing == framing, label).clicked() {
                self.send(
                    "viewer.guides",
                    json!({ "enabled": true, "framing": framing }),
                );
                ui.close();
            }
        }
        ui.separator();
        ui.label(RichText::new("Broadcast and Film").weak());
        for (label, framing) in [
            ("Default", Framing::Default),
            ("1.33", Framing::Broadcast1_33),
            ("1.66", Framing::Broadcast1_66),
            ("1.77", Framing::Broadcast1_77),
            ("1.85", Framing::Broadcast1_85),
            ("2.35", Framing::Broadcast2_35),
        ] {
            if ui.selectable_label(g.framing == framing, label).clicked() {
                self.send(
                    "viewer.guides",
                    json!({ "enabled": true, "framing": framing }),
                );
                ui.close();
            }
        }
        ui.separator();
        let mut extents = g.extents;
        if ui.checkbox(&mut extents, "Extents").clicked() {
            self.send("viewer.guides", json!({ "extents": extents }));
        }
        let mut action = g.action;
        if ui.checkbox(&mut action, "Action").clicked() {
            self.send("viewer.guides", json!({ "action": action }));
        }
        let mut title = g.title;
        if ui.checkbox(&mut title, "Title").clicked() {
            self.send("viewer.guides", json!({ "title": title }));
        }
        let mut center = g.center;
        if ui.checkbox(&mut center, "Center").clicked() {
            self.send("viewer.guides", json!({ "center": center }));
        }
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

    fn timecode_field(&mut self, ui: &mut egui::Ui) {
        let current = self.snap.timecode_text();
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
            if entry.focus && !ui.is_sizing_pass() {
                r.request_focus();
                entry.focus = false;
            }
            if r.lost_focus() {
                let submit = ui.input(|i| i.key_pressed(Key::Enter));
                let entry = self.timecode.take().expect("editing");
                if submit {
                    self.submit_timecode(&entry);
                }
            }
        } else {
            let r =
                ui.add(egui::Label::new(RichText::new(&current).font(font)).sense(Sense::click()));
            r.context_menu(|ui| self.timecode_menu(ui, &current));
            if r.clicked() {
                self.timecode = Some(TimecodeEntry {
                    sign: None,
                    text: String::new(),
                    focus: true,
                });
            }
        }
    }

    fn timecode_menu(&mut self, ui: &mut egui::Ui, text: &str) {
        use TimecodeDisplay::{
            Auto, RecordFrame, RecordTimecode, SourceFrame, SourceTimecode, Timer1, Timer2,
        };
        let current = self.snap.timecode_display;
        for (label, display) in [
            ("Auto", Auto),
            ("Source Timecode", SourceTimecode),
            ("Source Frame", SourceFrame),
            ("Record Timecode", RecordTimecode),
            ("Record Frame", RecordFrame),
            ("Timer 1", Timer1),
            ("Timer 2", Timer2),
        ] {
            if ui.selectable_label(current == display, label).clicked() {
                self.send("viewer.set", json!({ "display": display }));
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Reset Timer 1").clicked() {
            self.send("viewer.set", json!({ "reset_timer1": true }));
            ui.close();
        }
        if ui.button("Copy Timecode").clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
        if ui.button("Paste Timecode").clicked() {
            self.paste_timecode = PasteWatch::Wait;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            ui.close();
        }
    }

    fn submit_timecode(&self, entry: &TimecodeEntry) {
        let raw = entry.text.trim();
        if raw.is_empty() {
            return;
        }
        if let Some(sign) = entry.sign {
            let Some(spec) = timecode_input(raw, self.dropped()) else {
                return;
            };
            let at = if sign > 0 {
                format!("+{spec}")
            } else {
                format!("-{spec}")
            };
            self.send("player.seek", json!({ "at": at }));
            return;
        }
        let view = self.snap.timecode_view();
        if view.display == TimecodeDisplay::Auto && !view.viewer.is_source() {
            let Some(spec) = timecode_input(raw, self.dropped()) else {
                return;
            };
            self.send("player.seek", json!({ "at": spec }));
            return;
        }
        let text = if matches!(
            view.display,
            TimecodeDisplay::SourceFrame | TimecodeDisplay::RecordFrame
        ) {
            if parse_frame_number(raw).is_none() {
                self.send("viewer.set", json!({ "paste": raw }));
                return;
            }
            raw.to_string()
        } else {
            let Some(spec) = timecode_input(raw, self.dropped()) else {
                return;
            };
            if !spec.contains([':', ';']) {
                self.send("player.seek", json!({ "at": spec }));
                return;
            }
            spec
        };
        match seek_frame(&text, &view) {
            Ok(frame) => self.send("player.seek", json!({ "at": format!("{frame}f") })),
            Err(_) => self.send("viewer.set", json!({ "paste": text })),
        }
    }

    fn duration_field(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project() else { return };
        let (marks, end, media) = match self.snap.source_at() {
            Some((media, _)) => (
                p.media(media).map(|m| m.marks.clone()).unwrap_or_default(),
                self.media_frames(media),
                Some(media.to_string()),
            ),
            None => (p.marks.clone(), p.end(), None),
        };
        let current = self.src_tc(marks.mark_out.unwrap_or(end) - marks.mark_in.unwrap_or(0));
        if let Some(text) = &mut self.duration_text {
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .font(FontId::monospace(13.0))
                    .desired_width(110.0)
                    .hint_text(&current),
            );
            if r.lost_focus() {
                let submit = ui.input(|i| i.key_pressed(Key::Enter));
                let text = self.duration_text.take().unwrap_or_default();
                let text = text.trim();
                if submit && !text.is_empty() {
                    match media {
                        Some(m) => {
                            self.send("media.marks.set", json!({ "media": m, "duration": text }))
                        }
                        None => self.send("marks.set", json!({ "duration": text })),
                    }
                }
            } else if !r.has_focus() && !ui.is_sizing_pass() {
                r.request_focus();
            }
        } else {
            let r = ui.add(
                egui::Label::new(RichText::new(&current).font(FontId::monospace(13.0)))
                    .sense(Sense::click()),
            );
            ui.label(dim("Duration"));
            if r.clicked() {
                self.duration_text = Some(String::new());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Header {
    Mode(ViewerMode),
    Title,
    FullScreen,
    Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrimRow {
    Status,
    Close,
}

const TOOLS_ROW: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Tools,
    TrimOut,
    Jog,
    Previous,
    Stop,
    Play,
    Next,
    Loop,
    FastReview,
    TrimIn,
    MarkIn,
    MarkOut,
    Clear,
    Speed,
    SafeArea,
    Timecode,
}

const JOG_PIXELS: f32 = 8.0;

fn jog_steps(pixels: f32) -> i64 {
    if !pixels.is_finite() {
        return 0;
    }
    (pixels / JOG_PIXELS).trunc() as i64
}

fn jogged_mark(is_in: bool, inn: i64, out: i64, by: i64, len: i64) -> (i64, i64) {
    let len = len.max(0);
    if len == 0 {
        return (inn, out);
    }
    if is_in {
        let limit = (out - 1).clamp(0, len);
        ((inn + by).clamp(0, limit), out)
    } else {
        let start = inn.saturating_add(1).min(len);
        (inn, (out + by).clamp(start, len))
    }
}

fn skim_frame(x: f32, left: f32, width: f32, frames: i64) -> i64 {
    if frames <= 1 || !width.is_finite() || width <= 0.0 {
        return 0;
    }
    let t = ((x - left) / width).clamp(0.0, 1.0);
    let frame = (t * frames as f32).floor() as i64;
    frame.clamp(0, frames - 1)
}

fn tape_mark(pointer: i64, end: i64, is_in: bool) -> i64 {
    let end = end.max(1);
    if is_in {
        pointer.clamp(0, end - 1)
    } else {
        (pointer + 1).clamp(1, end)
    }
}

fn paint_guides(painter: &egui::Painter, picture: Rect, guides: &vidcut_core::preferences::Guides) {
    let Some(o) = guides.overlay(vidcut_core::preferences::GuideBox::from_size(
        picture.width(),
        picture.height(),
    )) else {
        return;
    };
    let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 180));
    let to_rect = |b: vidcut_core::preferences::GuideBox| {
        Rect::from_min_size(
            pos2(picture.min.x + b.x, picture.min.y + b.y),
            vec2(b.w, b.h),
        )
    };
    if o.crop {
        let frame = to_rect(o.frame);
        let shade = Color32::from_rgba_unmultiplied(0, 0, 0, 120);
        if frame.top() > picture.top() {
            painter.rect_filled(
                Rect::from_min_max(picture.left_top(), pos2(picture.right(), frame.top())),
                CornerRadius::ZERO,
                shade,
            );
        }
        if frame.bottom() < picture.bottom() {
            painter.rect_filled(
                Rect::from_min_max(pos2(picture.left(), frame.bottom()), picture.right_bottom()),
                CornerRadius::ZERO,
                shade,
            );
        }
        if frame.left() > picture.left() {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(picture.left(), frame.top()),
                    pos2(frame.left(), frame.bottom()),
                ),
                CornerRadius::ZERO,
                shade,
            );
        }
        if frame.right() < picture.right() {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(frame.right(), frame.top()),
                    pos2(picture.right(), frame.bottom()),
                ),
                CornerRadius::ZERO,
                shade,
            );
        }
    }
    if let Some(b) = o.extents {
        painter.rect_stroke(
            to_rect(b),
            CornerRadius::ZERO,
            stroke,
            egui::StrokeKind::Inside,
        );
    }
    if let Some(b) = o.action {
        painter.rect_stroke(
            to_rect(b),
            CornerRadius::ZERO,
            stroke,
            egui::StrokeKind::Inside,
        );
    }
    if let Some(b) = o.title {
        painter.rect_stroke(
            to_rect(b),
            CornerRadius::ZERO,
            stroke,
            egui::StrokeKind::Inside,
        );
    }
    if let Some((cx, cy)) = o.center {
        let c = pos2(picture.min.x + cx, picture.min.y + cy);
        let frame = to_rect(o.frame);
        painter.line_segment([pos2(frame.left(), c.y), pos2(frame.right(), c.y)], stroke);
        painter.line_segment([pos2(c.x, frame.top()), pos2(c.x, frame.bottom())], stroke);
    }
}

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
    use super::{jog_steps, jogged_mark, skim_frame, tape_mark, timecode_input};

    #[test]
    fn eight_pixels_move_one_frame() {
        assert_eq!(jog_steps(8.0), 1);
    }

    #[test]
    fn a_drag_left_moves_one_frame_earlier() {
        assert_eq!(jog_steps(-8.0), -1);
    }

    #[test]
    fn seven_pixels_stay_on_the_frame() {
        assert_eq!(jog_steps(7.0), 0);
    }

    #[test]
    fn jog_in_moves_the_in_mark_one_frame_later() {
        assert_eq!(jogged_mark(true, 10, 40, 1, 100), (11, 40));
    }

    #[test]
    fn jog_in_stops_before_the_out_mark() {
        assert_eq!(jogged_mark(true, 39, 40, 5, 100), (39, 40));
    }

    #[test]
    fn jog_out_moves_the_out_mark_one_frame_earlier() {
        assert_eq!(jogged_mark(false, 10, 40, -1, 100), (10, 39));
    }

    #[test]
    fn an_empty_clip_keeps_its_marks() {
        assert_eq!(jogged_mark(true, 0, 0, 1, 0), (0, 0));
    }

    #[test]
    fn the_left_edge_skims_the_first_frame() {
        assert_eq!(skim_frame(0.0, 0.0, 100.0, 10), 0);
    }

    #[test]
    fn the_right_edge_skims_the_last_frame() {
        assert_eq!(skim_frame(100.0, 0.0, 100.0, 10), 9);
    }

    #[test]
    fn an_empty_picture_skims_frame_zero() {
        assert_eq!(skim_frame(40.0, 0.0, 100.0, 0), 0);
    }

    #[test]
    fn an_empty_source_keeps_a_tape_in_mark_at_frame_zero() {
        assert_eq!(tape_mark(4, 0, true), 0);
    }

    #[test]
    fn an_empty_source_keeps_a_tape_out_mark_at_frame_one() {
        assert_eq!(tape_mark(4, 0, false), 1);
    }

    #[test]
    fn a_tape_in_mark_stops_on_the_last_frame() {
        assert_eq!(tape_mark(40, 10, true), 9);
    }

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
