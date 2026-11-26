use std::cell::OnceCell;
use std::sync::Arc;

use eframe::egui::{self, Rect, RichText, Sense, pos2, vec2};
use serde_json::json;
use vidcut_core::model::{Clip, Media, Project, RenderRange, TrackKind};
use vidcut_core::ops::clip_filter;
use vidcut_core::render::VideoPiece;
use vidcut_core::request::ClipLabel;
use vidcut_core::time::format_timecode;

use super::theme;
use super::tiles;
use super::workspace::{Workspace, dim};
use crate::live::PlayerId;

pub(super) const CLIPS_HEIGHT: f32 = 118.0;
const PICTURE: egui::Vec2 = vec2(96.0, 54.0);
const CELL_GAP: f32 = 8.0;
const COMPOUND: &str = "Compound Clip";

pub(super) struct Strip {
    project: Arc<Project>,
    thumbs: Vec<Thumb>,
}

struct Thumb {
    number: usize,
    track: String,
    clip: String,
    start: i64,
    end: i64,
    media: String,
    look: String,
    piece: OnceCell<Option<VideoPiece>>,
    timecode: String,
    name: String,
    codec: Option<String>,
}

fn look(p: &Project, settings: &[u8], c: &Clip, m: Option<&Media>) -> String {
    let mut bytes = settings.to_vec();
    bytes.extend(serde_json::to_vec(&(c, m)).unwrap_or_default());
    if let Some(m) = m {
        let lut = m.input_lut_in(&p.settings);
        bytes.extend(crate::thumbcache::look(m, lut).as_bytes());
    }
    vidcut_core::fingerprint::short_digest(&bytes)
}

fn thumbs(p: &Project) -> Vec<Thumb> {
    let mut clips: Vec<_> = (p.tracks.iter())
        .filter(|t| t.kind == TrackKind::Video && t.video_enabled)
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .filter(|(_, c)| !c.is_generator())
        .collect();
    clips.sort_by_key(|(t, c)| (c.start, c.end(), t.number()));
    let source_rate = p.settings.fps;
    let settings = serde_json::to_vec(&p.settings).unwrap_or_default();
    let filter = p.timeline().map(|t| &t.clip_filter);
    (clips.into_iter().enumerate())
        .filter(|(_, (_, c))| filter.is_none_or(|f| clip_filter::admits(p, c, f)))
        .map(|(i, (t, c))| {
            let media = p.media(&c.media);
            let timecode = media.map_or_else(
                || format_timecode(c.source_in, p.fps(), 0, false),
                |m| {
                    let rate = m.rate(source_rate);
                    let per = vidcut_core::render::per_frame(m, source_rate, p.fps());
                    let start = m.timecode_at(rate).round() as i64;
                    let first = vidcut_core::render::shown_frame(per, c.source_in as f64 + c.phase);
                    let drop = m.info.timecode.as_ref().is_some_and(|tc| tc.drop_frame);
                    format_timecode(first, rate, start, drop)
                },
            );
            let codec = match &c.compound {
                Some(_) => Some(COMPOUND.to_string()),
                None => (media.and_then(|m| m.info.video.as_ref())).map(|v| v.codec.clone()),
            };
            Thumb {
                number: i + 1,
                track: t.id.clone(),
                clip: c.id.clone(),
                start: c.start,
                end: c.end(),
                media: c.media.clone(),
                look: look(p, &settings, c, media),
                piece: OnceCell::new(),
                timecode,
                name: p.clip_name(c).unwrap_or_default(),
                codec,
            }
        })
        .collect()
}

