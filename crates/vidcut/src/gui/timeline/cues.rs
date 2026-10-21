use std::collections::HashSet;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rangef, Rect, RichText, Stroke,
    StrokeKind, pos2, vec2,
};
use serde_json::{Value, json};
use vidcut_core::looks::{LookKind, Styled};
use vidcut_core::model::{Cue, Place, Project, SubtitleTrack};
use vidcut_core::ops::Edge;

use super::geometry::{Row, Rows, Scale, TrackRect, header_parts, is_subtitle};
use super::gesture::Gesture;
use super::{header_button, icon, paint};
use crate::gui::export::Ask;
use crate::gui::keys;
use crate::gui::pool::elide;
use crate::gui::theme;
use crate::gui::workspace::Workspace;

const EDGE: f32 = 8.0;
const FILL: Color32 = Color32::from_rgb(52, 98, 108);

#[derive(Debug, Clone, PartialEq)]
pub enum CueHit {
    Body(String),
    Edge(String, Edge),
}

impl CueHit {
    pub fn cue(&self) -> &str {
        match self {
            Self::Body(id) | Self::Edge(id, _) => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CueMenu {
    pub upper: bool,
    pub track: String,
    pub cue: Option<String>,
}

fn lanes(cues: &[Cue]) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 1); cues.len()];
    let mut i = 0;
    while i < cues.len() {
        let mut ends: Vec<i64> = Vec::new();
        let mut reach = i64::MIN;
        let mut j = i;
        while j < cues.len() && (j == i || cues[j].start < reach) {
            let c = &cues[j];
            let lane = match ends.iter().position(|&e| e <= c.start) {
                Some(l) => {
                    ends[l] = c.end();
                    l
                }
                None => {
                    ends.push(c.end());
                    ends.len() - 1
                }
            };
            out[j].0 = lane;
            reach = reach.max(c.end());
            j += 1;
        }
        for o in &mut out[i..j] {
            o.1 = ends.len();
        }
        i = j;
    }
    out
}

