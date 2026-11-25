use eframe::egui::{self, Rect, RichText, Sense, pos2, vec2};
use serde_json::json;
use vidcut_core::model::{Project, RenderRange, TrackKind};
use vidcut_core::request::ClipLabel;
use vidcut_core::time::format_timecode;

use super::theme;
use super::tiles;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

pub(super) const CLIPS_HEIGHT: f32 = 118.0;
const PICTURE: egui::Vec2 = vec2(96.0, 54.0);
const CELL_GAP: f32 = 8.0;

struct Thumb {
    number: usize,
    track: String,
    start: i64,
    end: i64,
    media: String,
    frame: i64,
    timecode: String,
    name: String,
    codec: Option<String>,
}

fn thumbs(p: &Project) -> Vec<Thumb> {
    let mut clips: Vec<_> = (p.tracks.iter())
        .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .collect();
    clips.sort_by_key(|(t, c)| (c.start, t.number()));
    let source_rate = p.settings.fps;
    (clips.into_iter().enumerate())
        .map(|(i, (t, c))| {
            let media = p.media(&c.media);
            let timecode = media.map_or_else(String::new, |m| {
                let rate = m.rate(source_rate);
                let per = vidcut_core::render::per_frame(m, source_rate, p.fps());
                let start = m.timecode_at(rate).round() as i64;
                let first = vidcut_core::render::shown_frame(per, c.source_in as f64 + c.phase);
                let drop = m.info.timecode.as_ref().is_some_and(|tc| tc.drop_frame);
                format_timecode(first, rate, start, drop)
            });
            Thumb {
                number: i + 1,
                track: t.id.clone(),
                start: c.start,
                end: c.end(),
                media: c.media.clone(),
                frame: tiles::tile_frame(c.source_in, &c.pace(), 0.0, c.duration),
                timecode,
                name: p.clip_name(c).unwrap_or_default(),
                codec: media
                    .and_then(|m| m.info.video.as_ref())
                    .map(|v| v.codec.clone()),
            }
        })
        .collect()
}

impl Workspace {
    pub(super) fn thumbnail_timeline(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.project() else {
            return;
        };
        let list = thumbs(p);
        let at = self.snap.timeline.position;
        let current = (list.iter())
            .filter(|t| t.start <= at && at < t.end)
            .max_by_key(|t| t.track[1..].parse::<usize>().unwrap_or(0))
            .map(|t| t.number);
        let label = self.export.clip_label;
        let colors = theme::colors();
        let mut seek = None;
        let mut action: Option<(Mark, i64, i64)> = None;
        let mut cycle = false;
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = CELL_GAP;
                    for t in &list {
                        let cell = ui.vertical(|ui| {
                            ui.set_width(PICTURE.x);
                            ui.horizontal(|ui| {
                                ui.label(dim(t.number.to_string()));
                                ui.add(egui::Label::new(dim(&t.timecode).small()).truncate());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| ui.label(dim(&t.track)),
                                );
                            });
                            let (rect, picture) = ui.allocate_exact_size(PICTURE, Sense::click());
                            let painter = ui.painter_at(rect);
                            painter.rect_filled(rect, 2.0, colors.panel_dark);
                            if let Some(m) = p.media(&t.media)
                                && let Some(texture) =
                                    self.pool_picture(ui.ctx(), m, t.frame, PICTURE.y, 0)
                            {
                                let aspect =
                                    tiles::media_aspect(m).unwrap_or(PICTURE.x / PICTURE.y);
                                let shown = Rect::from_center_size(rect.center(), fit(aspect));
                                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                painter.image(texture, shown, uv, egui::Color32::WHITE);
                            }
                            if current == Some(t.number) {
                                let stroke = egui::Stroke::new(2.0, colors.accent);
                                painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                            }
                            let text = match (label, &t.codec) {
                                (ClipLabel::Codec, Some(codec)) => codec.as_str(),
                                _ => t.name.as_str(),
                            };
                            let under = ui.add(
                                egui::Label::new(RichText::new(text).small())
                                    .truncate()
                                    .sense(Sense::click()),
                            );
                            if under.double_clicked() {
                                cycle = true;
                            }
                            picture
                        });
                        let picture = cell.inner;
                        if picture.clicked() {
                            seek = Some(t.start);
                        }
                        picture.context_menu(|ui| {
                            if ui.button("Mark In").clicked() {
                                action = Some((Mark::In, t.start, t.end));
                                ui.close();
                            }
                            if ui.button("Mark Out").clicked() {
                                action = Some((Mark::Out, t.start, t.end));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Render This Clip").clicked() {
                                action = Some((Mark::Clip, t.start, t.end));
                                ui.close();
                            }
                        });
                    }
                });
            });
        if cycle {
            self.export.clip_label = match label {
                ClipLabel::Codec => ClipLabel::Name,
                ClipLabel::Name => ClipLabel::Codec,
            };
        }
        if let Some(frame) = seek {
            self.seek(PlayerId::Timeline, frame, false);
        }
        match action {
            Some((Mark::In, a, _)) => self.mark_in_out("in", a),
            Some((Mark::Out, _, b)) => self.mark_in_out("out", b),
            Some((Mark::Clip, a, b)) => self.render_clip(a, b),
            None => {}
        }
    }

    fn render_clip(&self, a: i64, b: i64) {
        let marks = json!({ "in": a, "out": b });
        if !self.render_whole() {
            self.send("marks.set", marks);
            return;
        }
        let commands = json!([
            { "method": "marks.set", "params": marks },
            { "method": "timeline.set", "params": { "render": RenderRange::InOut } },
        ]);
        self.send("apply", json!({ "commands": commands }));
    }
}

fn fit(aspect: f32) -> egui::Vec2 {
    match aspect > PICTURE.x / PICTURE.y {
        true => vec2(PICTURE.x, PICTURE.x / aspect),
        false => vec2(PICTURE.y * aspect, PICTURE.y),
    }
}

enum Mark {
    In,
    Out,
    Clip,
}
