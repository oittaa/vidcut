use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Stroke,
    StrokeKind, pos2, vec2,
};
use serde_json::json;
use vidcut_core::model::{Clip, Project};
use vidcut_core::ops::filmstrip;

use super::theme;
use super::tiles::{self, TileKey};
use super::timeline::{EditSel, Side, TrackRef};
use super::workspace::{Workspace, fit};

const GAP: f32 = 18.0;
const BAR: f32 = 5.0;
const ROLL_GRAB: f32 = 24.0;
const FADE_GRAB: f32 = 8.0;

pub(super) fn band_height(area: Rect) -> f32 {
    2.0 * strip_height(area) + GAP
}

fn strip_height(area: Rect) -> f32 {
    (area.height() * 0.14).clamp(36.0, 72.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TrimAnchor {
    track: TrackRef,
    out: Option<(String, i64)>,
    inc: Option<(String, i64)>,
}

impl TrimAnchor {
    fn of(track: TrackRef, out: Option<&Clip>, inc: Option<&Clip>) -> Self {
        let held = |c: Option<&Clip>| c.map(|c| (c.id.clone(), c.duration));
        Self {
            track,
            out: held(out),
            inc: held(inc),
        }
    }

    fn holds(&self, track: TrackRef, out: Option<&Clip>, inc: Option<&Clip>) -> bool {
        let id = |h: &Option<(String, i64)>| h.as_ref().map(|(id, _)| id.clone());
        self.track == track
            && id(&self.out) == out.map(|c| c.id.clone())
            && id(&self.inc) == inc.map(|c| c.id.clone())
    }

    fn moved(&self, out: Option<&Clip>, inc: Option<&Clip>) -> (i64, i64) {
        let since = |h: &Option<(String, i64)>, c: Option<&Clip>| {
            h.as_ref().zip(c).map_or(0, |((_, d), c)| c.duration - d)
        };
        (since(&self.out, out), -since(&self.inc, inc))
    }
}

struct FadeBar<'a> {
    x: &'a vidcut_core::model::Transition,
    sound: bool,
    room: (i64, i64),
    roll: bool,
    ppf: f32,
}

struct Strip<'a> {
    rect: Rect,
    clip: &'a Clip,
    outgoing: bool,
    shift: i64,
    moved: i64,
    step: i64,
}

pub(super) fn fade_step(frames: i64, cells: f32) -> i64 {
    ((frames as f32 * 1.6 / cells.max(1.0)).ceil() as i64).max(1)
}

pub(super) fn grown(before: i64, after: i64, grow: i64, room: (i64, i64)) -> (i64, i64) {
    let side = |s: i64, room: i64| {
        if s == 0 {
            0
        } else {
            (s + grow).max(1).min(room)
        }
    };
    (side(before, room.0), side(after, room.1))
}

fn beside_roll(cut: f32, at: f32, toward: f32) -> f32 {
    let clear = (ROLL_GRAB + FADE_GRAB) / 2.0;
    cut + toward * (toward * (at - cut)).max(clear)
}

