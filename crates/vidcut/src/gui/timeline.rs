//! The timeline area. M3 shows the upper timeline (spec §5): the whole
//! program at full width with the ruler, In/Out range, markers, Smart
//! Indicator and playhead. Clicking selects clips; the ruler scrubs. The
//! zoomed lower timeline and drag editing arrive in M4.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, vec2,
};
use serde_json::json;
use vidcut_core::ops;

use super::app::{App, Drag, dim};
use super::pool::elide;
use super::theme;
use crate::live::PlayerId;

const RULER: f32 = 22.0;

impl App {
    pub(super) fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project().cloned() else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Timeline").strong());
            let mut ripple = p.settings.ripple_on;
            if ui
                .toggle_value(&mut ripple, "Ripple On")
                .on_hover_text("Track 1 closes gaps on insert, delete, move and trim (p. 645)")
                .changed()
            {
                self.send("project.set", json!({ "ripple_on": ripple }));
            }
            if ui
                .button("Split")
                .on_hover_text("Split at the playhead (Ctrl+\\)")
                .clicked()
            {
                self.send(
                    "edit.split",
                    json!({ "at": self.snap.timeline.position, "clips": self.snap.selection.clips }),
                );
            }
            if ui
                .button("Marker")
                .on_hover_text("Add a marker at the playhead (M)")
                .clicked()
            {
                self.send("markers.add", json!({ "at": self.snap.timeline.position }));
            }
            ui.separator();
            let marks = match (p.marks.mark_in, p.marks.mark_out) {
                (None, None) => "no In/Out".to_string(),
                (a, b) => format!(
                    "In {}  Out {}",
                    a.map_or("-".into(), |f| self.tc(f)),
                    b.map_or("-".into(), |f| self.tc(f - 1)),
                ),
            };
            ui.label(dim(marks));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(dim(format!(
                    "{} clips, {}",
                    p.tracks.iter().map(|t| t.clips.len()).sum::<usize>(),
                    self.tc(p.end())
                )));
            });
        });

        let rect = ui.available_rect_before_wrap().shrink2(vec2(0.0, 2.0));
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(2), theme::TRACK);
        let end = p.end();
        // Leave room after the end so the last edit is easy to see and hit.
        let fps = p.settings.fps.as_f64();
        let span = ((end as f64 * 1.04).max(fps * 10.0)).ceil() as i64;
        let x = |f: i64| rect.left() + rect.width() * (f as f32 / span as f32);
        let frame_at = |px: f32| {
            (((px - rect.left()) / rect.width()).clamp(0.0, 1.0) * span as f32).round() as i64
        };
        let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER));
        painter.rect_filled(ruler, CornerRadius::ZERO, theme::PANEL_DARK);

        // Ticks every "nice" interval with at least ~90 px between labels.
        let secs_per_px = span as f64 / fps / f64::from(rect.width().max(1.0));
        let step = [
            1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
        ]
        .into_iter()
        .find(|s| s / secs_per_px >= 90.0)
        .unwrap_or(7200.0);
        let mut t = 0.0;
        while t * fps < span as f64 {
            let f = (t * fps).round() as i64;
            let px = x(f);
            painter.line_segment(
                [
                    Pos2::new(px, ruler.bottom() - 6.0),
                    Pos2::new(px, ruler.bottom()),
                ],
                Stroke::new(1.0, theme::TEXT_DIM),
            );
            painter.text(
                Pos2::new(px + 3.0, ruler.top() + 2.0),
                Align2::LEFT_TOP,
                self.tc(f),
                FontId::monospace(10.0),
                theme::TEXT_DIM,
            );
            t += step;
        }

        // In/Out range on the ruler and across the track.
        if p.marks.mark_in.is_some() || p.marks.mark_out.is_some() {
            let a = x(p.marks.mark_in.unwrap_or(0));
            let b = x(p.marks.mark_out.unwrap_or(end));
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(a, rect.top()), Pos2::new(b, rect.bottom())),
                CornerRadius::ZERO,
                theme::MARKED,
            );
        }

        // Track 1.
        let track = Rect::from_min_max(
            Pos2::new(rect.left(), ruler.bottom() + 6.0),
            Pos2::new(
                rect.right(),
                (ruler.bottom() + 6.0 + 84.0).min(rect.bottom() - 4.0),
            ),
        );
        painter.text(
            Pos2::new(track.left() + 4.0, track.top() + 2.0),
            Align2::LEFT_TOP,
            "1",
            FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
        let mut clicked_clip = None;
        let pointer = resp.interact_pointer_pos();
        for t in p.tracks.iter().filter(|t| t.id == "V1") {
            for c in &t.clips {
                let r = Rect::from_min_max(
                    Pos2::new(x(c.start), track.top()),
                    Pos2::new(x(c.end()).max(x(c.start) + 1.0), track.bottom()),
                );
                let audio_h = (r.height() * 0.32).round();
                let video = Rect::from_min_max(r.min, Pos2::new(r.right(), r.bottom() - audio_h));
                let fill = if c.enabled && t.video_enabled {
                    theme::CLIP
                } else {
                    theme::CLIP_DISABLED
                };
                painter.rect_filled(video.shrink2(vec2(0.5, 0.0)), CornerRadius::same(2), fill);
                if let Some((a, b)) = c.audio_range() {
                    let ar = Rect::from_min_max(
                        Pos2::new(x(a), r.bottom() - audio_h + 1.0),
                        Pos2::new(x(b), r.bottom()),
                    );
                    let muted = c.audio.as_ref().is_some_and(|a| a.muted) || t.muted;
                    let color = if muted {
                        theme::CLIP_DISABLED
                    } else {
                        theme::AUDIO
                    };
                    painter.rect_filled(ar.shrink2(vec2(0.5, 0.0)), CornerRadius::same(2), color);
                }
                let selected = self.snap.selection.clips.contains(&c.id);
                if selected {
                    painter.rect_stroke(
                        r,
                        CornerRadius::same(2),
                        Stroke::new(2.0, theme::ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                if video.width() > 30.0 {
                    let name = c
                        .name
                        .clone()
                        .or_else(|| p.media(&c.media).map(|m| m.name.clone()))
                        .unwrap_or_default();
                    let chars = (video.width() / 7.0) as usize;
                    painter.text(
                        video.left_top() + vec2(4.0, 3.0),
                        Align2::LEFT_TOP,
                        elide(&name, chars.max(3)),
                        FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                    if video.width() > 70.0 {
                        painter.text(
                            Pos2::new(video.left() + 4.0, video.bottom() - 3.0),
                            Align2::LEFT_BOTTOM,
                            self.src_tc(c.duration),
                            FontId::monospace(10.0),
                            Color32::from_white_alpha(170),
                        );
                    }
                }
                if resp.clicked()
                    && let Some(pos) = pointer
                    && r.contains(pos)
                {
                    clicked_clip = Some(c.id.clone());
                }
            }
        }

        // Markers and the Smart Indicator on the ruler (p. 696, 718).
        for k in &p.markers {
            let px = x(k.at);
            let color = marker_color(&k.color);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(px - 5.0, ruler.bottom() - 9.0),
                    Pos2::new(px + 5.0, ruler.bottom() - 9.0),
                    Pos2::new(px, ruler.bottom() - 1.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        let playhead = self.snap.timeline.position;
        if end > 0 {
            let smart = ops::nearest_edit(&p, playhead);
            let px = x(smart);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(px, track.top() - 1.0),
                    Pos2::new(px - 5.0, track.top() - 7.0),
                    Pos2::new(px + 5.0, track.top() - 7.0),
                ],
                theme::OK,
                Stroke::NONE,
            ));
        }
        let px = x(playhead);
        painter.line_segment(
            [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
            Stroke::new(2.0, theme::PLAYHEAD),
        );

        // Input: the ruler (or empty track space) scrubs; clips select. Any
        // click in the timeline shows the timeline in the viewer (p. 681).
        let on_ruler = |pos: Pos2| pos.y <= ruler.bottom() + 3.0;
        if resp.drag_started()
            && let Some(pos) = pointer
        {
            self.drag = Some(Drag::Scrub(PlayerId::Timeline));
            let f = frame_at(pos.x).min(end);
            self.scrub = Some((PlayerId::Timeline, f));
            self.seek(PlayerId::Timeline, f, true);
        } else if resp.dragged()
            && matches!(self.drag, Some(Drag::Scrub(PlayerId::Timeline)))
            && let Some(pos) = pointer
        {
            let f = frame_at(pos.x).min(end);
            if self.scrub != Some((PlayerId::Timeline, f)) {
                self.scrub = Some((PlayerId::Timeline, f));
                self.seek(PlayerId::Timeline, f, true);
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
            if let Some((id, f)) = self.scrub.take() {
                self.seek(id, f, false);
            }
        } else if resp.clicked()
            && let Some(pos) = pointer
        {
            let mods = ui.input(|i| i.modifiers);
            match clicked_clip {
                Some(id) if !on_ruler(pos) => {
                    let mut sel = self.snap.selection.clips.clone();
                    if mods.command {
                        if let Some(i) = sel.iter().position(|c| c == &id) {
                            sel.remove(i);
                        } else {
                            sel.push(id);
                        }
                    } else {
                        sel = vec![id];
                    }
                    self.send("selection.set", json!({ "clips": sel }));
                    self.send("viewer.set", json!({ "mode": "timeline" }));
                }
                _ => {
                    if !on_ruler(pos) && !mods.command {
                        self.send("selection.set", json!({ "clips": [] }));
                    }
                    self.seek(PlayerId::Timeline, frame_at(pos.x).min(end), false);
                }
            }
        }
    }
}

fn marker_color(name: &str) -> Color32 {
    match name {
        "red" => Color32::from_rgb(220, 70, 60),
        "green" => Color32::from_rgb(90, 190, 90),
        "yellow" => Color32::from_rgb(230, 200, 60),
        "purple" => Color32::from_rgb(160, 100, 220),
        "cyan" => Color32::from_rgb(80, 200, 210),
        "pink" => Color32::from_rgb(230, 120, 180),
        _ => Color32::from_rgb(80, 140, 230),
    }
}