impl Workspace {
    pub(super) fn thumbnail_timeline(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.project_shared() else {
            return;
        };
        let strip = match self.export.strip.take() {
            Some(s) if Arc::ptr_eq(&s.project, &project) => s,
            _ => Strip {
                thumbs: thumbs(&project),
                project,
            },
        };
        let (p, list) = (&*strip.project, &strip.thumbs);
        let at = self.snap.timeline.position;
        let current = (list.iter())
            .filter(|t| t.start <= at && at < t.end)
            .max_by_key(|t| t.track[1..].parse::<usize>().unwrap_or(0))
            .map(|t| t.number);
        let label = self.export.clip_label;
        let colors = theme::colors();
        let selected = &self.snap.selection.clips;
        let mut picked = None;
        let mut action: Option<(Mark, i64, i64)> = None;
        let mut cycle = false;
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = CELL_GAP;
                    for (i, t) in list.iter().enumerate() {
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
                            if ui.is_rect_visible(rect)
                                && let Some(m) = p.media(&t.media)
                                && let Some(texture) = self.clip_picture(ui.ctx(), p, m, t)
                            {
                                let aspect =
                                    tiles::media_aspect(m).unwrap_or(PICTURE.x / PICTURE.y);
                                let shown = Rect::from_center_size(rect.center(), fit(aspect));
                                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                painter.image(texture, shown, uv, egui::Color32::WHITE);
                            }
                            let outline = match current == Some(t.number) {
                                true => Some(colors.accent),
                                false => selected.contains(&t.clip).then_some(colors.error),
                            };
                            if let Some(color) = outline {
                                let stroke = egui::Stroke::new(2.0, color);
                                painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                            }
                            let text = match (label, &t.codec) {
                                (ClipLabel::Codec, Some(codec)) => codec.as_str(),
                                _ => t.name.as_str(),
                            };
                            let shade = match current == Some(t.number) {
                                true => colors.text,
                                false => colors.text_dim,
                            };
                            let under = ui.add(
                                egui::Label::new(RichText::new(text).small().color(shade))
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
                            picked = Some((i, ui.input(|input| input.modifiers)));
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
        let pick = picked.map(|(i, mods)| pick(list, selected, i, mods));
        self.export.strip = Some(strip);
        if cycle {
            self.export.clip_label = match label {
                ClipLabel::Codec => ClipLabel::Name,
                ClipLabel::Name => ClipLabel::Codec,
            };
        }
        if let Some(p) = pick {
            let params = match p.seek {
                Some(_) => json!({ "clips": p.clips, "cues": [] }),
                None => json!({ "clips": p.clips }),
            };
            self.send("selection.set", params);
            if let Some(frame) = p.seek {
                self.seek(PlayerId::Timeline, frame, false);
            }
        }
        match action {
            Some((Mark::In, a, _)) => self.mark_in_out("in", a),
            Some((Mark::Out, _, b)) => self.mark_in_out("out", b),
            Some((Mark::Clip, a, b)) => self.render_clip(a, b),
            None => {}
        }
    }

    fn clip_picture(
        &self,
        ctx: &egui::Context,
        p: &Project,
        m: &Media,
        t: &Thumb,
    ) -> Option<egui::TextureId> {
        let kept = self.tiles.as_ref()?;
        let aspect = tiles::media_aspect(m)?;
        let height = tiles::tile_height(PICTURE.y * ctx.pixels_per_point());
        let width = (height as f32 * aspect).round().max(1.0) as u32;
        let key = tiles::TileKey {
            media: format!("{}@{}", t.clip, t.look),
            frame: t.start,
            height,
        };
        kept.want(&key, || {
            let piece = t
                .piece
                .get_or_init(|| vidcut_core::render::graded_frame(p, &t.clip, t.start).ok());
            Some(kept.clip_job(p, m, piece.clone()?, &key, width))
        })
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

struct Pick {
    clips: Vec<String>,
    seek: Option<i64>,
}

fn pick(list: &[Thumb], selected: &[String], i: usize, mods: egui::Modifiers) -> Pick {
    let t = &list[i];
    if mods.command {
        let mut clips = selected.to_vec();
        match clips.iter().position(|c| *c == t.clip) {
            Some(k) => {
                clips.remove(k);
            }
            None => clips.push(t.clip.clone()),
        }
        return Pick { clips, seek: None };
    }
    let anchor = (selected.last()).and_then(|last| list.iter().position(|o| o.clip == *last));
    if mods.shift
        && let Some(from) = anchor
    {
        let clips = list[from.min(i)..=from.max(i)]
            .iter()
            .map(|o| o.clip.clone())
            .collect();
        return Pick { clips, seek: None };
    }
    Pick {
        clips: vec![t.clip.clone()],
        seek: Some(t.start),
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