pub fn placed<'a>(
    t: &'a SubtitleTrack,
    scale: &Scale,
    row: Rect,
) -> impl Iterator<Item = (&'a Cue, Rect)> + 'a {
    let scale = *scale;
    let (a, b) = scale.visible(row.left(), row.right());
    lanes(&t.cues)
        .into_iter()
        .zip(&t.cues)
        .filter(move |(_, c)| c.end() >= a && c.start <= b)
        .map(move |((lane, count), c)| {
            let x0 = scale.xf(c.start);
            let x1 = scale.xf(c.end()).max(x0 + 1.0);
            let h = (row.height() + 1.0) / count as f32;
            let top = (row.top() + h * lane as f32).round();
            let bottom = if lane + 1 == count {
                row.bottom()
            } else {
                (row.top() + h * (lane + 1) as f32).round() - 1.0
            };
            (c, Rect::from_x_y_ranges(x0..=x1, Rangef::new(top, bottom)))
        })
}

pub fn hit(t: &SubtitleTrack, scale: &Scale, row: Rect, pos: Pos2) -> Option<CueHit> {
    if !row.contains(pos) {
        return None;
    }
    let placed: Vec<(&Cue, Rect)> = placed(t, scale, row).collect();
    for (c, r) in placed.iter().rev() {
        if !r.y_range().contains(pos.y) || pos.x < r.left() - 2.0 || pos.x > r.right() + 2.0 {
            continue;
        }
        let edge = EDGE.min(r.width() / 3.0);
        if pos.x - r.left() <= edge {
            return Some(CueHit::Edge(c.id.clone(), Edge::Head));
        }
        if r.right() - pos.x <= edge {
            return Some(CueHit::Edge(c.id.clone(), Edge::Tail));
        }
        if r.x_range().contains(pos.x) {
            return Some(CueHit::Body(c.id.clone()));
        }
    }
    None
}

fn command(method: &str, params: Value) -> Value {
    json!({ "method": method, "params": params })
}

fn unlocked(p: &Project, cues: &[String]) -> Vec<String> {
    cues.iter()
        .filter(|q| p.find_cue(q).is_some_and(|(t, _)| !p.subtitles[t].locked))
        .cloned()
        .collect()
}

fn unlocked_clips(p: &Project, clips: &[String], at: Option<i64>) -> Vec<String> {
    clips
        .iter()
        .filter(|id| {
            p.find_clip(id).is_some_and(|(t, c)| {
                let clip = &p.tracks[t].clips[c];
                !p.tracks[t].locked && at.is_none_or(|at| clip.start < at && at < clip.end())
            })
        })
        .cloned()
        .collect()
}

pub fn one_line(text: &str) -> String {
    vidcut_core::subtitles::plain(text)
        .lines()
        .collect::<Vec<_>>()
        .join(" / ")
}

fn hatch(painter: &Painter, rect: Rect) {
    let mut x = rect.left() - rect.height();
    while x < rect.right() {
        painter.line_segment(
            [pos2(x, rect.bottom()), pos2(x + rect.height(), rect.top())],
            Stroke::new(1.0, Color32::from_black_alpha(90)),
        );
        x += 8.0;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn paint_track(
    painter: &Painter,
    tr: &TrackRect,
    scale: &Scale,
    t: &SubtitleTrack,
    speakers: &vidcut_core::speakers::Speakers,
    selected: &HashSet<&str>,
    compact: bool,
    typing: Option<(&str, &str)>,
) {
    let rect = tr.rect;
    painter.rect_filled(rect, CornerRadius::ZERO, theme::colors().track);
    let fill = if t.shown {
        FILL
    } else {
        let track = theme::colors().track;
        let mix = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
        Color32::from_rgb(
            mix(FILL.r(), track.r()),
            mix(FILL.g(), track.g()),
            mix(FILL.b(), track.b()),
        )
    };
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    for (c, r) in placed(t, scale, rect) {
        let r = if r.width() >= 2.0 {
            r.shrink2(vec2(0.5, 0.0))
        } else {
            r
        };
        painter.rect_filled(r, theme::clip_corners(), fill);
        if let Some([red, green, blue]) = speakers
            .color_of(c)
            .as_deref()
            .and_then(vidcut_core::subtitles::rgb)
        {
            let stripe = r.with_min_y(r.bottom() - (r.height() / 5.0).clamp(2.0, 4.0));
            painter.rect_filled(stripe, 0.0, Color32::from_rgb(red, green, blue));
        }
        if selected.contains(c.id.as_str()) {
            painter.rect_stroke(
                r,
                CornerRadius::same(2),
                Stroke::new(2.0, theme::colors().accent),
                StrokeKind::Inside,
            );
        }
        if compact || r.width() < 24.0 || r.height() < 10.0 {
            continue;
        }
        let text = match typing {
            Some((id, text)) if id == c.id => text,
            _ => c.text.as_str(),
        };
        let left = r.left().max(painter.clip_rect().left());
        let room = r.right() - left - 6.0;
        let label = elide(&one_line(text), (room / 6.0).max(1.0) as usize);
        painter.text(
            pos2(left + 3.0, r.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional((r.height() - 4.0).clamp(8.0, 11.0)),
            Color32::WHITE,
        );
    }
    if t.locked {
        hatch(&painter, rect);
    }
}

impl Workspace {
    pub(super) fn cue_gesture(
        &mut self,
        p: &Project,
        rows: &Rows,
        scale: &Scale,
        press: Pos2,
        upper: bool,
        additive: bool,
    ) -> Option<Gesture> {
        let row = rows.at(press.y).filter(|r| is_subtitle(&r.track))?;
        let t = p.subtitle_track(&row.track)?;
        let under = hit(t, scale, row.rect.rect, press).filter(|_| !t.locked);
        Some(match under {
            Some(CueHit::Edge(cue, edge)) => {
                let c = p.cue(&cue)?;
                Gesture::CueTrim {
                    from: if edge == Edge::Head { c.start } else { c.end() },
                    cue,
                    edge,
                }
            }
            Some(CueHit::Body(id)) => {
                let mut cues = if self.snap.selection.cues.contains(&id) {
                    self.snap.selection.cues.clone()
                } else {
                    self.send("selection.set", json!({ "cues": [id], "clips": [] }));
                    vec![id]
                };
                cues.retain(|c| p.cue(c).is_some());
                let start = cues
                    .iter()
                    .filter_map(|c| p.cue(c))
                    .map(|c| c.start)
                    .min()?;
                let end = cues.iter().filter_map(|c| p.cue(c)).map(Cue::end).max()?;
                Gesture::CueMove {
                    cues,
                    from: row.track.clone(),
                    grab: scale.frame(press.x),
                    start,
                    len: end - start,
                }
            }
            None if upper => Gesture::Scrub {
                drag_content: false,
                from: self.drawn_playhead() as f64,
            },
            None => Gesture::Marquee { additive },
        })
    }

    pub(super) fn cue_hover(
        &self,
        ui: &egui::Ui,
        p: &Project,
        row: &Row,
        scale: &Scale,
        pos: Pos2,
    ) {
        let Some(t) = p.subtitle_track(&row.track).filter(|t| !t.locked) else {
            return;
        };
        let Some(CueHit::Edge(id, edge)) = hit(t, scale, row.rect.rect, pos) else {
            return;
        };
        let Some(c) = p.cue(&id) else {
            return;
        };
        let (x, side, kind) = match edge {
            Edge::Head => (scale.xf(c.start), 1.0, paint::TrimPointer::Start),
            Edge::Tail => (scale.xf(c.end()), -1.0, paint::TrimPointer::End),
        };
        let green = Color32::from_rgb(90, 220, 120);
        let ys = row.rect.rect.y_range();
        let span = |w: f32| {
            let far = x + side * w;
            Rect::from_x_y_ranges(x.min(far)..=x.max(far), ys)
        };
        ui.painter()
            .rect_filled(span(18.0), 0.0, green.gamma_multiply(0.25));
        ui.painter().rect_filled(span(4.0), 0.0, green);
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("timeline-pointer"),
        ));
        paint::trim_pointer(&painter, pos, kind);
    }

    pub(super) fn cue_secondary(
        &mut self,
        p: &Project,
        rows: &Rows,
        scale: &Scale,
        pos: Pos2,
        upper: bool,
    ) -> bool {
        let Some(row) = rows.at(pos.y).filter(|r| is_subtitle(&r.track)) else {
            return false;
        };
        let cue = p
            .subtitle_track(&row.track)
            .and_then(|t| hit(t, scale, row.rect.rect, pos))
            .map(|h| h.cue().to_string());
        if let Some(id) = &cue {
            self.timeline.transition = None;
            self.timeline.edit = None;
            if !self.snap.selection.cues.contains(id) {
                self.send("selection.set", json!({ "cues": [id], "clips": [] }));
            }
        }
        self.timeline.cue_menu = Some(CueMenu {
            upper,
            track: row.track.clone(),
            cue,
        });
        self.timeline.clip_menu = None;
        self.timeline.transition_menu = None;
        true
    }

    pub(super) fn cue_menus(&mut self, resp: &egui::Response, p: &Project, upper: bool) {
        let Some(m) = self.timeline.cue_menu.clone().filter(|m| m.upper == upper) else {
            return;
        };
        if resp.context_menu(|ui| self.cue_menu(ui, p, &m)).is_none() {
            self.timeline.cue_menu = None;
        }
    }

    pub(super) fn cue_click(
        &mut self,
        resp: &egui::Response,
        p: &Project,
        (rows, scale): (&Rows, &Scale),
        pos: Pos2,
        upper: bool,
        mods: egui::Modifiers,
    ) -> bool {
        let Some(row) = rows.at(pos.y).filter(|r| is_subtitle(&r.track)) else {
            return false;
        };
        let under = p
            .subtitle_track(&row.track)
            .and_then(|t| hit(t, scale, row.rect.rect, pos))
            .map(|h| h.cue().to_string());
        if resp.double_clicked() || resp.triple_clicked() {
            if let Some(id) = under {
                self.open_cue(&id);
            }
            return true;
        }
        if !resp.clicked() {
            return false;
        }
        self.show_timeline();
        self.timeline.transition = None;
        self.timeline.edit = None;
        match under {
            Some(id) => self.click_cue(&id, mods, p),
            None => {
                if !mods.command {
                    self.send("selection.set", json!({ "clips": [], "cues": [] }));
                }
                if upper {
                    self.seek(
                        crate::live::PlayerId::Timeline,
                        scale.under(pos.x).clamp(0, p.end()),
                        false,
                    );
                }
            }
        }
        true
    }

    fn click_cue(&mut self, id: &str, mods: egui::Modifiers, p: &Project) {
        let mut sel = self.snap.selection.cues.clone();
        if mods.command {
            match sel.iter().position(|c| c == id) {
                Some(i) => {
                    sel.remove(i);
                }
                None => sel.push(id.to_string()),
            }
            self.send("selection.set", json!({ "cues": sel }));
            return;
        }
        if mods.shift
            && let Some(last) = sel.last()
            && let (Some((ta, a)), Some((tb, b))) = (p.find_cue(last), p.find_cue(id))
            && ta == tb
        {
            sel = p.subtitles[ta].cues[a.min(b)..=a.max(b)]
                .iter()
                .map(|c| c.id.clone())
                .collect();
        } else {
            sel = vec![id.to_string()];
        }
        self.send("selection.set", json!({ "cues": sel, "clips": [] }));
    }

    fn cue_menu(&mut self, ui: &mut egui::Ui, p: &Project, m: &CueMenu) {
        let at = self.snap.timeline.position;
        if let Some(id) = &m.cue
            && let Some(c) = p.cue(id)
        {
            let cues = if self.snap.selection.cues.contains(id) {
                self.snap.selection.cues.clone()
            } else {
                vec![id.clone()]
            };
            if ui.button("Edit Subtitle…").clicked() {
                self.open_cue(id);
                ui.close();
            }
            ui.menu_button("Position", |ui| {
                for (place, label) in [
                    (Place::Top, "Top"),
                    (Place::Middle, "Middle"),
                    (Place::Bottom, "Bottom"),
                ] {
                    if ui.radio(c.place == place, label).clicked() {
                        self.send_all(
                            unlocked(p, &cues)
                                .iter()
                                .map(|q| {
                                    command("subtitles.set", json!({ "cue": q, "place": place }))
                                })
                                .collect(),
                            &format!(
                                "Show {} subtitles at the {}",
                                cues.len(),
                                label.to_lowercase()
                            ),
                        );
                        ui.close();
                    }
                }
            });
            ui.separator();
            let inside = c.start < at && at < c.end();
            if ui
                .add_enabled(
                    inside,
                    egui::Button::new(keys("Split at Playhead  (Ctrl+B)")),
                )
                .clicked()
            {
                self.split_at_playhead();
                ui.close();
            }
            if !inside {
                super::super::dialog::why(ui, "the playhead isn't inside it");
            }
            let track = p.find_cue(id).map(|(t, _)| t);
            let joinable =
                cues.len() > 1 && cues.iter().all(|q| p.find_cue(q).map(|(t, _)| t) == track);
            let join = ui.add_enabled(joinable, egui::Button::new("Join Subtitles"));
            if !joinable {
                super::super::dialog::why(ui, "select two or more on one track");
            }
            if join.clicked() {
                self.send("subtitles.join", json!({ "cues": cues }));
                ui.close();
            }
            if ui.button("Delete  (Delete)").clicked() {
                self.delete_key(false);
                ui.close();
            }
            ui.separator();
        }
        self.subtitle_track_items(ui, &m.track);
    }

    fn subtitle_track_items(&mut self, ui: &mut egui::Ui, track: &str) {
        if ui.button("Add Subtitle").clicked() {
            self.add_subtitle(ui.ctx(), Some(track));
            ui.close();
        }
        if ui.button("Export Subtitle…").clicked() {
            self.export_subtitle(ui.ctx(), Some(track));
            ui.close();
        }
        if ui.button("Re-time to Speech…").clicked() {
            self.open_retime(track);
            ui.close();
        }
        if ui.button("Synchronize Word Timings").clicked() {
            self.time_words(track);
            ui.close();
        }
        let style = self
            .project()
            .and_then(|p| p.subtitle_track(track))
            .map(|t| t.style.clone());
        let looks = self.looks.of(LookKind::Subtitles).to_vec();
        ui.menu_button("Apply Look", |ui| {
            for (i, l) in looks.iter().enumerate() {
                let Styled::Subtitles(s) = &l.styled else {
                    continue;
                };
                if i > 0 && looks[i - 1].built_in && !l.built_in {
                    ui.separator();
                }
                let r = ui.selectable_label(style.as_ref() == Some(&s.style), &l.name);
                if r.clicked() {
                    self.send("track.set", json!({ "track": track, "look": l.name }));
                    ui.close();
                }
            }
        });
    }

    fn send_all(&self, commands: Vec<Value>, label: &str) {
        match commands.as_slice() {
            [] => {}
            [one] => self.send(
                one["method"].as_str().unwrap_or_default(),
                one["params"].clone(),
            ),
            _ => self.send("apply", json!({ "commands": commands, "label": label })),
        }
    }

    fn send_batch(&self, p: &Project, commands: Vec<Value>) {
        let names: Vec<String> = commands
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let name = vidcut_core::describe::describe(
                    c["method"].as_str().unwrap_or_default(),
                    &c["params"],
                    Some(p),
                );
                match (i, name.chars().next()) {
                    (0, _) | (_, None) => name,
                    (_, Some(first)) => first.to_lowercase().chain(name.chars().skip(1)).collect(),
                }
            })
            .collect();
        let label = match names.as_slice() {
            [rest @ .., last] if !rest.is_empty() => format!("{}, and {last}", rest.join(", ")),
            _ => names.concat(),
        };
        self.send_all(commands, &label);
    }

    pub(crate) fn delete_selected(&self, lift: bool) {
        let Some(p) = self.project() else {
            return;
        };
        let selection = &self.snap.selection;
        let cues = unlocked(p, &selection.cues);
        let (cues, clips) = if selection.clips.is_empty() {
            (selection.cues.clone(), Vec::new())
        } else if cues.is_empty() {
            (Vec::new(), selection.clips.clone())
        } else {
            (cues, unlocked_clips(p, &selection.clips, None))
        };
        let mut commands = Vec::new();
        if !cues.is_empty() {
            commands.push(command("subtitles.remove", json!({ "cues": cues })));
        }
        if !clips.is_empty() {
            commands.push(command(
                "edit.delete",
                json!({ "clips": clips, "lift": lift }),
            ));
        }
        self.send_batch(p, commands);
    }

    pub(crate) fn add_subtitle(&mut self, ctx: &egui::Context, track: Option<&str>) {
        let mut params = json!({ "text": "", "at": self.snap.timeline.position });
        if let Some(t) = track {
            params["track"] = json!(t);
        }
        self.timeline.adding = Some(Ask::send(&self.live, ctx, "subtitles.add", params));
    }

    pub(crate) fn split_at_playhead(&self) {
        let Some(p) = self.project() else {
            return;
        };
        let at = self.snap.timeline.position;
        let selection = &self.snap.selection;
        let under = |cues: &[String]| -> Vec<Value> {
            cues.iter()
                .filter(|q| p.cue(q).is_some_and(|c| c.start < at && at < c.end()))
                .map(|q| command("subtitles.split", json!({ "cue": q, "at": at })))
                .collect()
        };
        let split = |clips: &[String]| command("edit.split", json!({ "at": at, "clips": clips }));
        let commands = if selection.cues.is_empty() {
            vec![split(&selection.clips)]
        } else if selection.clips.is_empty() {
            under(&selection.cues)
        } else {
            let cues = under(&unlocked(p, &selection.cues));
            let clips = unlocked_clips(p, &selection.clips, Some(at));
            match (cues.is_empty(), clips.is_empty()) {
                (true, _) => vec![split(&selection.clips)],
                (false, true) => cues,
                (false, false) => std::iter::once(split(&clips)).chain(cues).collect(),
            }
        };
        self.send_batch(p, commands);
    }

    pub(super) fn subtitle_header(&mut self, ui: &mut egui::Ui, rect: Rect, t: &SubtitleTrack) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child.set_clip_rect(rect.intersect(child.clip_rect()));
        child.spacing_mut().button_padding.x = 3.0;
        let parts = header_parts(rect);
        let mut name = child.new_child(
            egui::UiBuilder::new()
                .max_rect(parts.name.with_min_x(parts.enlarge.left()))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let r = name.add(
            egui::Button::selectable(false, RichText::new(t.display_name()).strong().small())
                .truncate(),
        );
        if r.double_clicked() {
            self.open_track_dialog(t);
        }
        r.context_menu(|ui| {
            self.subtitle_track_items(ui, &t.id);
            if ui.button("Name and Style…").clicked() {
                self.open_track_dialog(t);
                ui.close();
            }
            if ui.button("Add Subtitle Track").clicked() {
                self.send("track.add", json!({ "kind": "subtitle" }));
                ui.close();
            }
            ui.separator();
            let n = t.cues.len();
            let text = if n == 0 {
                "Remove Track".to_string()
            } else {
                format!("Remove Track and Its {n} Subtitle(s)")
            };
            if ui.button(text).clicked() {
                self.send("track.remove", json!({ "track": t.id, "force": true }));
                ui.close();
            }
        });
        let lock = if t.locked {
            icon::LOCKED
        } else {
            icon::UNLOCKED
        };
        let shown = if t.shown {
            icon::VIDEO_ON
        } else {
            icon::VIDEO_OFF
        };
        let buttons = [
            (
                "Lock",
                t.locked,
                lock,
                json!({ "track": t.id, "locked": !t.locked }),
                parts.toggles[0],
            ),
            (
                "Show",
                !t.shown,
                shown,
                json!({ "track": t.id, "shown": !t.shown }),
                parts.toggles[2],
            ),
        ];
        for (what, lit, glyph, change, at) in buttons {
            let name = format!("{what} {}", t.id);
            if header_button(&mut child, at, glyph, &name, lit).clicked() {
                self.send("track.set", change);
            }
        }
    }
}