impl Workspace {
    pub(super) fn trim_strips(&mut self, ui: &mut egui::Ui, band: Rect, p: &Project, e: EditSel) {
        ui.allocate_rect(band, Sense::hover());
        let Some(t) = p.track(&e.track.id()) else {
            return;
        };
        let out = t.clips.iter().find(|c| c.end() == e.at);
        let inc = t.clips.iter().find(|c| c.start == e.at);
        if !self
            .trim_anchor
            .as_ref()
            .is_some_and(|a| a.holds(e.track, out, inc))
        {
            self.trim_anchor = Some(TrimAnchor::of(e.track, out, inc));
        }
        let (out_moved, in_moved) = self
            .trim_anchor
            .as_ref()
            .map_or((0, 0), |a| a.moved(out, inc));
        let row = (band.height() - GAP) / 2.0;
        let h = (row - BAR).max(8.0);
        let aspect = p.settings.width as f32 / p.settings.height.max(1) as f32;
        let w = (h * aspect).max(8.0);
        let cut = band.center().x.round();
        let fade = self.trim_transition(p, e);
        let step = fade
            .as_ref()
            .map_or(1, |x| fade_step(x.before + x.after, band.width() / w));
        let fx = |d: i64| cut + d as f32 / step as f32 * w;
        let top = Rect::from_min_size(band.min, vec2(band.width(), row));
        let bottom = Rect::from_min_size(
            pos2(band.left(), band.bottom() - row),
            vec2(band.width(), row),
        );
        let gap = Rect::from_x_y_ranges(band.x_range(), top.bottom()..=bottom.top());
        let grab = |rect: Rect, width: f32, label: &str| {
            let at = Rect::from_center_size(pos2(cut, rect.center().y), vec2(width, rect.height()));
            let r = ui
                .interact(at, ui.scope_id().with(("trim-strip", label)), Sense::drag())
                .on_hover_cursor(CursorIcon::ResizeHorizontal)
                .on_hover_text(label);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Unknown, true, label));
            r
        };
        let handles = [
            (Side::Out, out.map(|_| grab(top, 12.0, "Trim the Out"))),
            (Side::In, inc.map(|_| grab(bottom, 12.0, "Trim the In"))),
            (
                Side::Both,
                out.and(inc).map(|_| grab(gap, ROLL_GRAB, "Roll the edit")),
            ),
        ];
        let reach = |side: Side| match (side, out, inc) {
            (Side::Out, Some(o), _) => Some(filmstrip::out_reach(p, o)),
            (Side::In, _, Some(i)) => Some(filmstrip::in_reach(p, i)),
            (Side::Both, Some(o), Some(i)) => Some(filmstrip::roll_reach(p, o, i)),
            _ => None,
        };
        let ppf = self.timeline.options.ppf.max(0.01) as f32;
        let active = handles.iter().find_map(|(side, r)| {
            let r = r.as_ref().filter(|r| r.dragged() || r.drag_stopped())?;
            let (lo, hi) = reach(*side)?;
            let by = match r.total_drag_delta() {
                Some(d) => {
                    let by = ((d.x / ppf).round() as i64).clamp(lo, hi.max(lo));
                    ui.data_mut(|m| m.insert_temp(r.id, by));
                    by
                }
                None => ui.data_mut(|m| m.remove_temp::<i64>(r.id)).unwrap_or(0),
            };
            Some((*side, by, r.drag_stopped()))
        });
        self.timeline.strip_drag = match active {
            Some((side, by, false)) if by != 0 => Some((EditSel { side, ..e }, by)),
            _ => None,
        };
        let shift = |side: Side| match active {
            Some((s, by, _)) if s == side || s == Side::Both => by,
            _ => 0,
        };
        let painter = ui.painter_at(band);
        let ppp = ui.ctx().pixels_per_point();
        for strip in [
            out.map(|clip| Strip {
                rect: top,
                clip,
                outgoing: true,
                shift: shift(Side::Out),
                moved: out_moved,
                step,
            }),
            inc.map(|clip| Strip {
                rect: bottom,
                clip,
                outgoing: false,
                shift: shift(Side::In),
                moved: in_moved,
                step,
            }),
        ]
        .into_iter()
        .flatten()
        {
            self.paint_strip(&painter, p, &strip, cut, w, ppp);
        }
        let (ok, stop) = (theme::colors().ok, theme::colors().error);
        let mark = |rect: Rect, width: f32, color: Color32| {
            let r = Rect::from_center_size(pos2(cut, rect.center().y), vec2(width, rect.height()));
            painter.rect_filled(r, CornerRadius::same(1), color);
        };
        if let Some((_, hi)) = reach(Side::Out) {
            mark(top, 3.0, if shift(Side::Out) >= hi { stop } else { ok });
        }
        if let Some((lo, _)) = reach(Side::In) {
            mark(bottom, 3.0, if shift(Side::In) <= lo { stop } else { ok });
        }
        if let Some(x) = &fade {
            let sound = t
                .transitions
                .iter()
                .any(|y| y.sound && y.from == x.from && y.to == x.to);
            let room = vidcut_core::ops::dissolve_room(p, &e.track.id(), e.at, false)
                .unwrap_or((x.before, x.after));
            let bar = FadeBar {
                x,
                sound,
                room,
                roll: out.is_some() && inc.is_some(),
                ppf: w / step as f32,
            };
            self.fade_bar(ui, &painter, [gap, top, bottom], e, &bar, &fx);
        }
        if out.is_some() && inc.is_some() {
            mark(gap.shrink2(vec2(0.0, 4.0)), 3.0, Color32::WHITE);
        }
        if let Some((side, by, true)) = active
            && by != 0
        {
            self.timeline.edit = Some(EditSel {
                at: e.at,
                side,
                track: e.track,
            });
            self.nudge_edit(side, by);
        }
    }

    fn fade_bar(
        &mut self,
        ui: &mut egui::Ui,
        painter: &Painter,
        [gap, top, bottom]: [Rect; 3],
        e: EditSel,
        bar: &FadeBar,
        fx: &dyn Fn(i64) -> f32,
    ) {
        let FadeBar {
            x,
            sound,
            room,
            roll,
            ppf,
        } = *bar;
        let edges = [
            (false, "The dissolve's start"),
            (true, "The dissolve's end"),
        ]
        .map(|(end, label)| {
            let (at, toward) = match (end, x.before, x.after) {
                (false, 0, _) => (fx(0), 1.0),
                (false, b, _) => (fx(-b), -1.0),
                (true, _, 0) => (fx(0), -1.0),
                (true, _, a) => (fx(a), 1.0),
            };
            let at = if roll {
                beside_roll(fx(0), at, toward)
            } else {
                at
            };
            let r = ui
                .interact(
                    Rect::from_center_size(pos2(at, gap.center().y), vec2(FADE_GRAB, gap.height())),
                    ui.scope_id().with(("trim-fade", end)),
                    Sense::drag(),
                )
                .on_hover_cursor(CursorIcon::ResizeHorizontal)
                .on_hover_text(label);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Unknown, true, label));
            (end, r)
        });
        let grow = edges.iter().find_map(|(end, r)| {
            if !(r.dragged() || r.drag_stopped()) {
                return None;
            }
            let grow = match r.total_drag_delta() {
                Some(d) => {
                    let frames = (d.x / ppf).round() as i64;
                    let grow = if *end { frames } else { -frames };
                    ui.data_mut(|m| m.insert_temp(r.id, grow));
                    grow
                }
                None => ui.data_mut(|m| m.remove_temp::<i64>(r.id)).unwrap_or(0),
            };
            Some((grow, r.drag_stopped()))
        });
        let (before, after) = grow.map_or((x.before, x.after), |(g, _)| {
            grown(x.before, x.after, g, room)
        });
        let bar = Rect::from_x_y_ranges(
            fx(-before)..=fx(after),
            gap.shrink2(vec2(0.0, 2.0)).y_range(),
        );
        painter.rect_filled(bar, CornerRadius::same(2), Color32::from_white_alpha(40));
        painter.rect_stroke(
            bar,
            CornerRadius::same(2),
            Stroke::new(1.0, Color32::from_white_alpha(190)),
            StrokeKind::Inside,
        );
        let name = x.otio["name"].as_str().unwrap_or("Cross Dissolve");
        painter.text(
            bar.left_center() + vec2(4.0, 0.0),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(11.0),
            Color32::WHITE,
        );
        for (at, row) in [(fx(-before), top), (fx(after), bottom)] {
            let r = Rect::from_center_size(pos2(at, row.bottom() - BAR / 2.0), vec2(2.0, BAR));
            painter.rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
        }
        if let Some((_, true)) = grow
            && (before, after) != (x.before, x.after)
        {
            self.send(
                "edit.dissolve",
                json!({
                    "at": e.at,
                    "track": e.track.id(),
                    "before": format!("{before}f"),
                    "after": format!("{after}f"),
                    "sound": sound,
                }),
            );
        }
    }

    fn paint_strip(&self, painter: &Painter, p: &Project, s: &Strip, cut: f32, w: f32, ppp: f32) {
        let c = s.clip;
        let step = s.step.max(1);
        let h = (s.rect.height() - BAR).max(8.0);
        let left = ((cut - s.rect.left()) / w).ceil() as i64 + 1;
        let right = ((s.rect.right() - cut) / w).ceil() as i64 + 1;
        let middle = (s.shift as f32 / step as f32).round() as i64;
        let media = p
            .media(&c.media)
            .filter(|m| m.info.video.is_some() && c.video);
        let aspect = media.and_then(tiles::media_aspect).unwrap_or(w / h);
        let height = tiles::tile_height(h * ppp);
        let width = (height as f32 * aspect).round().max(1.0) as u32;
        let (first, last) = if s.outgoing {
            (1 - c.duration, s.shift)
        } else {
            (s.shift, c.duration - 1)
        };
        let tint = c
            .shown_color(p.media(&c.media))
            .and_then(theme::clip_color)
            .unwrap_or(theme::colors().clip);
        let base = if s.outgoing { 1.0 } else { 0.0 };
        let numbers = ((middle - left)..=(middle + right)).map(|k| k * step);
        for f in filmstrip::strip(p, c, s.outgoing, numbers) {
            let inside = (first..=last).contains(&f.number);
            if f.source.is_none() && !inside {
                continue;
            }
            let x = cut + ((f.number - s.shift) as f32 / step as f32 - base) * w;
            let cell = Rect::from_min_size(pos2(x, s.rect.top()), vec2(w, h));
            painter.rect_filled(cell.shrink(0.5), CornerRadius::ZERO, Color32::from_gray(18));
            let texture = media.zip(f.source).and_then(|(m, frame)| {
                let key = TileKey {
                    media: m.id.clone(),
                    frame,
                    height,
                };
                let t = self.tiles.as_ref()?;
                t.want(&key, || t.job(p, &key, width))
                    .or_else(|| t.nearest(&m.id, height, frame, 2))
            });
            if let Some(texture) = texture {
                painter.image(
                    texture,
                    fit(cell.shrink(1.0), aspect),
                    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let bar = Rect::from_min_size(pos2(x, cell.bottom()), vec2(w, BAR));
            let color = if inside { tint } else { Color32::from_gray(90) };
            painter.rect_filled(bar.shrink2(vec2(0.0, 1.0)), CornerRadius::ZERO, color);
            let number = f.number + s.moved;
            let label = if number == 0 {
                "0".to_string()
            } else {
                format!("{number:+}")
            };
            for (offset, color) in [
                (vec2(1.0, 1.0), Color32::BLACK),
                (vec2(0.0, 0.0), Color32::WHITE),
            ] {
                painter.text(
                    cell.center() + offset,
                    Align2::CENTER_CENTER,
                    &label,
                    FontId::proportional(11.0),
                    color,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: &str, duration: i64) -> Clip {
        serde_json::from_value(serde_json::json!({
            "id": id, "media": "m1", "start": 0, "duration": duration,
            "source_in": 0, "video": true, "enabled": true
        }))
        .unwrap()
    }

    #[test]
    fn a_one_second_dissolve_shows_three_frames_a_cell_across_sixteen_cells() {
        assert_eq!(fade_step(30, 16.0), 3);
    }

    #[test]
    fn a_dissolve_starting_on_the_edit_grows_only_after_it() {
        assert_eq!(grown(0, 30, 5, (100, 100)), (0, 35));
    }

    #[test]
    fn dragging_an_uneven_dissolves_end_two_frames_out_grows_the_side_with_media() {
        assert_eq!(grown(10, 15, 2, (10, 300)), (10, 17));
    }

    #[test]
    fn a_dissolve_edge_on_the_cut_is_grabbed_beside_the_roll_handle() {
        assert_eq!(beside_roll(100.0, 100.0, 1.0), 116.0);
    }

    #[test]
    fn numbers_count_from_where_the_edit_was_when_the_editor_opened() {
        let anchor = TrimAnchor::of(TrackRef::MAIN, Some(&clip("c1", 50)), Some(&clip("c2", 40)));
        let moved = anchor.moved(Some(&clip("c1", 21)), Some(&clip("c2", 40)));
        assert_eq!(moved, (-29, 0));
    }
}
